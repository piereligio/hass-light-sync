"""Light Interpolator - Smooth 2D HS chromaticity disk transitions and brightness calibration."""
import asyncio
import logging
import math
import time

import voluptuous as vol

from homeassistant.helpers import config_validation as cv, entity_registry as er

_LOGGER = logging.getLogger(__name__)
DOMAIN = "light_interpolator"

DEFAULT_HUE_LUT = [
    (0.0, 0.6),    # Red
    (60.0, 0.45),  # Yellow
    (120.0, 0.75), # Green
    (180.0, 0.8),  # Cyan
    (240.0, 1.0),  # Blue
    (300.0, 0.75), # Magenta
]

# How long after the last emitted step the cached colour is trusted over the entity state
# (LocalTuya reports the new state with some delay).
STATE_SETTLE_SECS = 3.0
# Tolerances when comparing the entity state with the last emitted colour (integer hue on the bulb).
HUE_MATCH_DEG = 2.0
SAT_MATCH_PCT = 2.0
# Fraction of the transition spent accelerating, and again decelerating.
EASE_RAMP = 0.2
# A gradient data point write slower than this means the bulb ignores it (LocalTuya waits for a reply).
GRADIENT_STALL_SECS = 1.5
# Lowest value (of 1000) Tuya colour data accepts.
TUYA_MIN_VALUE = 10

SET_TARGET_SCHEMA = vol.Schema(
    {
        vol.Required("entity_id"): cv.entity_id,
        vol.Required("target_h"): vol.Coerce(float),
        vol.Required("target_s"): vol.All(vol.Coerce(float), vol.Clamp(min=0.0, max=100.0)),
        vol.Optional("base_brightness", default=50): vol.All(
            vol.Coerce(int), vol.Clamp(min=1, max=255)
        ),
        vol.Optional("max_duration"): vol.All(vol.Coerce(float), vol.Clamp(min=0.0, max=600.0)),
        # Legacy alias of max_duration
        vol.Optional("duration"): vol.All(vol.Coerce(float), vol.Clamp(min=0.0, max=600.0)),
        vol.Optional("min_duration", default=0.0): vol.All(
            vol.Coerce(float), vol.Clamp(min=0.0, max=600.0)
        ),
        vol.Optional("step_ms", default=45): vol.All(vol.Coerce(int), vol.Clamp(min=30, max=2000)),
        vol.Optional("min_saturation_floor", default=35.0): vol.All(
            vol.Coerce(float), vol.Clamp(min=0.0, max=100.0)
        ),
        vol.Optional("white_multiplier", default=0.45): vol.All(
            vol.Coerce(float), vol.Clamp(min=0.0, max=2.0)
        ),
        vol.Optional("hue_lut"): [vol.ExactSequence([vol.Coerce(float), vol.Coerce(float)])],
        # LocalTuya only: data point of the bulb's gradient control (28 on standard Tuya lights)
        vol.Optional("tuya_gradient_dp"): vol.Any(None, vol.All(vol.Coerce(int), vol.Range(min=1, max=255))),
    }
)

STOP_SCHEMA = vol.Schema({vol.Required("entity_id"): cv.entity_id})


def normalize_hue_lut(lut=None):
    """Return LUT points sorted by hue in [0, 360), without duplicates (360 folds onto 0)."""
    by_hue = {}
    for h, m in lut or DEFAULT_HUE_LUT:
        by_hue.setdefault(round(float(h) % 360.0, 6), float(m))
    return sorted(by_hue.items())


def sample_hue_lut(h_deg: float, points) -> float:
    """Circular linear interpolation over normalized LUT points."""
    if not points:
        return 1.0
    if len(points) == 1:
        return points[0][1]
    h = h_deg % 360.0
    if h < points[0][0]:
        h += 360.0
    ring = points + [(points[0][0] + 360.0, points[0][1])]
    for (h1, m1), (h2, m2) in zip(ring, ring[1:]):
        if h1 <= h <= h2:
            span = h2 - h1
            if span < 1e-9:
                return m1
            return m1 + (h - h1) / span * (m2 - m1)
    return points[0][1]


