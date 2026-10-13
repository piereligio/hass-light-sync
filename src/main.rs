
mod accent_map;
mod color_pipeline;
mod config;
mod flux_warmth;

use captrs::*;
use color_pipeline::{
    apply_yellow_tint_norm_rgb, chromaticity_hs_ha, process_rgb, sample_hue_brightness_lut,
    slew_limit_rgb, smooth_toward, PipelineParams,
};
use config::{Settings, YellowTintMode};
use console::Emoji;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static RUNNING: AtomicBool = AtomicBool::new(true);
static SHUTDOWN_INFO: Mutex<Option<(String, String, String)>> = Mutex::new(None);

#[cfg(windows)]
unsafe extern "system" fn console_ctrl_handler(_ctrl_type: u32) -> i32 {
    RUNNING.store(false, Ordering::SeqCst);
    if let Ok(guard) = SHUTDOWN_INFO.lock() {
        if let Some((ref endpoint, ref token, ref entity)) = *guard {
            let url = format!("{}/api/services/light_interpolator/stop", endpoint.trim_end_matches('/'));
            let payload = serde_json::json!({ "entity_id": entity });
            let _ = ureq::post(&url)
                .set("Authorization", &format!("Bearer {}", token.trim()))
                .timeout(Duration::from_millis(1500))
                .send_json(&payload);
        }
    }
    0
}

