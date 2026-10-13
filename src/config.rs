use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    Night,
    Accent,
    Standard,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct StandardModeConfig {
    #[serde(default = "default_smoothing_tau")]
    pub smoothing_tau_ms: f64,
    #[serde(default = "default_one_f64")]
    pub gamma: f64,
    #[serde(default)]
    pub gamma_white: Option<f64>,
    #[serde(default = "default_white_chroma_max")]
    pub white_chroma_max: f64,
    #[serde(default)]
    pub hsl_lut: Vec<HslLutPoint>,
}

fn default_smoothing_tau() -> f64 {
    0.0
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Settings {
    pub api_endpoint: String,
    pub light_entity_name: String,
    pub token: String,
    pub grab_interval: i16,
    /// ms between HA posts and smoothing steps. None = same as `grab_interval`.
    #[serde(default)]
    pub send_interval_ms: Option<u64>,
    pub skip_pixels: i16,
    /// Exponential smoothing time constant (ms). `prev += (1-exp(-dt/τ))*(target-prev)`. 0 = instant.
    #[serde(default)]
    pub smoothing_tau_ms: f64,
    /// HA `light.turn_on` transition (seconds). When set, posts are spaced by at least this duration.
    #[serde(default)]
    pub transition: Option<f64>,
    #[serde(default)]
    pub monitor_id: i16,

    #[serde(default = "default_max_brightness")]
    pub max_brightness: u64,

    /// Explicit operating mode: "night" (or "accent") vs "standard". None = fallback to accent_mode.enabled.
    #[serde(default)]
    pub mode: Option<SyncMode>,

    /// When true (default), delegates 2D HS interpolation to HA's light_interpolator service in night mode.
    #[serde(default = "default_true")]
    pub use_light_interpolator: bool,

    /// Specific configuration for standard mode (average RGB sync).
    #[serde(default, alias = "standard_mode")]
    pub standard_mode: Option<StandardModeConfig>,

    #[serde(default = "default_one_f64")]
    pub gamma: f64,

    #[serde(default)]
    pub gamma_white: Option<f64>,

    #[serde(default = "default_white_chroma_max")]
    pub white_chroma_max: f64,

    #[serde(default)]
    pub hsl_lut: Vec<HslLutPoint>,

    #[serde(default)]
    pub hue_brightness_lut: Vec<HueBrightnessLutPoint>,

    #[serde(default)]
    pub yellow_tint: YellowTintConfig,

    #[serde(default = "default_flux_poll_ms")]
    pub flux_poll_interval_ms: u64,

    #[serde(default, alias = "night_mode")]
    pub accent_mode: AccentModeConfig,
}

impl Settings {
    pub fn is_night_mode(&self) -> bool {
        match self.mode {
            Some(SyncMode::Night) | Some(SyncMode::Accent) => true,
            Some(SyncMode::Standard) => false,
            None => self.accent_mode.enabled,
        }
    }

    pub fn get_gamma(&self) -> f64 {
        self.standard_mode
            .as_ref()
            .map(|s| s.gamma)
            .unwrap_or(self.gamma)
    }

    pub fn get_gamma_white(&self) -> f64 {
        self.standard_mode
            .as_ref()
            .and_then(|s| s.gamma_white)
            .or(self.gamma_white)
            .unwrap_or_else(|| self.get_gamma())
    }

    pub fn get_white_chroma_max(&self) -> f64 {
        self.standard_mode
            .as_ref()
            .map(|s| s.white_chroma_max)
            .unwrap_or(self.white_chroma_max)
    }

    pub fn get_standard_tau_ms(&self) -> f64 {
        self.standard_mode
            .as_ref()
            .map(|s| s.smoothing_tau_ms)
            .unwrap_or(self.smoothing_tau_ms)
    }

    pub fn get_hsl_lut(&self) -> &[HslLutPoint] {
        if let Some(ref std_cfg) = self.standard_mode {
            if !std_cfg.hsl_lut.is_empty() {
                return &std_cfg.hsl_lut;
            }
        }
        &self.hsl_lut
    }
}

fn default_max_brightness() -> u64 {
    255
}

fn default_one_f64() -> f64 {
    1.0
}

fn default_white_chroma_max() -> f64 {
    0.04
}

fn default_flux_poll_ms() -> u64 {
    3000
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HslLutPoint {
    /// Normalized output brightness key in 0..1 (matches max RGB after gamma).
    pub brightness: f64,
    #[serde(default)]
    pub hue_shift_deg: f64,
    #[serde(default = "default_one_f64")]
    pub saturation_mul: f64,
    #[serde(default = "default_one_f64")]
    pub lightness_mul: f64,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct HueBrightnessLutPoint {
    /// Hue in degrees (0.0 .. 360.0).
    pub hue: f64,
    /// Multiplier applied to brightness (e.g. 1.0 = unchanged, 0.7 = 70%, 1.2 = 120%).
    #[serde(default = "default_one_f64")]
    pub brightness_mul: f64,
    /// Optional color description or user note (e.g. "Rosso / Rosso-arancio").
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum YellowTintMode {
    Off,
    Manual,
    Flux,
}

impl Default for YellowTintMode {
    fn default() -> Self {
        YellowTintMode::Off
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct YellowTintConfig {
    pub mode: YellowTintMode,
    /// 0..1. Used for Manual mode, or as fallback when Flux mode cannot read warmth.
    pub manual_strength: f64,
}

impl Default for YellowTintConfig {
    fn default() -> Self {
        YellowTintConfig {
            mode: YellowTintMode::Off,
            manual_strength: 0.0,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(untagged)]
pub enum AccentOutput {
    Simple([u64; 3]),
    Detailed {
        rgb: [u64; 3],
        #[serde(default = "default_max_brightness")]
        target_brightness: u64,
    },
}

impl AccentOutput {
    pub fn rgb(&self) -> [u64; 3] {
        match self {
            AccentOutput::Simple(rgb) => *rgb,
            AccentOutput::Detailed { rgb, .. } => *rgb,
        }
    }

    pub fn target_brightness(&self) -> u64 {
        match self {
            AccentOutput::Simple(_) => 255,
            AccentOutput::Detailed { target_brightness, .. } => *target_brightness,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct AccentModeConfig {
    pub enabled: bool,
    /// Base brightness for night mode (1..255). None = fallback to top-level max_brightness.
    #[serde(default)]
    pub brightness: Option<u64>,
    #[serde(default = "default_one_f64")]
    pub brightness_multiplier: f64,
    /// Minimum saturation floor (0..100) to keep bulb strictly in RGB mode and prevent Tuya white CCT flash.
    #[serde(default = "default_min_saturation_floor")]
    pub min_saturation_floor: f64,
    pub min_saturation: f64,
    pub hist_bin_deg: f64,
    pub split_hue_deg: f64,
    pub secondary_min_weight_ratio: f64,
    pub weak_peak_ratio: f64,
    /// Minimum weight ratio (+25% by default) a candidate must beat the current slot by to switch.
    #[serde(default = "default_hysteresis_ratio")]
    pub hysteresis_ratio: f64,
    /// Temporal exponential smoothing factor (0.0 = instant, 0.6 = smooth) for the hue histogram.
    #[serde(default = "default_hist_smoothing")]
    pub hist_smoothing: f64,
    /// Minimum fraction of sampled pixels with colorful weight to avoid triggering on tiny icons.
    #[serde(default = "default_min_accent_weight")]
    pub min_accent_weight: f64,
    /// Weak histogram: if true, run full `process_rgb` (gray UI can go near white).
    /// If false, dim warm accent only.
    #[serde(default = "default_allow_neutral_on_weak")]
    pub allow_neutral_on_weak: bool,
    /// HA brightness (1..255) for weak path when `allow_neutral_on_weak` is false.
    #[serde(default = "default_weak_fallback_brightness")]
    pub weak_fallback_brightness: u64,
    /// Transition time constant (ms). Accepts transition_tau_ms or color_smoothing_tau_ms.
    #[serde(default, alias = "transition_tau_ms", alias = "smoothing_tau_ms")]
    pub color_smoothing_tau_ms: Option<f64>,
    /// Maximum color difference (0..255 RGB delta) allowed in a single iteration / step.
    /// Directly bounds the transition speed per frame (e.g. 3.0 .. 10.0).
    #[serde(default, alias = "max_step_per_iter", alias = "color_delta_limit")]
    pub max_delta_per_iter: Option<f64>,
    /// Maximum color difference allowed per second (converted to per-iteration via dt).
    #[serde(default, alias = "max_step_per_sec")]
    pub max_delta_per_sec: Option<f64>,
    /// Maximum duration in seconds for the light_interpolator transition in Home Assistant (scaled by chromatic jump).
    #[serde(default = "default_transition_duration_secs", alias = "duration", alias = "transition_seconds", alias = "max_transition_duration_secs", alias = "max_duration")]
    pub transition_duration_secs: f64,
    /// Lower bound in seconds for any light_interpolator transition, so short hops are not abrupt.
    #[serde(default = "default_transition_min_duration_secs")]
    pub transition_min_duration_secs: f64,
    /// Brightness multiplier the hue LUT blends toward as saturation goes to zero (white).
    #[serde(default = "default_white_multiplier")]
    pub white_multiplier: f64,
    /// LocalTuya bulbs only: data point of the bulb's gradient control (28 on standard Tuya lights).
    /// When set, light_interpolator sends intermediate steps there and the bulb fades between them.
    #[serde(default)]
    pub tuya_gradient_dp: Option<u64>,
    /// ms between steps when `tuya_gradient_dp` is set.
    #[serde(default = "default_tuya_gradient_step_ms")]
    pub tuya_gradient_step_ms: u64,
    /// HA master brightness smoothing: 0 or None = brightness follows color synchronously without lag.
    #[serde(default)]
    pub master_brightness_smoothing_tau_ms: Option<f64>,
    pub warm: AccentOutput,
    pub magenta: AccentOutput,
    pub green: AccentOutput,
    pub cyan: AccentOutput,
    pub violet: AccentOutput,
}

fn default_true() -> bool {
    true
}

fn default_transition_duration_secs() -> f64 {
    2.5
}

fn default_transition_min_duration_secs() -> f64 {
    1.5
}

fn default_tuya_gradient_step_ms() -> u64 {
    150
}

fn default_white_multiplier() -> f64 {
    0.45
}

fn default_min_saturation_floor() -> f64 {
    35.0
}

fn default_hysteresis_ratio() -> f64 {
    1.25
}

fn default_hist_smoothing() -> f64 {
    0.6
}

fn default_min_accent_weight() -> f64 {
    0.03
}

fn default_accent_min_sat() -> f64 {
    0.14
}

fn default_hist_bin_deg() -> f64 {
    20.0
}

fn default_split_hue_deg() -> f64 {
    92.0
}

fn default_secondary_ratio() -> f64 {
    0.22
}

fn default_weak_peak_ratio() -> f64 {
    0.065
}

fn default_allow_neutral_on_weak() -> bool {
    false
}

fn default_weak_fallback_brightness() -> u64 {
    48
}

fn default_warm_output() -> AccentOutput {
    AccentOutput::Simple([255, 72, 18])
}

fn default_magenta_output() -> AccentOutput {
    AccentOutput::Simple([215, 42, 125])
}

fn default_green_output() -> AccentOutput {
    AccentOutput::Simple([48, 205, 78])
}

fn default_cyan_output() -> AccentOutput {
    AccentOutput::Simple([0, 175, 195])
}

fn default_violet_output() -> AccentOutput {
    AccentOutput::Simple([105, 48, 175])
}

impl Default for AccentModeConfig {
    fn default() -> Self {
        AccentModeConfig {
            enabled: false,
            brightness: None,
            brightness_multiplier: 1.0,
            min_saturation_floor: default_min_saturation_floor(),
            min_saturation: default_accent_min_sat(),
            hist_bin_deg: default_hist_bin_deg(),
            split_hue_deg: default_split_hue_deg(),
            secondary_min_weight_ratio: default_secondary_ratio(),
            weak_peak_ratio: default_weak_peak_ratio(),
            hysteresis_ratio: default_hysteresis_ratio(),
            hist_smoothing: default_hist_smoothing(),
            min_accent_weight: default_min_accent_weight(),
            allow_neutral_on_weak: default_allow_neutral_on_weak(),
            weak_fallback_brightness: default_weak_fallback_brightness(),
            color_smoothing_tau_ms: None,
            max_delta_per_iter: None,
            max_delta_per_sec: None,
            transition_duration_secs: default_transition_duration_secs(),
            transition_min_duration_secs: default_transition_min_duration_secs(),
            white_multiplier: default_white_multiplier(),
            tuya_gradient_dp: None,
            tuya_gradient_step_ms: default_tuya_gradient_step_ms(),
            master_brightness_smoothing_tau_ms: None,
            warm: default_warm_output(),
            magenta: default_magenta_output(),
            green: default_green_output(),
            cyan: default_cyan_output(),
            violet: default_violet_output(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_and_aliases_deserialization() {
        let json = r#"{
            "api_endpoint": "http://localhost:8123",
            "light_entity_name": "light.desk",
            "token": "secret",
            "grab_interval": 50,
            "skip_pixels": 10,
            "mode": "night",
            "night_mode": {
                "brightness_multiplier": 0.8,
                "color_smoothing_tau_ms": 1200.0
            },
            "standard_mode": {
                "smoothing_tau_ms": 250.0,
                "gamma": 1.8
            }
        }"#;

        let cfg: Settings = serde_json::from_str(json).expect("valid settings json");
        assert!(cfg.is_night_mode());
        assert!(cfg.use_light_interpolator);
        assert_eq!(cfg.accent_mode.transition_duration_secs, 2.5);
        assert_eq!(cfg.accent_mode.brightness_multiplier, 0.8);
        assert_eq!(cfg.accent_mode.color_smoothing_tau_ms, Some(1200.0));
        assert_eq!(cfg.get_standard_tau_ms(), 250.0);
        assert_eq!(cfg.get_gamma(), 1.8);
    }
}

