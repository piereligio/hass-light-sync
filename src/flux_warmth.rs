//! Best-effort f.lux warmth on Windows (registry via reg.exe). No winreg/windows-sys.

#[cfg(windows)]
pub fn read_flux_warmth() -> Option<f64> {
    use std::process::Command;

    const K_DAY: f64 = 6500.0;
    const K_NIGHT: f64 = 2700.0;

    fn kelvin_to_warmth(k: f64) -> f64 {
        if !k.is_finite() || k <= 0.0 {
            return 0.0;
        }
        ((K_DAY - k) / (K_DAY - K_NIGHT)).clamp(0.0, 1.0)
    }

    fn warmth_from_kelvin(k: f64) -> Option<f64> {
        if (1000.0..=20000.0).contains(&k) {
            Some(kelvin_to_warmth(k))
        } else {
            None
        }
    }

    fn query_key(key_path: &str, value_name: &str) -> Option<f64> {
        let out = Command::new("reg")
            .args(["query", key_path, "/v", value_name])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let mut it = line.split_whitespace();
            let name = it.next()?;
            if !name.eq_ignore_ascii_case(value_name) {
                continue;
            }
            let kind = it.next()?;
            if kind.eq_ignore_ascii_case("REG_DWORD") {
                let raw = it.next()?;
                let v = if let Some(h) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
                    u32::from_str_radix(h, 16).ok()?
                } else {
                    raw.parse::<u32>().ok()?
                };
                return warmth_from_kelvin(v as f64);
            }
            if kind.eq_ignore_ascii_case("REG_SZ") {
                let rest = it.collect::<Vec<_>>().join(" ");
                let s = rest.trim_matches('"');
                let k: f64 = s.parse().ok()?;
                return warmth_from_kelvin(k);
            }
        }
        None
    }

    let keys = [
        r"HKCU\Software\Michael Herf\Flux",
        r"HKCU\Software\Michael Herf\flux",
    ];
    let names = [
        "ct",
        "color",
        "ColorTemp",
        "temperature",
        "nighttemp",
        "daytemp",
        "Kelvin",
    ];

    for key in keys {
        for name in names {
            if let Some(w) = query_key(key, name) {
                return Some(w);
            }
        }
    }
    None
}

#[cfg(not(windows))]
pub fn read_flux_warmth() -> Option<f64> {
    None
}