#[derive(Serialize)]
struct HassTurnOn {
    entity_id: String,
    brightness: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    rgb_color: Option<[u64; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hs_color: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transition: Option<f64>,
}

#[derive(Serialize)]
struct HassInterpolatorTarget {
    entity_id: String,
    target_h: f64,
    target_s: f64,
    base_brightness: u64,
    max_duration: f64,
    min_duration: f64,
    step_ms: u64,
    min_saturation_floor: f64,
    white_multiplier: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    hue_lut: Option<Vec<[f64; 2]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tuya_gradient_dp: Option<u64>,
    max_hue_step: f64,
    max_sat_step: f64,
    min_step_ms: u64,
}

/// Per-request timeout so a stalled Home Assistant cannot freeze the capture loop.
const HA_TIMEOUT: Duration = Duration::from_millis(2500);
/// Pause before the next attempt after a failed request.
const HA_RETRY_BACKOFF: Duration = Duration::from_secs(2);
/// The interpolator target is re-sent this often so the light recovers after a Home Assistant restart.
const INTERPOLATOR_HEARTBEAT: Duration = Duration::from_secs(60);
const INTERPOLATOR_STEP_MS: u64 = 35;

enum HaError {
    Unauthorized,
    Other(String),
}

fn main() {
    let term = console::Term::stdout();
    term.set_title("HASS-Light-Sync running...");

    println!("{}hass-light-sync - Starting...", Emoji("💡 ", ""));
    println!("{}Reading config...", Emoji("⚙️ ", ""));
    let settingsfile =
        std::fs::read_to_string("settings.json").expect("❌ settings.json file does not exist");

    let cleaned = strip_json_comments(&settingsfile);
    let mut settings: Settings = serde_json::from_str(cleaned.as_str())
        .expect("❌ Failed to parse settings. Please read the configuration section in the README");

    settings
        .hsl_lut
        .sort_by(|a, b| a.brightness.partial_cmp(&b.brightness).unwrap_or(std::cmp::Ordering::Equal));
    settings
        .hue_brightness_lut
        .sort_by(|a, b| {
            let ha = a.hue.rem_euclid(360.0);
            let hb = b.hue.rem_euclid(360.0);
            ha.partial_cmp(&hb).unwrap_or(std::cmp::Ordering::Equal)
        });

    println!("{}Config loaded successfully!", Emoji("✅ ", ""));

    if let Ok(mut guard) = SHUTDOWN_INFO.lock() {
        *guard = Some((
            settings.api_endpoint.clone(),
            settings.token.clone(),
            settings.light_entity_name.clone(),
        ));
    }

    #[cfg(windows)]
    unsafe {
        extern "system" {
            fn SetConsoleCtrlHandler(
                handler: Option<unsafe extern "system" fn(u32) -> i32>,
                add: i32,
            ) -> i32;
        }
        SetConsoleCtrlHandler(Some(console_ctrl_handler), 1);
    }

    let mut steps = (settings.skip_pixels as u64).max(1);
    let mut capture_ms = settings.grab_interval.max(1) as u64;
    let mut send_ms = settings
        .send_interval_ms
        .unwrap_or(capture_ms)
        .max(1);
    let mut tau_in_ms = settings.get_standard_tau_ms().max(0.0);

    let mut gamma_white = settings.get_gamma_white();

    let mut capturer = loop {
        match Capturer::new(settings.monitor_id as usize) {
            Ok(c) => break c,
            Err(err) => {
                println!(
                    "{} Waiting for screen capture interface ({:?})... retrying in 2s",
                    Emoji("⏳ ", ""),
                    err
                );
                std::thread::sleep(Duration::from_millis(2000));
            }
        }
    };

    let (mut w, mut h) = capturer.geometry();
    let mut size = (w as u64 * h as u64) / steps;

    let (mut prev_r, mut prev_g, mut prev_b) = (0.0_f64, 0.0_f64, 0.0_f64);
    let mut smoothed_accent_rgb: Option<[f64; 3]> = None;
    let mut accent_bri = 1.0_f64;
    let mut current_accent_slot: Option<accent_map::AccentSlot> = None;

    let mut last_capture: Option<Instant> = None;
    let mut last_ps: Option<Vec<Bgr8>> = None;
    let mut last_avg = (0.0_f64, 0.0_f64, 0.0_f64);
    let mut cached_hist: Vec<f64> = Vec::new();

    println!();

    let mut last_timestamp = std::time::Instant::now();
    let mut flux_interval = Duration::from_millis(settings.flux_poll_interval_ms.max(1));
    let mut last_flux_poll = Instant::now() - flux_interval;
    let mut flux_cache: Option<f64> = None;

    #[derive(Clone, Copy, PartialEq, Eq)]
    struct LastSentState {
        brightness: u64,
        rgb: Option<[u64; 3]>,
        hs: Option<[u64; 2]>,
    }

    let mut loop_tick: Option<Instant> = None;
    let mut last_sent: Option<LastSentState> = None;
    let mut last_sent_time: Option<Instant> = None;
    let mut last_dispatched_hs: Option<[i64; 2]> = None;
    let mut ha_retry_after: Option<Instant> = None;
    let mut last_dispatched_bri: Option<u64> = None;
    let mut last_interpolator_sent_time: Option<Instant> = None;

    let mut last_settings_poll = Instant::now();
    let mut last_settings_mtime = std::fs::metadata("settings.json").and_then(|m| m.modified()).ok();

    while RUNNING.load(Ordering::SeqCst) {
        let tick = Instant::now();

        if last_settings_poll.elapsed() >= Duration::from_millis(500) {
            last_settings_poll = tick;
            if let Ok(meta) = std::fs::metadata("settings.json") {
                if let Ok(mtime) = meta.modified() {
                    if Some(mtime) != last_settings_mtime {
                        last_settings_mtime = Some(mtime);
                        if let Ok(content) = std::fs::read_to_string("settings.json") {
                            let cleaned = strip_json_comments(&content);
                            if let Ok(mut new_settings) = serde_json::from_str::<Settings>(&cleaned) {
                                new_settings.hsl_lut.sort_by(|a, b| {
                                    a.brightness.partial_cmp(&b.brightness).unwrap_or(std::cmp::Ordering::Equal)
                                });
                                new_settings.hue_brightness_lut.sort_by(|a, b| {
                                    let ha = a.hue.rem_euclid(360.0);
                                    let hb = b.hue.rem_euclid(360.0);
                                    ha.partial_cmp(&hb).unwrap_or(std::cmp::Ordering::Equal)
                                });
                                steps = (new_settings.skip_pixels as u64).max(1);
                                capture_ms = new_settings.grab_interval.max(1) as u64;
                                send_ms = new_settings.send_interval_ms.unwrap_or(capture_ms).max(1);
                                tau_in_ms = new_settings.get_standard_tau_ms().max(0.0);
                                gamma_white = new_settings.get_gamma_white();
                                size = (w as u64 * h as u64) / steps;
                                flux_interval = Duration::from_millis(new_settings.flux_poll_interval_ms.max(1));
                                if let Ok(mut guard) = SHUTDOWN_INFO.lock() {
                                    *guard = Some((
                                        new_settings.api_endpoint.clone(),
                                        new_settings.token.clone(),
                                        new_settings.light_entity_name.clone(),
                                    ));
                                }
                                settings = new_settings;
                                println!("{}Settings reloaded live!", Emoji("🔄 ", ""));
                            }
                        }
                    }
                }
            }
        }
        let dt_default = (send_ms as f64) * 1e-3;
        let dt_secs = loop_tick
            .map(|t| tick.duration_since(t).as_secs_f64())
            .unwrap_or(dt_default)
            .max(1e-6);
        loop_tick = Some(tick);

        let cap_dur = Duration::from_millis(capture_ms);
        let cap_due = last_ps.is_none()
            || last_capture
                .map(|t| tick.duration_since(t) >= cap_dur)
                .unwrap_or(true);

        let mut did_capture = false;
        if cap_due {
            match capturer.capture_frame() {
                Ok(ps) => {
                    last_ps = Some(ps);
                    last_capture = Some(Instant::now());
                    did_capture = true;
                }
                Err(error) => {
                    println!("{} Failed to grab frame: {:?}. Retrying capturer in 1s...", Emoji("❗ ", ""), error);
                    std::thread::sleep(Duration::from_millis(1000));
                    if let Ok(new_cap) = Capturer::new(settings.monitor_id as usize) {
                        let (nw, nh) = new_cap.geometry();
                        w = nw;
                        h = nh;
                        size = (w as u64 * h as u64) / steps;
                        capturer = new_cap;
                        println!("{} Screen capture interface reacquired!", Emoji("✅ ", ""));
                    }
                    continue;
                }
            }
        }


        if did_capture {
            let last_frame = last_ps.as_ref().unwrap();
            let (mut total_r, mut total_g, mut total_b) = (0, 0, 0);
            let mut count = 0;
            for px in last_frame.iter() {
                if count % steps == 0 {
                    total_r += px.r as u64;
                    total_g += px.g as u64;
                    total_b += px.b as u64;
                }
                count += 1;
            }
            last_avg = (
                total_r as f64 / size as f64,
                total_g as f64 / size as f64,
                total_b as f64 / size as f64,
            );
            if settings.is_night_mode() {
                let frame_hist = accent_map::hue_histogram_weighted(
                    last_frame.as_slice(),
                    steps as usize,
                    settings.accent_mode.hist_bin_deg,
                    settings.accent_mode.min_saturation,
                );
                let sm = settings.accent_mode.hist_smoothing.clamp(0.0, 0.95);
                if cached_hist.len() == frame_hist.len() && sm > 0.0 {
                    for (c, f) in cached_hist.iter_mut().zip(frame_hist.iter()) {
                        *c = *c * sm + *f * (1.0 - sm);
                    }
                } else {
                    cached_hist = frame_hist;
                }
            }
        }

        let (avg_r, avg_g, avg_b) = last_avg;

        prev_r = smooth_toward(prev_r, avg_r, dt_secs, tau_in_ms);
        prev_g = smooth_toward(prev_g, avg_g, dt_secs, tau_in_ms);
        prev_b = smooth_toward(prev_b, avg_b, dt_secs, tau_in_ms);

        let yellow_strength = match settings.yellow_tint.mode {
            YellowTintMode::Off => 0.0,
            YellowTintMode::Manual => settings.yellow_tint.manual_strength.clamp(0.0, 1.0),
            YellowTintMode::Flux => {
                if last_flux_poll.elapsed() >= flux_interval {
                    flux_cache = flux_warmth::read_flux_warmth();
                    last_flux_poll = Instant::now();
                }
                flux_cache
                    .unwrap_or(settings.yellow_tint.manual_strength)
                    .clamp(0.0, 1.0)
            }
        };

        let pipe = PipelineParams {
            max_brightness: settings.max_brightness,
            gamma: settings.get_gamma(),
            gamma_white,
            white_chroma_max: settings.get_white_chroma_max(),
            hsl_lut: settings.get_hsl_lut(),
            hue_brightness_lut: settings.hue_brightness_lut.as_slice(),
            yellow_strength,
        };

        let max_b = settings.max_brightness.clamp(1, 255) as f64;
        let scale_out = max_b / 255.0;

        let (color_rgb, color_hs, brightness, accent_label) = if settings.is_night_mode() {
            let tau_rgb_accent = settings
                .accent_mode
                .color_smoothing_tau_ms
                .unwrap_or(1200.0)
                .max(0.0);
            let tau_bri_accent = settings
                .accent_mode
                .master_brightness_smoothing_tau_ms
                .unwrap_or(0.0)
                .max(0.0);
            let hist = cached_hist.as_slice();
            let total_w = accent_map::hist_total(hist);
            let max_w = hist.iter().cloned().fold(0.0_f64, f64::max);
            let total_samples = size as f64;
            let accent_density = if total_samples > 0.0 {
                (total_w * 2.5) / total_samples
            } else {
                0.0
            };

            let weak = total_w < 1e-9
                || max_w / total_w < settings.accent_mode.weak_peak_ratio
                || accent_density < settings.accent_mode.min_accent_weight;

            let bri_mult = settings.accent_mode.brightness_multiplier.max(0.0);
            let base_night_bri = settings
                .accent_mode
                .brightness
                .unwrap_or(settings.max_brightness)
                .clamp(1, 255) as f64;

            let (tr, tg, tb, tbri, label): (f64, f64, f64, f64, &'static str) = if weak && current_accent_slot.is_none() {
                // Initial fallback only when no previous accent has ever been selected
                if settings.accent_mode.allow_neutral_on_weak {
                    let (r, g, b, bri) = process_rgb((prev_r, prev_g, prev_b), &pipe);
                    let scaled_bri = ((bri as f64) * bri_mult).round().clamp(1.0, max_b);
                    (r as f64, g as f64, b as f64, scaled_bri, "avg")
                } else {
                    let w = settings.accent_mode.warm;
                    let bri_cap = settings
                        .accent_mode
                        .weak_fallback_brightness
                        .clamp(1, 255);
                    let rgb = w.rgb();
                    let rn = rgb[0] as f64 / 255.0;
                    let gn = rgb[1] as f64 / 255.0;
                    let bn = rgb[2] as f64 / 255.0;
                    let (rn2, gn2, bn2) =
                        apply_yellow_tint_norm_rgb(rn, gn, bn, yellow_strength);
                    let cr = (rn2 * 255.0 * scale_out).clamp(0.0, max_b);
                    let cg = (gn2 * 255.0 * scale_out).clamp(0.0, max_b);
                    let cb = (bn2 * 255.0 * scale_out).clamp(0.0, max_b);
                    let scaled_bri =
                        (((bri_cap as f64 / 255.0) * base_night_bri) * bri_mult).round().clamp(1.0, max_b);
                    (cr, cg, cb, scaled_bri, "dim")
                }
            } else {
                let slot = if weak {
                    // Elements too small or weak -> PERMANI AL VECCHIO ACCENT!
                    current_accent_slot.unwrap_or(accent_map::AccentSlot::Warm)
                } else {
                    accent_map::resolve_accent_slot_from_hist(
                        &settings.accent_mode,
                        hist,
                        current_accent_slot,
                    )
                };
                current_accent_slot = Some(slot);
                let ([r0, g0, b0], tbri_u) =
                    accent_map::accent_choice_to_rgb_bri(&settings.accent_mode, slot);
                let rn = r0 as f64 / 255.0;
                let gn = g0 as f64 / 255.0;
                let bn = b0 as f64 / 255.0;
                let (rn2, gn2, bn2) = apply_yellow_tint_norm_rgb(rn, gn, bn, yellow_strength);
                let cr = (rn2 * 255.0 * scale_out).clamp(0.0, max_b);
                let cg = (gn2 * 255.0 * scale_out).clamp(0.0, max_b);
                let cb = (bn2 * 255.0 * scale_out).clamp(0.0, max_b);
                let scaled_bri = (((tbri_u as f64 / 255.0) * base_night_bri) * bri_mult).round().clamp(1.0, max_b);
                let label = match slot {
                    accent_map::AccentSlot::Warm => "warm",
                    accent_map::AccentSlot::Magenta => "magenta",
                    accent_map::AccentSlot::Green => "green",
                    accent_map::AccentSlot::Cyan => "cyan",
                    accent_map::AccentSlot::Violet => "violet",
                };
                (cr, cg, cb, scaled_bri, label)
            };

            if settings.use_light_interpolator {
                let (h_deg, raw_s_pct) = chromaticity_hs_ha(tr, tg, tb);
                let min_s_floor = settings.accent_mode.min_saturation_floor.clamp(0.0, 100.0);
                let s_pct = raw_s_pct.max(min_s_floor);
                let bri_u = tbri.round().clamp(1.0, max_b) as u64;
                (None, Some([h_deg, s_pct]), bri_u, label)
            } else {
                // 3D Cartesian interpolation in RGB space
                let (cur_r, cur_g, cur_b) = match smoothed_accent_rgb {
                    None => {
                        smoothed_accent_rgb = Some([tr, tg, tb]);
                        (tr, tg, tb)
                    }
                    Some([r, g, b]) => {
                        let delta_limit = settings.accent_mode.max_delta_per_iter.or_else(|| {
                            settings.accent_mode.max_delta_per_sec.map(|per_sec| (per_sec * dt_secs).max(0.1))
                        });

                        let (target_r, target_g, target_b) = if tau_rgb_accent > 0.0 {
                            (
                                smooth_toward(r, tr, dt_secs, tau_rgb_accent),
                                smooth_toward(g, tg, dt_secs, tau_rgb_accent),
                                smooth_toward(b, tb, dt_secs, tau_rgb_accent),
                            )
                        } else {
                            (tr, tg, tb)
                        };

                        let [nr, ng, nb] = if let Some(limit) = delta_limit {
                            slew_limit_rgb([r, g, b], [target_r, target_g, target_b], limit)
                        } else {
                            [target_r, target_g, target_b]
                        };

                        smoothed_accent_rgb = Some([nr, ng, nb]);
                        (nr, ng, nb)
                    }
                };

                let (h_deg, raw_s_pct) = chromaticity_hs_ha(cur_r, cur_g, cur_b);
                let min_s_floor = settings.accent_mode.min_saturation_floor.clamp(0.0, 100.0);
                let s_pct = raw_s_pct.max(min_s_floor);

                let lut_mul = if settings.hue_brightness_lut.is_empty() {
                    1.0
                } else {
                    sample_hue_brightness_lut(h_deg, &settings.hue_brightness_lut)
                };
                let s_norm = (s_pct / 100.0).clamp(0.0, 1.0);
                // Same blend as the light_interpolator component
                let white_mul = settings.accent_mode.white_multiplier.clamp(0.0, 2.0);
                let eff_mul = white_mul + s_norm * (lut_mul - white_mul);
                let tbri_calibrated = (tbri * eff_mul).clamp(1.0, max_b);

                let bri_u = if tau_bri_accent <= 0.0 {
                    accent_bri = tbri_calibrated;
                    tbri_calibrated.round().clamp(1.0, max_b) as u64
                } else {
                    accent_bri = smooth_toward(accent_bri, tbri_calibrated, dt_secs, tau_bri_accent);
                    accent_bri.round().clamp(1.0, max_b) as u64
                };

                (
                    None,
                    Some([h_deg, s_pct]),
                    bri_u,
                    label,
                )
            }
        } else {
            let (r, g, b, bri) = process_rgb((prev_r, prev_g, prev_b), &pipe);
            (Some([r, g, b]), None, bri, "")
        };

        let time_elapsed = last_timestamp.elapsed().as_millis();
        last_timestamp = std::time::Instant::now();

        let _ = term.move_cursor_up(1);
        let _ = term.clear_line();
        if settings.is_night_mode() {
            let hs = color_hs.expect("accent sends hs");
            println!(
                "{}HS {:.1} {:.1}%  bri {}  yellow {:.2}  accent {}  FPS {}",
                Emoji("💡 ", ""),
                hs[0],
                hs[1],
                brightness,
                yellow_strength,
                accent_label,
                if time_elapsed > 0 {
                    1000 / time_elapsed
                } else {
                    0
                }
            );
        } else {
            let rgb = color_rgb.expect("non-accent sends rgb");
            println!(
                "{}RGB {:?}  bri {}  yellow {:.2}  FPS {}",
                Emoji("💡 ", ""),
                rgb,
                brightness,
                yellow_strength,
                if time_elapsed > 0 {
                    1000 / time_elapsed
                } else {
                    0
                }
            );
        }
        let ha_ready = ha_retry_after.map(|t| tick >= t).unwrap_or(true);
        if settings.is_night_mode() && settings.use_light_interpolator {
            if let Some([h, s]) = color_hs {
                // Quantized like the bulb (integer hue), so yellow tint drift is dispatched too
                let target_hs = [(h.round() as i64).rem_euclid(360), s.round() as i64];
                let hs_changed = last_dispatched_hs != Some(target_hs);
                let bri_changed = last_dispatched_bri != Some(brightness);
                let heartbeat_due = last_interpolator_sent_time
                    .map(|t| tick.duration_since(t) >= INTERPOLATOR_HEARTBEAT)
                    .unwrap_or(true);

                if ha_ready && (hs_changed || bri_changed || heartbeat_due) {
                    match send_ha_interpolator(&settings, h, s, brightness) {
                        Ok(()) => {
                            last_dispatched_hs = Some(target_hs);
                            last_dispatched_bri = Some(brightness);
                            last_interpolator_sent_time = Some(tick);
                            ha_retry_after = None;
                        }
                        Err(e) => {
                            report_ha_error(&e);
                            ha_retry_after = Some(tick + HA_RETRY_BACKOFF);
                        }
                    }
                }
            }
        } else {
            let current_state = LastSentState {
                brightness,
                rgb: color_rgb,
                hs: color_hs.map(|[h, s]| [(h * 10.0).round() as u64, (s * 10.0).round() as u64]),
            };

            let time_since_last_sent = last_sent_time.map(|t| tick.duration_since(t));
            let heartbeat_due = time_since_last_sent
                .map(|d| d >= Duration::from_secs(3))
                .unwrap_or(true);

            let state_changed = match last_sent {
                None => true,
                Some(prev) => prev != current_state,
            };

            if ha_ready && (state_changed || heartbeat_due) {
                match send_ha_light(
                    &settings,
                    brightness,
                    color_rgb,
                    color_hs,
                    settings.transition.filter(|t| *t > 0.0 && t.is_finite()),
                ) {
                    Ok(()) => {
                        last_sent = Some(current_state);
                        last_sent_time = Some(tick);
                        ha_retry_after = None;
                    }
                    Err(e) => {
                        report_ha_error(&e);
                        ha_retry_after = Some(tick + HA_RETRY_BACKOFF);
                    }
                }
            }
        }

        let loop_elapsed = tick.elapsed();
        let target_dur = Duration::from_millis(send_ms);
        if loop_elapsed < target_dur {
            std::thread::sleep(target_dur - loop_elapsed);
        }
    }

    if settings.is_night_mode() && settings.use_light_interpolator {
        send_ha_interpolator_stop(&settings);
        println!("\n{}Light interpolator stopped on exit.", Emoji("🛑 ", ""));
    }
}

/// POSTs a service call to Home Assistant. `service` is `<domain>/<service>`.
fn post_ha_service<T: Serialize>(
    settings: &Settings,
    service: &str,
    body: &T,
    timeout: Duration,
) -> Result<(), HaError> {
    let url = format!(
        "{}/api/services/{}",
        settings.api_endpoint.trim_end_matches('/'),
        service
    );

    let response = ureq::post(&url)
        .set(
            "Authorization",
            &format!("Bearer {}", settings.token.trim()),
        )
        .timeout(timeout)
        .send_json(body);

    match response {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(401, _)) => Err(HaError::Unauthorized),
        Err(ureq::Error::Status(code, _)) => Err(HaError::Other(format!("HTTP {}", code))),
        Err(e) => Err(HaError::Other(e.to_string())),
    }
}

