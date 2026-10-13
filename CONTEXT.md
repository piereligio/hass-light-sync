# hass-light-sync - agent context

## Purpose

Rust CLI that captures the primary display (via `captrs`) and drives one Home Assistant light over the REST API. Reads `settings.json` from the current working directory (live-reloaded).

- `mode: "standard"`: averaged, smoothed screen colour, POSTed to `light/turn_on` as `rgb_color` + `brightness`.
- `mode: "night"` with `use_light_interpolator: true` (default): a hue histogram picks a palette slot; the app POSTs `light_interpolator/set_target` only when the target hue/saturation or brightness changes (plus a 60 s heartbeat) and `light_interpolator/stop` on exit. The fade itself runs inside Home Assistant.
- `mode: "night"` with `use_light_interpolator: false`: the app fades in RGB space itself and streams `light/turn_on` with `hs_color` + `brightness`.

All requests have a timeout; failures are retried after a short backoff and only HTTP 401 terminates the app.

## Home Assistant side

`custom_components/light_interpolator/` is the source of the component deployed in the HA config directory. `set_target` fades one light along the HS disk: straight chord between the colours, bent around the `min_saturation_floor` circle, travelled at constant colour speed with a short ease in and out; base brightness is faded on the same clock and multiplied by the circular hue LUT. Each step is a blocking `light.turn_on`, so the bulb's reply time bounds the step rate. Transitions start from the last emitted colour, or from the entity state when something else changed the light. The app and the component must agree on the brightness formula `base * (white_multiplier + S * (lut(H) - white_multiplier))`.

## Layout

| Path | Role |
|------|------|
| `Cargo.toml` | Single crate `hass-light-sync`, edition 2021 |
| `src/main.rs` | Entry, capture loop, HTTP client |
| `src/config.rs` | `settings.json` schema and defaults |
| `src/color_pipeline.rs` | Gamma, LUTs, smoothing, HS conversion |
| `src/accent_map.rs` | Hue histogram and palette slot selection (night mode) |
| `src/flux_warmth.rs` | f.lux warmth read for the yellow tint |
| `custom_components/light_interpolator/` | Home Assistant component (deployed by hand) |
| `settings.json` | Example config (ship with release zips) |
| `.github/workflows/release.yml` | Tagged `v*.*.*` builds Linux / Windows / macOS |

## Build tool

**Cargo only.** No workspace members, no extra build scripts in repo.

Local release binary:

```bash
cargo build --release
```

Artifacts: `target/release/hass-light-sync` (Unix) or `target/release/hass-light-sync.exe` (Windows). Run with `settings.json` beside the binary or from a directory that contains it (app uses relative `settings.json`).

Optional checks (not wired in CI here): `cargo clippy`, `cargo test`.

## CI vs local (from workflow)

- **Linux (Ubuntu):** `sudo apt-get install -y libxrandr-dev` before build (X11 for capture stack).
- **macOS:** `brew install --cask xquartz` before build.
- **Windows:** stock `windows-latest`, `cargo build --release --verbose`.

Releases trigger on git tags matching `v*.*.*`. Zips include the binary + `settings.json`.

## Related Rust build notes (other Piereligio repo)

`AC-Zero-Launcher/agents_memory_bank.md` documents a heavier Windows setup: `.cargo/config.toml` with shared `target-dir`, MSVC `+crt-static`, and invoking `cargo` from a VS Developer environment or a script that runs `vcvars64.bat`. **This repo has none of that** unless you add it. If you need a fully static MSVC binary or a custom `target-dir`, copy that pattern from there. Optional Linux cross-compile to `x86_64-pc-windows-gnu` with MinGW is also described there; not the default here.

## Configuration (summary)

JSON fields: base capture + HA (`api_endpoint`, `light_entity_name`, `token`, `grab_interval`, `send_interval_ms`, `skip_pixels`, `smoothing_tau_ms`, `transition`, `monitor_id`), color pipeline (`max_brightness`, `gamma`, `gamma_white`, `white_chroma_max`, `hsl_lut`, `hue_brightness_lut`), `yellow_tint`, `flux_poll_interval_ms`, `accent_mode`. Full example: `settings.json` in repo; legacy snippet in `README.md` may omit newer keys (defaults apply).

## Dependencies (why they matter)

- **captrs** - screen capture; platform deps (X11 on Linux, `dxgcap` + `winapi` on Windows).
- **ureq** - blocking HTTP + JSON POST to HA. Default features off: `http://` needs no TLS crate. For `https://`, extend `ureq` features (e.g. `rustls`, `webpki-roots`; see ureq docs).
- **serde / serde_json** - `settings.json` and request body.
- **console** - pinned **0.14.x** (`winapi`). **Do not** bump to 0.15+ without checking: newer `console` pulls `windows-sys`, which breaks `x86_64-pc-windows-gnu` builds when MinGW `dlltool.exe` is missing.

## Windows GNU (`x86_64-pc-windows-gnu`)

Prefer this dependency set over async `reqwest` + `tokio` + modern `console` so `cargo build` does not pull `windows-sys` (avoids `dlltool.exe` requirement). Alternative fixes: install full MinGW-w64, or use `x86_64-pc-windows-msvc` + Visual Studio C++ build tools.

## Agent preferences (from dev notes)

Source code and comments in **English**. This project is unrelated to Android/Wine testing described in the same memory bank file; ignore device tables unless the task explicitly touches that stack.
