use crate::config::{HslLutPoint, HueBrightnessLutPoint};

const WARM_R: f64 = 1.0;
const WARM_G: f64 = 0.82;
const WARM_B: f64 = 0.55;

pub struct PipelineParams<'a> {
    pub max_brightness: u64,
    pub gamma: f64,
    pub gamma_white: f64,
    pub white_chroma_max: f64,
    pub hsl_lut: &'a [HslLutPoint],
    pub hue_brightness_lut: &'a [HueBrightnessLutPoint],
    pub yellow_strength: f64,
}

pub fn apply_yellow_tint_norm_rgb(
    r: f64,
    g: f64,
    b: f64,
    yellow_strength: f64,
) -> (f64, f64, f64) {
    let warm = (WARM_R, WARM_G, WARM_B);
    mix3((r, g, b), warm, yellow_strength)
}

fn mix3(a: (f64, f64, f64), b: (f64, f64, f64), t: f64) -> (f64, f64, f64) {
    let t = t.clamp(0.0, 1.0);
    (
        a.0 * (1.0 - t) + b.0 * t,
        a.1 * (1.0 - t) + b.1 * t,
        a.2 * (1.0 - t) + b.2 * t,
    )
}

/// Brightest channel must reach this (normalized) to count as UI white, so dark gray is not white.
const NEAR_WHITE_MIN_MAX_CHANNEL: f64 = 0.92;

fn is_near_white(r: f64, g: f64, b: f64, max_chroma: f64) -> bool {
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    mx >= NEAR_WHITE_MIN_MAX_CHANNEL && (mx - mn) <= max_chroma
}

/// Per-channel response. `gamma > 1` applies `v^gamma` (darker mids, punchier). `gamma == 1` linear.
/// `0 < gamma < 1` lifts shadows (softer). Matches common UI intuition, not sRGB decode `v^(1/gamma)`.
fn apply_gamma_channel(v: f64, gamma: f64) -> f64 {
    if gamma <= 0.0 || (gamma - 1.0).abs() < 1e-9 {
        return v.clamp(0.0, 1.0);
    }
    v.clamp(0.0, 1.0).powf(gamma)
}

pub(crate) fn rgb_to_hsl(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let l = (max + min) / 2.0;

    if d < 1e-9 {
        return (0.0, 0.0, l);
    }

    let s = if l < 0.5 {
        d / (max + min)
    } else {
        d / (2.0 - max - min)
    };

    let h = if (max - r).abs() < 1e-9 {
        ((g - b) / d).rem_euclid(6.0) * 60.0
    } else if (max - g).abs() < 1e-9 {
        60.0 * (((b - r) / d) + 2.0)
    } else {
        60.0 * (((r - g) / d) + 4.0)
    };

    (h, s, l)
}

/// Linear RGB 0..1 to HSV. Hue degrees 0..360, S and V in 0..1 (HA `hs_color` uses S as 0..100).
pub(crate) fn rgb_to_hsv(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let v = max;
    if max < 1e-12 {
        return (0.0, 0.0, 0.0);
    }
    let s = d / max;
    let h = if d < 1e-12 {
        0.0
    } else if (max - r).abs() < 1e-9 {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if (max - g).abs() < 1e-9 {
        60.0 * (((b - r) / d) + 2.0)
    } else {
        60.0 * (((r - g) / d) + 4.0)
    };
    (h.rem_euclid(360.0), s.clamp(0.0, 1.0), v.clamp(0.0, 1.0))
}

/// Hue and saturation % for HA from scaled RGB (e.g. 0..max_b): chromaticity via max-normalize.
pub(crate) fn chromaticity_hs_ha(tr: f64, tg: f64, tb: f64) -> (f64, f64) {
    let m = tr.max(tg).max(tb);
    if m < 1e-9 {
        return (0.0, 0.0);
    }
    let (h, s, _) = rgb_to_hsv(tr / m, tg / m, tb / m);
    (h.rem_euclid(360.0), (s * 100.0).clamp(0.0, 100.0))
}

pub(crate) fn smooth_alpha(dt_secs: f64, tau_ms: f64) -> f64 {
    if tau_ms <= 0.0 || !tau_ms.is_finite() {
        return 1.0;
    }
    let tau_s = (tau_ms * 1e-3).max(1e-12);
    (1.0_f64 - (-dt_secs / tau_s).exp()).clamp(0.0, 1.0)
}

pub(crate) fn smooth_toward(prev: f64, target: f64, dt_secs: f64, tau_ms: f64) -> f64 {
    if tau_ms <= 0.0 || !tau_ms.is_finite() {
        return target;
    }
    let a = smooth_alpha(dt_secs, tau_ms);
    prev + a * (target - prev)
}

#[allow(dead_code)]
pub(crate) fn hue_shortest_delta_deg(from_h: f64, to_h: f64) -> f64 {
    let mut d = (to_h - from_h).rem_euclid(360.0);
    if d > 180.0 {
        d -= 360.0;
    }
    d
}

/// Advance from `current` toward `target` clamped to at most `max_delta` per-component change in 3D RGB space.
pub(crate) fn slew_limit_rgb(current: [f64; 3], target: [f64; 3], max_delta: f64) -> [f64; 3] {
    if max_delta <= 0.0 || !max_delta.is_finite() {
        return target;
    }
    let dr = target[0] - current[0];
    let dg = target[1] - current[1];
    let db = target[2] - current[2];
    let dist = dr.abs().max(dg.abs()).max(db.abs());
    if dist <= max_delta || dist <= 1e-12 {
        target
    } else {
        let ratio = max_delta / dist;
        [
            current[0] + dr * ratio,
            current[1] + dg * ratio,
            current[2] + db * ratio,
        ]
    }
}

fn hue_to_rgb(p: f64, q: f64, mut t: f64) -> f64 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 0.5 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

pub(crate) fn hsl_to_rgb(h_deg: f64, s: f64, l: f64) -> (f64, f64, f64) {
    let h = (h_deg / 360.0).rem_euclid(1.0);
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);

    if s < 1e-9 {
        return (l, l, l);
    }

    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let r = hue_to_rgb(p, q, h + 1.0 / 3.0);
    let g = hue_to_rgb(p, q, h);
    let b = hue_to_rgb(p, q, h - 1.0 / 3.0);
    (r, g, b)
}