def brightness_multiplier(h: float, s_pct: float, white_mul: float, points) -> float:
    s_norm = max(0.0, min(1.0, s_pct / 100.0))
    # Radial blend: as S -> 0 (toward white), attenuates toward white_mul to compensate multi-channel glare
    return white_mul + s_norm * (sample_hue_lut(h, points) - white_mul)


def calculate_brightness(h, s_pct, base_b, white_mul=0.45, points=None) -> int:
    if points is None:
        points = normalize_hue_lut()
    return max(1, min(255, round(base_b * brightness_multiplier(h, s_pct, white_mul, points))))


def hue_distance(a: float, b: float) -> float:
    d = abs(a - b) % 360.0
    return min(d, 360.0 - d)


def _to_xy(h_deg: float, s_pct: float):
    r = s_pct / 100.0
    rad = math.radians(h_deg)
    return r * math.cos(rad), r * math.sin(rad)


def ease_in_out(t: float, ramp: float = EASE_RAMP) -> float:
    """Constant speed with a short acceleration and deceleration (peak speed 1 / (1 - ramp))."""
    if t <= 0.0:
        return 0.0
    if t >= 1.0:
        return 1.0
    if t < ramp:
        return t * t / (2.0 * ramp * (1.0 - ramp))
    if t > 1.0 - ramp:
        return 1.0 - (1.0 - t) ** 2 / (2.0 * ramp * (1.0 - ramp))
    return (t - ramp / 2.0) / (1.0 - ramp)


class ChromaPath:
    """Path between two colours on the HS disk that never enters the saturation floor circle.

    It is the straight chord when that stays outside the circle; otherwise the chord up to the
    circle, the minor arc along it, and the chord out. `point(u)` is parameterized by arc length,
    so equal time steps give equal colour steps (the plain chord clamped to the floor swings the
    hue through most of its range in a fraction of the transition).
    """

    def __init__(self, start, end, floor_r):
        self._segments = []
        self.length = 0.0

        dx, dy = end[0] - start[0], end[1] - start[1]
        a = dx * dx + dy * dy
        b = 2.0 * (start[0] * dx + start[1] * dy)
        c = start[0] ** 2 + start[1] ** 2 - floor_r * floor_r
        disc = b * b - 4.0 * a * c

        t1 = t2 = None
        if floor_r > 1e-6 and a > 1e-12 and disc > 1e-12:
            root = math.sqrt(disc)
            t1 = max(0.0, (-b - root) / (2.0 * a))
            t2 = min(1.0, (-b + root) / (2.0 * a))

        if t1 is None or t2 - t1 < 1e-6:
            self._add_line(start, end)
            return

        p_in = (start[0] + t1 * dx, start[1] + t1 * dy)
        p_out = (start[0] + t2 * dx, start[1] + t2 * dy)
        th_in = math.atan2(p_in[1], p_in[0])
        th_out = math.atan2(p_out[1], p_out[0])
        sweep = (th_out - th_in + math.pi) % (2.0 * math.pi) - math.pi

        self._add_line(start, p_in)
        arc_len = floor_r * abs(sweep)
        if arc_len > 1e-9:
            self._segments.append(("arc", arc_len, floor_r, th_in, sweep))
            self.length += arc_len
        self._add_line(p_out, end)

    def _add_line(self, p, q):
        seg_len = math.hypot(q[0] - p[0], q[1] - p[1])
        if seg_len > 1e-9:
            self._segments.append(("line", seg_len, p, q))
            self.length += seg_len

    def point(self, u):
        for seg in self._segments:
            seg_len = seg[1]
            if u <= seg_len or seg is self._segments[-1]:
                f = max(0.0, min(1.0, u / seg_len))
                if seg[0] == "line":
                    p, q = seg[2], seg[3]
                    return p[0] + f * (q[0] - p[0]), p[1] + f * (q[1] - p[1])
                r, th0, sweep = seg[2], seg[3], seg[4]
                th = th0 + f * sweep
                return r * math.cos(th), r * math.sin(th)
            u -= seg_len
        return None


