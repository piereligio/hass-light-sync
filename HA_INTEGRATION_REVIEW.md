# Home Assistant integration review (2026-10-13)

Read-only review of how `hass-light-sync` talks to the Home Assistant instance in the
`homeassistant` iocage jail. Nothing was changed on the NAS.

**Status.** The sections below describe the state found on 2026-10-13. Since then, in this
repository: mismatches 1 to 5 are fixed, the app has timeouts, retry, heartbeat and one error
policy, and the component (now in `custom_components/light_interpolator/`, version 1.1.0) has a
schema and `services.yaml`. Still open: the component on the NAS is the old one until it is
copied over and Home Assistant restarted; the helper automation and recorder load; coordination
with other automations; `stop` on Ctrl+C for non-Windows builds; plain HTTP.

**Stepping.** The large, fast steps came from the clamped chord: between near-complementary
colours the hue swept about 70 degrees in 5% of the transition. See `CAVEATS.md`.

## End-to-end structure

```
hass-light-sync (Rust, Windows PC)
  │  HTTP (no TLS), Bearer long-lived token, blocking ureq, no timeout
  ▼
Home Assistant Core 2022.6.7 (Python 3.9, FreeBSD 12.2 jail, :8123)
  ├─ night mode + use_light_interpolator (current settings)
  │    POST /api/services/light_interpolator/set_target   (only when accent slot or brightness changes)
  │    POST /api/services/light_interpolator/stop         (on exit)
  │      └─ custom_components/light_interpolator/__init__.py (YAML setup: `light_interpolator:`)
  │           asyncio task per entity: linear chord on the HS disk, per step
  │           light.turn_on {hs_color, brightness} with blocking=True, deduplicated
  └─ every other mode
       POST /api/services/light/turn_on {rgb_color | hs_color, brightness, transition?}
       every send_interval_ms on change, plus a 3 s heartbeat
  ▼
LocalTuya 3.2.1 → light.luce_scrivania (supported_color_modes: hs only, integer hue)
  ▼
automation "Aggiorna helpers luce scrivania quando cambia stato" (fires on every state change:
  2x input_number.set_value, 1x input_text.set_value, 1x script) → recorder (2.5 GB DB, 365 days)
```

`set_target` payload sent by the app: `entity_id`, `target_h`, `target_s`, `base_brightness`,
`duration`, `max_duration` (same value), `step_ms` (hard-coded 35), `min_saturation_floor`, `hue_lut`.
The component reads all of them except `duration` (only a fallback) and additionally accepts
`white_multiplier` (default 0.45), which the app never sends.

## Mismatches (bugs)

1. **Hue LUT is flat 0.5 in interpolator mode.** `main.rs` sorts `hue_brightness_lut` with a stable
   sort keyed on `hue.rem_euclid(360)`, so the `360.0` point gets key 0 and lands right after `0.0`:
   `[[0,0.5],[360,0.5],[60,0.625],…,[300,0.75]]`. `send_ha_interpolator` sends the raw hues in that
   order. The component's `sample_hue_lut` scans consecutive pairs and returns on the first one with
   `h1 <= h <= h2`; the first pair is `(0, 360)`, which matches every hue and yields 0.5. Blue,
   meant to be 1.0, gets 0.5 like everything else. (Derived from reading both sides, not measured.)
   The app-side `sample_hue_brightness_lut` is correct (dedup + wrap), so only the HA path is affected.
   Without the `360` point the component still would not wrap: hues above the last point fall back to
   `points[0]`.

2. **Two different brightness formulas.** App path: `mul = 1 + s·(lut − 1)` (white stays at full
   base brightness). Component: `mul = white_mul + s·(lut − white_mul)` with `white_mul = 0.45`.
   Same settings therefore give different brightness depending on `use_light_interpolator`, and the
   comment in `settings.json` describes the app formula only. `white_multiplier` is not configurable
   from the app.