fn lut_sample(brightness_key: f64, lut: &[HslLutPoint]) -> (f64, f64, f64) {
    if lut.is_empty() {
        return (0.0, 1.0, 1.0);
    }
    let k = brightness_key.clamp(0.0, 1.0);
    if lut.len() == 1 {
        let p = &lut[0];
        return (p.hue_shift_deg, p.saturation_mul, p.lightness_mul);
    }
    if k <= lut[0].brightness {
        let p = &lut[0];
        return (p.hue_shift_deg, p.saturation_mul, p.lightness_mul);
    }
    let last = lut.len() - 1;
    if k >= lut[last].brightness {
        let p = &lut[last];
        return (p.hue_shift_deg, p.saturation_mul, p.lightness_mul);
    }
    for i in 0..last {
        let a = &lut[i];
        let b = &lut[i + 1];
        if k <= b.brightness {
            let t = if (b.brightness - a.brightness).abs() < 1e-9 {
                0.0
            } else {
                (k - a.brightness) / (b.brightness - a.brightness)
            };
            let hue = a.hue_shift_deg + t * (b.hue_shift_deg - a.hue_shift_deg);
            let sat = a.saturation_mul + t * (b.saturation_mul - a.saturation_mul);
            let lig = a.lightness_mul + t * (b.lightness_mul - a.lightness_mul);
            return (hue, sat, lig);
        }
    }
    let p = &lut[last];
    (p.hue_shift_deg, p.saturation_mul, p.lightness_mul)
}

/// Piecewise linear interpolation over circular hue space [0, 360) deg.
pub fn sample_hue_brightness_lut(hue_deg: f64, lut: &[HueBrightnessLutPoint]) -> f64 {
    if lut.is_empty() {
        return 1.0;
    }
    if lut.len() == 1 {
        return lut[0].brightness_mul;
    }

    let h = hue_deg.rem_euclid(360.0);

    let mut pts: Vec<(f64, f64)> = lut
        .iter()
        .map(|p| (p.hue.rem_euclid(360.0), p.brightness_mul))
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9);

    if pts.len() == 1 {
        return pts[0].1;
    }

    let n = pts.len();
    if h >= pts[0].0 && h <= pts[n - 1].0 {
        for i in 0..n - 1 {
            let (h0, m0) = pts[i];
            let (h1, m1) = pts[i + 1];
            if h >= h0 && h <= h1 {
                let span = h1 - h0;
                if span < 1e-9 {
                    return m0;
                }
                let t = (h - h0) / span;
                return m0 + t * (m1 - m0);
            }
        }
    }

    let (h_last, m_last) = pts[n - 1];
    let (h_first, m_first) = pts[0];
    let wrap_span = (360.0 - h_last) + h_first;
    if wrap_span < 1e-9 {
        return m_first;
    }

    let dist = if h >= h_last {
        h - h_last
    } else {
        (360.0 - h_last) + h
    };
    let t = (dist / wrap_span).clamp(0.0, 1.0);
    m_last + t * (m_first - m_last)
}

