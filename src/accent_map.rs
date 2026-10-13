//! Two-accent hue histogram and palette pick for night-friendly single-light output.

use crate::color_pipeline::rgb_to_hsl;
use crate::config::{AccentModeConfig, AccentOutput};
use captrs::Bgr8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccentSlot {
    Warm,
    Magenta,
    Green,
    Cyan,
    Violet,
}

#[derive(Debug, Clone, Copy)]
pub struct HuePeak {
    pub hue_deg: f64,
    pub weight: f64,
}

fn norm_hue(h: f64) -> f64 {
    h.rem_euclid(360.0)
}

fn hue_shortest_diff_deg(a: f64, b: f64) -> f64 {
    let d = (norm_hue(a) - norm_hue(b)).abs();
    d.min(360.0 - d)
}

fn is_warm(h: f64) -> bool {
    let h = norm_hue(h);
    h < 75.0 || h >= 345.0
}

fn is_blueish(h: f64) -> bool {
    let h = norm_hue(h);
    h >= 205.0 && h <= 265.0
}

fn is_greenish(h: f64) -> bool {
    let h = norm_hue(h);
    h >= 75.0 && h < 165.0
}

fn is_cyanish(h: f64) -> bool {
    let h = norm_hue(h);
    h >= 165.0 && h < 215.0
}

fn is_violetish(h: f64) -> bool {
    let h = norm_hue(h);
    h >= 215.0 && h < 295.0
}

fn is_magentish(h: f64) -> bool {
    let h = norm_hue(h);
    h >= 295.0 && h < 345.0
}

fn preset(cfg: &AccentModeConfig, slot: AccentSlot) -> AccentOutput {
    match slot {
        AccentSlot::Warm => cfg.warm,
        AccentSlot::Magenta => cfg.magenta,
        AccentSlot::Green => cfg.green,
        AccentSlot::Cyan => cfg.cyan,
        AccentSlot::Violet => cfg.violet,
    }
}

/// `bgr` slice in capture order. `step` same as skip_pixels stride. Returns histogram weights per hue bin.
pub fn hue_histogram_weighted(
    bgr: &[Bgr8],
    step: usize,
    bin_deg: f64,
    min_saturation: f64,
) -> Vec<f64> {
    let bin_deg = bin_deg.clamp(5.0, 60.0);
    let n_bins = (360.0 / bin_deg).ceil().max(1.0) as usize;
    let mut hist = vec![0.0_f64; n_bins];
    let mut count = 0_usize;
    for px in bgr.iter() {
        if count % step != 0 {
            count += 1;
            continue;
        }
        count += 1;
        let r = px.r as f64 / 255.0;
        let g = px.g as f64 / 255.0;
        let b = px.b as f64 / 255.0;
        let max_c = r.max(g).max(b);
        let min_c = r.min(g).min(b);
        let chroma = max_c - min_c;

        // Filter out near-black dark mode UI backgrounds (where HSL saturation explodes due to 1 / 2L)
        // and require real RGB chroma (max - min) in addition to HSV saturation.
        if max_c < 0.16 || chroma < min_saturation {
            continue;
        }
        let s_hsv = chroma / max_c;
        if s_hsv < min_saturation {
            continue;
        }

        let (h, _s_hsl, _l) = rgb_to_hsl(r, g, b);
        // Weight by chroma * s_hsv (scaled by 2.0 so colorful pixels contribute strong density without penalizing bright green).
        let w = (chroma * s_hsv * 2.0).min(1.0);
        let idx = ((norm_hue(h) / bin_deg).floor() as isize).clamp(0, n_bins as isize - 1) as usize;
        hist[idx] += w;
    }
    hist
}

pub fn top_two_peaks(hist: &[f64], bin_deg: f64) -> (HuePeak, HuePeak) {
    let n = hist.len();
    if n == 0 {
        return (
            HuePeak {
                hue_deg: 0.0,
                weight: 0.0,
            },
            HuePeak {
                hue_deg: 0.0,
                weight: 0.0,
            },
        );
    }
    let mut best_i = 0_usize;
    let mut best_w = hist[0];
    for i in 1..n {
        if hist[i] > best_w {
            best_w = hist[i];
            best_i = i;
        }
    }
    let h1 = norm_hue(best_i as f64 * bin_deg + bin_deg * 0.5);

    let mut masked = hist.to_vec();
    for di in -1_i32..=1 {
        let j = (best_i as i32 + di).rem_euclid(n as i32) as usize;
        masked[j] = 0.0;
    }
    let mut second_i = best_i;
    let mut second_w = 0.0_f64;
    for i in 0..n {
        if masked[i] > second_w {
            second_w = masked[i];
            second_i = i;
        }
    }
    let h2 = norm_hue(second_i as f64 * bin_deg + bin_deg * 0.5);
    (
        HuePeak {
            hue_deg: h1,
            weight: best_w,
        },
        HuePeak {
            hue_deg: h2,
            weight: second_w,
        },
    )
}