async def async_setup(hass, config):
    active_tasks = {}
    # entity_id -> (h, s, base_brightness, monotonic time) of the last emitted step
    last_emitted = {}

    def cancel_active(entity_id):
        prev_task = active_tasks.get(entity_id)
        was_running = prev_task is not None and not prev_task.done()
        if was_running:
            prev_task.cancel()
        return was_running

    # Entities whose bulb did not answer on the gradient data point (until the next restart)
    gradient_failed = set()

    def localtuya_device_id(entity_id):
        """Tuya device id of a LocalTuya entity (unique id is local_<device id>_<dp>), else None."""
        entry = er.async_get(hass).async_get(entity_id)
        if not entry or entry.platform != "localtuya":
            return None
        unique_id = entry.unique_id or ""
        if not unique_id.startswith("local_") or "_" not in unique_id[6:]:
            return None
        return unique_id[6:].rsplit("_", 1)[0]

    async def handle_stop(call):
        entity_id = call.data["entity_id"]
        cancel_active(entity_id)
        active_tasks.pop(entity_id, None)
        last_emitted.pop(entity_id, None)
        _LOGGER.info("Halted light interpolation for %s", entity_id)

    def resolve_start(entity_id, was_running, target, floor_pct, white_mul, lut):
        """Pick the colour and base brightness the transition starts from."""
        cached = last_emitted.get(entity_id)
        if cached and (was_running or time.monotonic() - cached[3] < STATE_SETTLE_SECS):
            return cached[:3]

        state = hass.states.get(entity_id)
        hs = state.attributes.get("hs_color") if state and state.state == "on" else None
        if not hs:
            # Off, unavailable or not in a colour mode: apply the target directly
            return target

        cur_h, cur_s = float(hs[0]), float(hs[1])
        if (
            cached
            and hue_distance(cur_h, cached[0]) <= HUE_MATCH_DEG
            and abs(cur_s - cached[1]) <= SAT_MATCH_PCT
        ):
            return cached[:3]

        # Changed by something else: start from what the light really shows
        cur_base = target[2]
        cur_bri = state.attributes.get("brightness")
        if cur_bri:
            mul = brightness_multiplier(cur_h, max(cur_s, floor_pct), white_mul, lut)
            if mul > 1e-3:
                cur_base = max(1.0, float(cur_bri) / mul)
        return cur_h, cur_s, cur_base

    async def handle_set_target(call):
        data = call.data
        entity_id = data["entity_id"]
        floor_pct = data["min_saturation_floor"]
        white_mul = data["white_multiplier"]
        lut = normalize_hue_lut(data.get("hue_lut"))
        max_duration = data.get("max_duration", data.get("duration", 6.0))
        min_duration = min(data["min_duration"], max_duration)
        dt = data["step_ms"] / 1000.0

        target_h = data["target_h"] % 360.0
        target_s = max(floor_pct, data["target_s"])
        target_base = float(data["base_brightness"])

        was_running = cancel_active(entity_id)
        start_h, start_s, start_base = resolve_start(
            entity_id, was_running, (target_h, target_s, target_base), floor_pct, white_mul, lut
        )
        start_s = max(floor_pct, min(100.0, start_s))

        path = ChromaPath(_to_xy(start_h, start_s), _to_xy(target_h, target_s), floor_pct / 100.0)
        colour_moves = path.length > 1e-3
        if colour_moves or abs(target_base - start_base) >= 1.0:
            # Duration scaled by the travelled distance (disk diameter = 2.0 = max_duration)
            duration = max(min_duration, max_duration * min(1.0, path.length / 2.0))
        else:
            duration = 0.0

        gradient_dp = data.get("tuya_gradient_dp")
        gradient_device = None
        if gradient_dp and entity_id not in gradient_failed:
            gradient_device = localtuya_device_id(entity_id)

        async def interpolate_worker():
            try:
                start_time = time.monotonic()
                last_sent_tuple = None
                use_gradient = gradient_device is not None
                h_eff = start_h

                while True:
                    now = time.monotonic()
                    t = 1.0 if duration <= 0.0 else min(1.0, (now - start_time) / duration)
                    eased = ease_in_out(t)

                    if t >= 1.0 or not colour_moves:
                        h_eff, s_eff = target_h, target_s
                    else:
                        x, y = path.point(path.length * eased)
                        radius = math.hypot(x, y)
                        if radius > 1e-6:
                            h_eff = math.degrees(math.atan2(y, x)) % 360.0
                        s_eff = max(floor_pct, min(100.0, radius * 100.0))

                    base = start_base + eased * (target_base - start_base)
                    bri = calculate_brightness(h_eff, s_eff, base, white_mul, lut)

                    last_emitted[entity_id] = (h_eff, s_eff, base, now)

                    if use_gradient and last_sent_tuple is not None and t < 1.0:
                        # Intermediate steps go to the bulb's gradient data point, so the firmware
                        # fades between them. First and last step use light.turn_on, which switches
                        # the light on in colour mode and keeps the entity state in sync.
                        value = base * brightness_multiplier(h_eff, s_eff, white_mul, lut) / 255.0
                        cmd_tuple = (
                            round(h_eff) % 360,
                            round(s_eff * 10.0),
                            max(TUYA_MIN_VALUE, min(1000, round(value * 1000.0))),
                        )
                        if cmd_tuple != last_sent_tuple:
                            last_sent_tuple = cmd_tuple
                            try:
                                await hass.services.async_call(
                                    "localtuya",
                                    "set_dp",
                                    {
                                        "device_id": gradient_device,
                                        "dp": gradient_dp,
                                        "value": "1%04x%04x%04x00000000" % cmd_tuple,
                                    },
                                    blocking=True,
                                )
                                stalled = time.monotonic() - now > GRADIENT_STALL_SECS
                            except Exception as e:  # pylint: disable=broad-except
                                _LOGGER.warning("Gradient data point failed for %s: %s", entity_id, e)
                                stalled = True
                            if stalled:
                                _LOGGER.warning(
                                    "%s does not answer on data point %s, using light.turn_on steps",
                                    entity_id,
                                    gradient_dp,
                                )
                                gradient_failed.add(entity_id)
                                use_gradient = False
                        await asyncio.sleep(max(0.005, dt - (time.monotonic() - now)))
                        continue

                    # Deduplicate identical quantized hardware states (LocalTuya rounds Hue to integer degrees)
                    cmd_tuple = (round(h_eff) % 360, round(s_eff, 1), bri)
                    if cmd_tuple != last_sent_tuple:
                        last_sent_tuple = cmd_tuple
                        await hass.services.async_call(
                            "light",
                            "turn_on",
                            {
                                "entity_id": entity_id,
                                "hs_color": [round(h_eff, 1), round(s_eff, 1)],
                                "brightness": bri,
                            },
                            blocking=True,
                        )

                    if t >= 1.0:
                        break

                    # The blocking call already paces to the bulb; only sleep what is left of the step
                    await asyncio.sleep(max(0.005, dt - (time.monotonic() - now)))

            except asyncio.CancelledError:
                pass
            except Exception as e:  # pylint: disable=broad-except
                _LOGGER.error("Error during light interpolation for %s: %s", entity_id, e)

        active_tasks[entity_id] = hass.async_create_task(interpolate_worker())

    hass.services.async_register(DOMAIN, "set_target", handle_set_target, schema=SET_TARGET_SCHEMA)
    hass.services.async_register(DOMAIN, "stop", handle_stop, schema=STOP_SCHEMA)
    hass.services.async_register(DOMAIN, "halt", handle_stop, schema=STOP_SCHEMA)
    _LOGGER.info("Light Interpolator component initialized successfully")
    return True
