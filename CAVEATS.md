# Project Caveats

## LocalTuya Hardware Quantization and Socket Pacing (`light.luce_scrivania`)

1. **Integer Hue Quantization (`0..360`)**:
   - In Home Assistant's `localtuya/light.py`, `async_turn_on` encodes the Tuya DPS color payload using `round(hs[0])` (integer degrees `0..360`).
   - Because hardware Hue steps are strictly 1 integer degree wide, any transition across $\Delta H$ degrees has at most $|\Delta H|$ discrete color steps.
   - If `transition_duration_secs` is set too high (e.g. `120.0` seconds), a $60^\circ$ transition steps only once per second ($1^\circ/\text{s}$), making individual $1^\circ$ hardware jumps visibly discrete ("jerky and slow"). Keep `transition_duration_secs` around `6.0 .. 12.0` seconds so hardware steps occur at ~15-25 Hz.

2. **Backpressure and State Deduplication in `light_interpolator`**:
   - Calling `hass.services.async_call("light", "turn_on", ..., blocking=False)` at 35ms intervals floods the LocalTuya TCP socket faster than the bulb's Wi-Fi MCU can ACK, building a backlog queue in Home Assistant's event loop and triggering state-change automations (`Aggiorna helpers luce scrivania`) dozens of times per second.
   - `light_interpolator` must use wall-clock progression (`time.monotonic()`), `blocking=True` backpressure, and state deduplication `(round(h_eff), round(s_eff, 1), bri)` so only actual hardware state changes are transmitted.

3. **Dark UI Background HSL Saturation Explosion**:
   - In standard HSL, $S = \frac{\max - \min}{\max + \min}$ divides by $2L$. On dark-themed desktops (`RGB ~ [18, 22, 28]`), tiny cool-gray differences ($B - R \approx 6..10$) produce $S_{\text{HSL}} \approx 0.20$ at $H \approx 215^\circ..225^\circ$, flooding the histogram with `Cyan` unless pixels are filtered by minimum brightness (`max_c >= 0.16`) and true RGB chroma (`max - min >= min_saturation`).

## `light_interpolator` path and pacing

1. **A straight chord clamped to the saturation floor is not uniform.** Between near-complementary colours the chord passes close to the centre of the HS disk; clamping saturation to `min_saturation_floor` there makes the hue sweep most of its range in a few percent of the transition (warm to cyan: about 70 degrees in 5% of the time). The component therefore follows the floor circle explicitly and paces by arc length.
2. **Step size is bounded by the bulb, not by `step_ms`.** Each step waits for LocalTuya (`blocking=True`), so steps per transition is roughly duration divided by the bulb's reply time. Short transitions mean large steps; `transition_min_duration_secs` is the lever.
3. **The hue LUT must be sent sorted and without the `360` duplicate of `0`.** A `(0, 360)` pair at the head of the list matches every hue in a linear segment scan and flattens the LUT.
4. **Every interpolation step is a state change of the light**, so state-triggered automations on it (`Aggiorna helpers luce scrivania quando cambia stato`) run per step and add load to the same event loop that paces the fade.
