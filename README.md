# hass-light-sync

hass-light-sync is a program to capture the display's image and send the average color to a light in Home Assistant. It's similar to solutions like Philips Hue Sync and Ambilight, but only supports a single light at the moment.

# Demo (Video)
[![Demo Video](https://img.youtube.com/vi/iO3uI8IiNZM/0.jpg)](https://youtu.be/iO3uI8IiNZM)

# Installation

## Precompiled Binaries (Windows)
Download the zip-Archive from Releases and edit the `settings.json` file with your server information, and run the `hass-light-sync.exe`.

## Compile it yourself
Install the Rust SDK and clone the git repo. Run:

    cargo build --release 

and copy the `hass-light-sync.exe` from the target folder anywhere you like. Then copy the `settings.json` file from the root of the project to the same directory, and edit it to match your server info.

# Configuration

The application supports hot-reloading `settings.json` automatically when saved. Two modes are supported:
- `"night"`: smart discrete accent palette with 2D chromaticity disk chord transitions and perceived brightness correction.
- `"standard"`: classic screen-averaged ambilight with gamma correction.

When `use_light_interpolator` is true (default), the app offloads 2D HS interpolation and brightness calibration directly to the `light_interpolator` custom component in Home Assistant, eliminating high-frequency network streaming from the PC.

The component lives in `custom_components/light_interpolator/`. Copy that folder into the Home Assistant config directory (`<config>/custom_components/light_interpolator/`), add `light_interpolator:` to `configuration.yaml` and restart Home Assistant. Update the component together with the app: the app sends `min_duration` and `white_multiplier`, which older component versions ignore.

```json
{
    "api_endpoint": "http://<IP>:<PORT>", 
    "light_entity_name": "light.[entity]",
    "token": "LONG ACCESS TOKEN HERE",  
    "monitor_id": 0,
    "grab_interval": 50,
    "send_interval_ms": 80,
    "skip_pixels": 10,
    "transition": null,
    "max_brightness": 255,
    "mode": "night",
    "use_light_interpolator": true,

    "hue_brightness_lut": [
        { "hue": 0.0, "brightness_mul": 0.5, "color": "Red" },
        { "hue": 60.0, "brightness_mul": 0.625, "color": "Yellow" },
        { "hue": 120.0, "brightness_mul": 0.75, "color": "Green" },
        { "hue": 180.0, "brightness_mul": 0.875, "color": "Cyan" },
        { "hue": 240.0, "brightness_mul": 1.0, "color": "Blue" },
        { "hue": 300.0, "brightness_mul": 0.75, "color": "Magenta" },
        { "hue": 360.0, "brightness_mul": 0.5, "color": "Red" }
    ],

    "night_mode": {
        "brightness": null,
        "brightness_multiplier": 1.0,
        "transition_tau_ms": 1500,
        "min_saturation_floor": 35.0,
        "hysteresis_ratio": 1.30,
        "hist_smoothing": 0.6,
        "min_accent_weight": 0.03,
        "min_saturation": 0.15,
        "allow_neutral_on_weak": false,
        "weak_fallback_brightness": 48,
        "warm": [255, 72, 18],
        "magenta": [215, 42, 125],
        "green": [48, 205, 78],
        "cyan": [0, 175, 195],
        "violet": [105, 48, 175]
    },

    "standard_mode": {
        "smoothing_tau_ms": 300.0,
        "gamma": 1.0,
        "gamma_white": null,
        "white_chroma_max": 0.04
    }
}
```