/// Transient failures are retried by the main loop; a rejected token cannot recover.
fn report_ha_error(err: &HaError) {
    match err {
        HaError::Unauthorized => {
            println!(
                "{}Home Assistant rejected the token (HTTP 401). Check the token in settings.json",
                Emoji("❌ ", "")
            );
            std::process::exit(1);
        }
        HaError::Other(msg) => {
            println!(
                "{}Home Assistant request failed: {} (retrying)",
                Emoji("❗ ", ""),
                msg
            );
            // The status line overwrites the line above it
            println!();
        }
    }
}

fn send_ha_light(
    settings: &Settings,
    brightness: u64,
    rgb: Option<[u64; 3]>,
    hs: Option<[f64; 2]>,
    transition: Option<f64>,
) -> Result<(), HaError> {
    let api_body = HassTurnOn {
        entity_id: settings.light_entity_name.clone(),
        brightness,
        rgb_color: rgb,
        hs_color: hs,
        transition,
    };

    post_ha_service(settings, "light/turn_on", &api_body, HA_TIMEOUT)
}

/// Hue LUT as `[hue, multiplier]` pairs sorted by hue in `[0, 360)`; `360` folds onto `0`.
fn normalized_hue_lut(lut: &[config::HueBrightnessLutPoint]) -> Vec<[f64; 2]> {
    let mut pts: Vec<[f64; 2]> = lut
        .iter()
        .map(|p| [p.hue.rem_euclid(360.0), p.brightness_mul])
        .collect();
    pts.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
    pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-9);
    pts
}