pub fn hist_total(hist: &[f64]) -> f64 {
    hist.iter().sum()
}

pub fn classify_hue_to_slot(h: f64) -> AccentSlot {
    if is_greenish(h) {
        AccentSlot::Green
    } else if is_cyanish(h) {
        AccentSlot::Cyan
    } else if is_violetish(h) {
        AccentSlot::Violet
    } else if is_magentish(h) {
        AccentSlot::Magenta
    } else {
        AccentSlot::Warm
    }
}

/// Pick palette slot from hue histogram with robust hysteresis support against the current slot.
pub fn resolve_accent_slot_from_hist(
    cfg: &AccentModeConfig,
    hist: &[f64],
    current_slot: Option<AccentSlot>,
) -> AccentSlot {
    let (pk1, pk2) = top_two_peaks(hist, cfg.hist_bin_deg);
    let (p1, p2) = if pk1.weight >= pk2.weight { (pk1, pk2) } else { (pk2, pk1) };
    let h1 = p1.hue_deg;
    let h2 = p2.hue_deg;
    let w1 = p1.weight;
    let w2 = p2.weight;
    let split = hue_shortest_diff_deg(h1, h2);
    let strong_secondary = w2 >= w1 * cfg.secondary_min_weight_ratio && w2 > 1e-9;

    let candidate = if strong_secondary && split >= cfg.split_hue_deg {
        let warm_cool_bridge =
            (is_warm(h1) && (is_blueish(h2) || is_violetish(h2)))
                || (is_warm(h2) && (is_blueish(h1) || is_violetish(h1)));
        if warm_cool_bridge {
            AccentSlot::Magenta
        } else {
            classify_hue_to_slot(h1)
        }
    } else {
        classify_hue_to_slot(h1)
    };

    if let Some(curr) = current_slot {
        if curr != candidate {
            let bin_deg = cfg.hist_bin_deg.clamp(5.0, 60.0);
            let mut curr_weight = 0.0_f64;
            let mut cand_weight = 0.0_f64;
            for (i, &bw) in hist.iter().enumerate() {
                if bw <= 1e-9 {
                    continue;
                }
                let h = norm_hue(i as f64 * bin_deg + bin_deg * 0.5);
                let slot = classify_hue_to_slot(h);
                if slot == curr {
                    curr_weight += bw;
                }
                if slot == candidate {
                    cand_weight += bw;
                }
            }
            if candidate == AccentSlot::Magenta {
                cand_weight = cand_weight.max((w1 + w2) * 0.75);
            }
            let ratio = cfg.hysteresis_ratio.max(1.0);
            if cand_weight < curr_weight * ratio {
                return curr;
            }
        }
    }

    candidate
}

/// Pick palette slot from two hue peaks with hysteresis support against the current slot.
#[allow(dead_code)]
pub fn resolve_accent_slot(
    cfg: &AccentModeConfig,
    a: HuePeak,
    b: HuePeak,
    current_slot: Option<AccentSlot>,
) -> AccentSlot {
    let (p1, p2) = if a.weight >= b.weight { (a, b) } else { (b, a) };
    let h1 = p1.hue_deg;
    let h2 = p2.hue_deg;
    let w1 = p1.weight;
    let w2 = p2.weight;
    let split = hue_shortest_diff_deg(h1, h2);
    let strong_secondary = w2 >= w1 * cfg.secondary_min_weight_ratio && w2 > 1e-9;

    let candidate = if strong_secondary && split >= cfg.split_hue_deg {
        let warm_cool_bridge =
            (is_warm(h1) && (is_blueish(h2) || is_violetish(h2)))
                || (is_warm(h2) && (is_blueish(h1) || is_violetish(h1)));
        if warm_cool_bridge {
            AccentSlot::Magenta
        } else {
            classify_hue_to_slot(h1)
        }
    } else {
        classify_hue_to_slot(h1)
    };

    if let Some(curr) = current_slot {
        if curr != candidate {
            let ratio = cfg.hysteresis_ratio.max(1.0);
            let curr_w = if classify_hue_to_slot(h1) == curr {
                w1
            } else if classify_hue_to_slot(h2) == curr {
                w2
            } else {
                0.0
            };
            let cand_w = if candidate == classify_hue_to_slot(h1) {
                w1
            } else if candidate == classify_hue_to_slot(h2) {
                w2
            } else {
                (w1 + w2) * 0.5
            };
            if cand_w < curr_w * ratio {
                return curr;
            }
        }
    }

    candidate
}

