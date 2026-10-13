# Agents Log

## 2026-10-13 01:51

Fixed jerky and slow remote transitions in `light_interpolator`: replaced step-counter loop (`blocking=False`) with wall-clock `time.monotonic()` progression, `blocking=True` socket backpressure, and quantized state deduplication `(round(h_eff), round(s_eff, 1), bri)` to prevent LocalTuya TCP queue buildup and redundant state-change automation triggers. Adjusted `transition_duration_secs` from `120.0`s (where Tuya's integer degree quantization `round(hs[0])` limited movement to ~1 step/s) to `8.0`s for continuous ~15-20 Hz stepping.

## 2026-10-13 01:40

Fixed permanent lock on Cyan in Night Mode: filtered out near-black dark UI backgrounds (`max_c < 0.16` and low RGB chroma) whose tiny cool-gray delta caused HSL saturation ($S = \Delta / 2L$) to explode at $H \approx 210^\circ..225^\circ$, balanced hue-to-slot sectors (narrowing `Cyan` to $165^\circ..215^\circ$, assigning $215^\circ..295^\circ$ to `Violet` and $295^\circ..345^\circ$ to `Magenta`), normalized `accent_density` scaling, and removed the 30s heartbeat that restarted active Home Assistant interpolations. Rebuilt and deployed release binary to Nextcloud.

## 2026-10-10 02:55

Scaled interpolation duration proportionally to the 2D chromatic distance from the currently assigned color, making `transition_duration_secs` represent the maximum duration for the largest possible chromatic jump (diameter = 2.0). Implemented `light_interpolator.stop` service in Home Assistant and registered Windows console control handlers (`SetConsoleCtrlHandler`) in `hass-light-sync` to halt interpolation immediately upon program exit or Ctrl+C. Rebuilt release binary and updated configuration in Nextcloud.

## 2026-10-10 02:25

Implemented and verified the `light_interpolator` custom component in Home Assistant (executing 2D Cartesian chord interpolation across the chromaticity disk and real-time radial brightness correction at 35ms intervals directly over LocalTuya). Integrated the service dispatch into `hass-light-sync` with `use_light_interpolator` and `transition_duration_secs`, eliminating high-frequency network streaming from the desktop while guaranteeing smooth transitions without passing through white CCT LEDs or unintended hue sweeps. Built release binary and deployed to Nextcloud.

## 2026-10-10 01:52

Added linear slew rate limiting (`max_delta_per_iter` and `max_delta_per_sec`) to clamp maximum RGB color difference per iteration, guaranteeing a steady constant-speed transition without abrupt initial jumps. Rebuilt release binary and updated configuration in Nextcloud.

## 2026-10-10 01:48

Implemented 3D Cartesian RGB space interpolation to transition between colors directly across the color space without 1D hue circle rotation, avoiding unwanted yellow and green sweeps. Enforced a minimum saturation floor (35%) preventing Tuya white CCT LED activation, eliminated the warm-green bridge and lightness penalty that caused green avoidance, and unified brightness control under hue LUT and base brightness with synchronized transition timing. Updated schema to clean RGB preset arrays, made DXGI display acquisition gracefully retry on lock screen/sleep, and deployed updated release binary.

## 2026-10-10 01:21

Fixed causes of flicker and abrupt jumps in Night Mode: retained current accent slot during weak and small element frames instead of resetting to Warm dim, implemented full-histogram hysteresis across all palette slots, streamed every discrete step without stalling during transitions, and balanced color and brightness smoothing time constants. Deployed updated binary to Nextcloud.

## 2026-10-10 01:05

Identified root causes of jerky transitions: Tuya Wi-Fi bulbs lack hardware transition support in firmware, and previous code throttled post transmission by `transition_dur`, dropping intermediate software interpolation steps. Removed transmission throttling, added deadband change detection with 3s heartbeat to eliminate static network spam, and paced loop to steady 80ms cadence. Built and deployed updated binary to Nextcloud.

## 2026-10-10 00:52

Separated `settings.json` into explicit `night_mode` and `standard_mode` sections with `mode` selector and `brightness_multiplier` control. Added accent switching hysteresis, histogram temporal smoothing, small element weight filtering, and synchronized hue/brightness smoothing. Built and deployed updated binary to Nextcloud with full unit test coverage.

## 2026-10-10 00:40

Added JSON comment stripping preprocessor (`//` and `/* */`) to preserve comments while parsing `settings.json`. Added inline comments and descriptive `"color"` fields to `hue_brightness_lut` in both repository and Nextcloud configs. Replaced release binary in Nextcloud.

## 2026-10-10 00:32

Added live hot-reloading of `settings.json` during execution polling every 500ms when file modification time changes. Generated and deployed a 6-slice circular LUT (0.5 for red-orange at 0 deg to 1.0 for blue at 240 deg) to both repository and Nextcloud installations.

## 2026-10-10 00:27

Deployed newly built release binary to `Nextcloud/Strumenti/Programmi Windows/Home Assistant Light Sync/` with automated `.bak` backups. Preserved existing Home Assistant token and server configuration while adding `hue_brightness_lut` to `settings.json`.

## 2026-10-10 00:08

Added `hue_brightness_lut` configuration and piecewise circular interpolation to calibrate output brightness based on hue. Integrated into both standard `process_rgb` (with saturation weighting) and `accent_mode` target brightness before exponential smoothing. Verified with new and existing unit tests and release cargo check.