fn send_ha_interpolator(
    settings: &Settings,
    target_h: f64,
    target_s: f64,
    base_brightness: u64,
) -> Result<(), HaError> {
    let hue_lut = if settings.hue_brightness_lut.is_empty() {
        None
    } else {
        Some(normalized_hue_lut(&settings.hue_brightness_lut))
    };

    let max_duration = settings.accent_mode.transition_duration_secs.max(0.1);

    let payload = HassInterpolatorTarget {
        entity_id: settings.light_entity_name.clone(),
        target_h: (target_h * 10.0).round() / 10.0,
        target_s: (target_s * 10.0).round() / 10.0,
        base_brightness,
        max_duration,
        min_duration: settings
            .accent_mode
            .transition_min_duration_secs
            .clamp(0.0, max_duration),
        step_ms: match settings.accent_mode.tuya_gradient_dp {
            Some(_) => settings.accent_mode.tuya_gradient_step_ms.max(30),
            None => INTERPOLATOR_STEP_MS,
        },
        min_saturation_floor: settings.accent_mode.min_saturation_floor.clamp(0.0, 100.0),
        white_multiplier: settings.accent_mode.white_multiplier.clamp(0.0, 2.0),
        hue_lut,
        tuya_gradient_dp: settings.accent_mode.tuya_gradient_dp,
        max_hue_step: settings.accent_mode.tuya_gradient_max_hue_step_deg.clamp(0.5, 45.0),
        max_sat_step: settings.accent_mode.tuya_gradient_max_sat_step_pct.clamp(0.5, 100.0),
        min_step_ms: settings.accent_mode.tuya_gradient_min_step_ms.clamp(30, 2000),
    };

    post_ha_service(settings, "light_interpolator/set_target", &payload, HA_TIMEOUT)
}