pub fn accent_choice_to_rgb_bri(cfg: &AccentModeConfig, slot: AccentSlot) -> ([u64; 3], u64) {
    let p = preset(cfg, slot);
    let bri = p.target_brightness().clamp(1, 255);
    let rgb = p.rgb();
    let r = rgb[0].clamp(0, 255);
    let g = rgb[1].clamp(0, 255);
    let b = rgb[2].clamp(0, 255);
    ([r, g, b], bri)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AccentModeConfig {
        AccentModeConfig::default()
    }

    #[test]
    fn red_blue_uses_magenta() {
        let c = cfg();
        let slot = resolve_accent_slot(
            &c,
            HuePeak {
                hue_deg: 5.0,
                weight: 10.0,
            },
            HuePeak {
                hue_deg: 220.0,
                weight: 8.0,
            },
            None,
        );
        assert_eq!(slot, AccentSlot::Magenta);
    }

    #[test]
    fn warm_only_uses_warm() {
        let c = cfg();
        let slot = resolve_accent_slot(
            &c,
            HuePeak {
                hue_deg: 35.0,
                weight: 10.0,
            },
            HuePeak {
                hue_deg: 55.0,
                weight: 2.0,
            },
            None,
        );
        assert_eq!(slot, AccentSlot::Warm);
    }

    #[test]
    fn green_dominant_green() {
        let c = cfg();
        let slot = resolve_accent_slot(
            &c,
            HuePeak {
                hue_deg: 120.0,
                weight: 10.0,
            },
            HuePeak {
                hue_deg: 125.0,
                weight: 1.0,
            },
            None,
        );
        assert_eq!(slot, AccentSlot::Green);
    }

    #[test]
    fn hysteresis_preserves_current_slot() {
        let c = cfg(); // default hysteresis_ratio = 1.25
        // Cyan is slightly ahead (11.0 vs 10.0), but within 1.25 margin of Green -> stays Green
        let slot = resolve_accent_slot(
            &c,
            HuePeak {
                hue_deg: 180.0,
                weight: 11.0,
            },
            HuePeak {
                hue_deg: 120.0,
                weight: 10.0,
            },
            Some(AccentSlot::Green),
        );
        assert_eq!(slot, AccentSlot::Green);

        // Cyan clearly dominates (13.0 >= 10.0 * 1.25) -> switches to Cyan
        let slot = resolve_accent_slot(
            &c,
            HuePeak {
                hue_deg: 180.0,
                weight: 13.0,
            },
            HuePeak {
                hue_deg: 120.0,
                weight: 10.0,
            },
            Some(AccentSlot::Green),
        );
        assert_eq!(slot, AccentSlot::Cyan);
    }

    #[test]
    fn green_and_warm_chooses_dominant_not_magenta() {
        let c = cfg();
        // Green dominant, Warm secondary -> MUST be Green, never Magenta!
        let slot = resolve_accent_slot(
            &c,
            HuePeak {
                hue_deg: 120.0,
                weight: 10.0,
            },
            HuePeak {
                hue_deg: 30.0,
                weight: 5.0,
            },
            None,
        );
        assert_eq!(slot, AccentSlot::Green);

        // Warm dominant, Green secondary -> MUST be Warm, never Magenta!
        let slot = resolve_accent_slot(
            &c,
            HuePeak {
                hue_deg: 30.0,
                weight: 10.0,
            },
            HuePeak {
                hue_deg: 120.0,
                weight: 5.0,
            },
            None,
        );
        assert_eq!(slot, AccentSlot::Warm);
    }
}