/// `rgb` inputs are linear 0..255 averages (as f64). Returns RGB 0..255 and HA brightness (max channel).
pub fn process_rgb(rgb: (f64, f64, f64), p: &PipelineParams<'_>) -> (u64, u64, u64, u64) {
    let max_b = p.max_brightness.clamp(1, 255) as f64;
    let scale_out = max_b / 255.0;

    let mut r = (rgb.0 / 255.0).clamp(0.0, 1.0);
    let mut g = (rgb.1 / 255.0).clamp(0.0, 1.0);
    let mut b = (rgb.2 / 255.0).clamp(0.0, 1.0);

    (r, g, b) = apply_yellow_tint_norm_rgb(r, g, b, p.yellow_strength);

    let white = is_near_white(r, g, b, p.white_chroma_max);
    let g_use = if white { p.gamma_white } else { p.gamma };

    r = apply_gamma_channel(r, g_use);
    g = apply_gamma_channel(g, g_use);
    b = apply_gamma_channel(b, g_use);

    if !white && !p.hsl_lut.is_empty() {
        let key = r.max(g).max(b).clamp(0.0, 1.0);
        let (hue_shift, sat_mul, light_mul) = lut_sample(key, p.hsl_lut);
        let (h, s, l) = rgb_to_hsl(r, g, b);
        let h2 = (h + hue_shift).rem_euclid(360.0);
        let s2 = (s * sat_mul).clamp(0.0, 1.0);
        let l2 = (l * light_mul).clamp(0.0, 1.0);
        (r, g, b) = hsl_to_rgb(h2, s2, l2);
    }

    if !p.hue_brightness_lut.is_empty() {
        let (h, s, _) = rgb_to_hsl(r, g, b);
        let raw_mul = sample_hue_brightness_lut(h, p.hue_brightness_lut);
        // Blend with saturation so neutral grays/whites remain unshifted
        let eff_mul = 1.0 + s * (raw_mul - 1.0);
        let mx = r.max(g).max(b);
        if mx > 1e-9 {
            let target_mx = (mx * eff_mul).clamp(0.0, 1.0);
            let factor = target_mx / mx;
            r *= factor;
            g *= factor;
            b *= factor;
        }
    }

    r = (r * scale_out * 255.0).clamp(0.0, max_b);
    g = (g * scale_out * 255.0).clamp(0.0, max_b);
    b = (b * scale_out * 255.0).clamp(0.0, max_b);

    let br = r.max(g).max(b).round() as u64;
    let rr = r.round() as u64;
    let gr = g.round() as u64;
    let brgb = b.round() as u64;
    (rr, gr, brgb, br)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HueBrightnessLutPoint;

    #[test]
    fn test_sample_hue_brightness_lut_empty_and_single() {
        assert_eq!(sample_hue_brightness_lut(120.0, &[]), 1.0);

        let single = vec![HueBrightnessLutPoint {
            hue: 90.0,
            brightness_mul: 0.7,
            color: None,
        }];
        assert_eq!(sample_hue_brightness_lut(0.0, &single), 0.7);
        assert_eq!(sample_hue_brightness_lut(90.0, &single), 0.7);
    }

    #[test]
    fn test_sample_hue_brightness_lut_interpolation() {
        let lut = vec![
            HueBrightnessLutPoint {
                hue: 60.0,
                brightness_mul: 0.8,
                color: None,
            },
            HueBrightnessLutPoint {
                hue: 120.0,
                brightness_mul: 0.5,
                color: None,
            },
        ];
        assert!((sample_hue_brightness_lut(60.0, &lut) - 0.8).abs() < 1e-6);
        assert!((sample_hue_brightness_lut(120.0, &lut) - 0.5).abs() < 1e-6);
        assert!((sample_hue_brightness_lut(90.0, &lut) - 0.65).abs() < 1e-6);
        assert!((sample_hue_brightness_lut(0.0, &lut) - 0.74).abs() < 1e-6);
    }

    #[test]
    fn test_sample_hue_brightness_lut_with_0_and_360() {
        let lut = vec![
            HueBrightnessLutPoint {
                hue: 0.0,
                brightness_mul: 1.0,
                color: None,
            },
            HueBrightnessLutPoint {
                hue: 120.0,
                brightness_mul: 0.5,
                color: None,
            },
            HueBrightnessLutPoint {
                hue: 360.0,
                brightness_mul: 1.0,
                color: None,
            },
        ];
        assert!((sample_hue_brightness_lut(0.0, &lut) - 1.0).abs() < 1e-6);
        assert!((sample_hue_brightness_lut(60.0, &lut) - 0.75).abs() < 1e-6);
        assert!((sample_hue_brightness_lut(120.0, &lut) - 0.5).abs() < 1e-6);
        assert!((sample_hue_brightness_lut(240.0, &lut) - 0.75).abs() < 1e-6);
        assert!((sample_hue_brightness_lut(360.0, &lut) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_slew_limit_rgb() {
        let cur = [0.0, 100.0, 255.0];
        let target = [255.0, 72.0, 18.0];
        // dr = 255, dg = -28, db = -237 -> max dist = 255
        let step = slew_limit_rgb(cur, target, 5.0);
        assert!((step[0] - 5.0).abs() < 1e-4);
        assert!((step[1] - (100.0 - 28.0 * (5.0 / 255.0))).abs() < 1e-4);
        assert!((step[2] - (255.0 - 237.0 * (5.0 / 255.0))).abs() < 1e-4);

        // Within limit snaps directly to target
        let snap = slew_limit_rgb(cur, target, 300.0);
        assert_eq!(snap, target);
    }
}