fn send_ha_interpolator_stop(settings: &Settings) {
    let payload = serde_json::json!({
        "entity_id": settings.light_entity_name.clone(),
    });

    let _ = post_ha_service(
        settings,
        "light_interpolator/stop",
        &payload,
        Duration::from_millis(1500),
    );
}

/// Strips `// ...` and `/* ... */` comments from JSON without altering string literals.
fn strip_json_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(next_c) = chars.next() {
                    out.push(next_c);
                }
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if c == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for nc in chars.by_ref() {
                if nc == '\n' {
                    out.push('\n');
                    break;
                }
            }
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            while let Some(nc) = chars.next() {
                if nc == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_json_comments() {
        let input = "{\n  \"api\": \"http://example.com\", // comment\n  /* block */ \"k\": 1\n}";
        let stripped = strip_json_comments(input);
        assert!(stripped.contains("http://example.com"));
        assert!(!stripped.contains("comment"));
        assert!(!stripped.contains("block"));
    }

    #[test]
    fn test_parse_disk_settings_json() {
        let content = std::fs::read_to_string("settings.json").expect("settings.json exists");
        let stripped = strip_json_comments(&content);
        let s: Settings = serde_json::from_str(&stripped).expect("parses settings.json successfully");
        assert!(s.is_night_mode());
        assert_eq!(s.accent_mode.warm.rgb(), [255, 72, 18]);
        assert_eq!(s.accent_mode.green.rgb(), [48, 205, 78]);
        assert_eq!(s.accent_mode.min_saturation_floor, 35.0);
    }

    #[test]
    fn test_normalized_hue_lut_folds_360_onto_0() {
        let point = |hue, brightness_mul| config::HueBrightnessLutPoint {
            hue,
            brightness_mul,
            color: None,
        };
        let lut = [point(240.0, 1.0), point(0.0, 0.5), point(360.0, 0.5), point(60.0, 0.625)];
        assert_eq!(
            normalized_hue_lut(&lut),
            vec![[0.0, 0.5], [60.0, 0.625], [240.0, 1.0]]
        );
    }
}