3. **Brightness is not interpolated.** The worker computes brightness from the new `base_brightness`
   at every step, so a change of base (weak/dim 48 → accent 255) jumps on the first step while only
   the colour fades.

4. **Stale start point.** `last_hs_cache` wins over the entity's real `hs_color` and is cleared only
   by `stop`. If the light is changed by anything else while the app runs (manual, the sleep
   automation setting 1%, scenes), the next transition starts from the cached colour and jumps.

5. **Yellow tint changes are not dispatched** in interpolator mode: a resend happens only when the
   accent slot or the brightness changes, so a moving f.lux warmth shifts the target hue without a
   new `set_target`.

## Fragilities

- **No resend after a failed `set_target`.** `last_dispatched_*` are updated even when the POST
  fails, and there is no heartbeat in interpolator mode. An HA restart (start-up here takes ~7
  minutes) leaves the light on the old colour until the next slot change. A 401 or 404 (component not
  loaded) is only printed.
- **No HTTP timeout** on `set_target` and `light.turn_on`; the capture loop blocks if HA stalls.
  Only the `stop` call has one (1.5 s).
- **Opposite error policies.** `send_ha_light` terminates the process (exit code 0) on any error,
  including a transient one; `send_ha_interpolator` ignores everything.
- **Token handling differs**: `.trim()` in the interpolator calls, not in `send_ha_light`.
- **Component has no input validation**: services are registered without a schema, `float()`/`int()`
  on bad input raise inside the handler, `hue_lut` is trusted to be sorted. `entity_id` defaults to
  the hard-coded `light.luce_scrivania`.
- **No `services.yaml`** (HA logs a warning); the component is not in this repository, is owned by
  `root` inside the jail, and was last modified 2026-10-13 01:50 with an HA restart at the same time.
- **Helper automation load.** Each interpolation step is a state change, so the automation above
  runs per step. Last three days of log: 83 "Already running" warnings and 25 template errors
  (`hs_color` is `None` when the light is off). Every step also writes several rows to a recorder
  DB that is 2.5 GB with `purge_keep_days: 365`.
- **No coordination with other automations** on the same light (sleep routine, button toggle): the
  app keeps overriding them while it runs.
- **Non-Windows builds never send `stop`** on Ctrl+C (handler is Windows only). On Windows `stop`
  can be sent twice (handler and loop exit); harmless.
- Plain HTTP with a long-lived token on the LAN; `ureq` is built without TLS.
- `CONTEXT.md` still describes the pre-interpolator app (single `main.rs`, `light.turn_on` only).
- Leftover automation "Auto Luce Scrivania PC" (webhook `update_desk_light`, no actions).

## Suggested improvements, in order

1. Fix the LUT: in the component, normalise (`hue % 360`, sort, dedup) and interpolate circularly;
   in the app, send the deduplicated, sorted points. Fixes mismatch 1.
2. Pick one brightness formula and make `white_multiplier` a setting sent by the app (or drop it).
3. Interpolate brightness in the worker (cache last emitted brightness next to HS).
4. Start from the entity's real state when it differs from the last value the worker emitted
   (compare with `round(h)`, since LocalTuya reports integer hue), instead of trusting the cache.
5. App resilience: timeout on every request (about 2 s), update `last_dispatched_*` only on HTTP 200,
   add a slow heartbeat (30–60 s) or retry, and use one error policy for both paths (log and retry;
   exit non-zero only on 401).
6. Add a `vol` schema and `services.yaml` to the component, and version it in this repo under
   `custom_components/light_interpolator/` with a short deploy note.
7. HA side: add a `for:` debounce (or a condition on `hs_color is not none`) to the helper
   automation and exclude `light.luce_scrivania` plus its helpers from `recorder`.
8. Optional: an `input_boolean` gate checked by the component so other automations can pause the sync.
9. Refresh `CONTEXT.md`.
