//! What can be set: the annotated settings plus every key the core types accept.
//!
//! Descriptions, ranges and enum values are written down for the keys known when
//! this table was written. Everything else is discovered from the core types
//! themselves: a field with a non-null default shows up in the default document,
//! and the types reject an unknown key with a message that lists the valid ones.
//! A setting added to the core is therefore listed (as unannotated) and settable
//! without touching this file; annotating it only adds its description and range.

use std::fmt::Write as _;

use openlogi_core::binding::Action;
use openlogi_core::config::{AppSettings, Config, DeviceConfig, SCHEMA_VERSION};
use serde_json::{Map, Value, json};

/// Where a key lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Scope {
    /// Under `app_settings`.
    App,
    /// Under `devices.<device>`; addressed with `--device`.
    Device,
    /// A top-level key.
    Root,
}

impl Scope {
    fn label(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Device => "device",
            Self::Root => "root",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Bool,
    Integer,
    Text,
    Enum,
    Object,
    Array,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Bool => "boolean",
            Self::Integer => "integer",
            Self::Text => "string",
            Self::Enum => "enum",
            Self::Object => "object",
            Self::Array => "array",
        }
    }
}

#[derive(Clone, Copy)]
struct Spec {
    key: &'static str,
    kind: Kind,
    /// Inclusive bounds of an integer.
    range: Option<(i64, i64)>,
    values: &'static [&'static str],
    /// JSON default when the document does not show one (a field omitted at its
    /// default, such as `enabled = true`).
    default: Option<&'static str>,
    description: &'static str,
}

const fn spec(key: &'static str, kind: Kind, description: &'static str) -> Spec {
    Spec {
        key,
        kind,
        range: None,
        values: &[],
        default: None,
        description,
    }
}

const fn int(key: &'static str, min: i64, max: i64, description: &'static str) -> Spec {
    Spec {
        range: Some((min, max)),
        ..spec(key, Kind::Integer, description)
    }
}

const fn choice(
    key: &'static str,
    values: &'static [&'static str],
    description: &'static str,
) -> Spec {
    Spec {
        values,
        ..spec(key, Kind::Enum, description)
    }
}

const fn with_default(spec: Spec, default: &'static str) -> Spec {
    Spec {
        default: Some(default),
        ..spec
    }
}

const GLIDE: &[&str] = &["sharp", "linear", "smooth", "floaty"];

const APP: &[Spec] = &[
    spec(
        "launch_at_login",
        Kind::Bool,
        "Start the background agent at login.",
    ),
    spec(
        "check_for_updates",
        Kind::Bool,
        "Check once per launch whether a newer version exists.",
    ),
    spec(
        "auto_install_updates",
        Kind::Bool,
        "Download and stage updates found by the update check.",
    ),
    spec(
        "update_prompt_seen",
        Kind::Bool,
        "The first-run update prompt has been answered.",
    ),
    choice(
        "update_channel",
        &["stable", "beta"],
        "Update channel; unset follows the running version.",
    ),
    spec(
        "show_in_menu_bar",
        Kind::Bool,
        "Show the menu-bar or tray icon.",
    ),
    spec(
        "capture_mouse_events",
        Kind::Bool,
        "Install the OS mouse hook used for button remapping; applies on agent restart.",
    ),
    spec(
        "smooth_scroll",
        Kind::Bool,
        "Replace ordinary mouse-wheel input with a smooth scroll animation.",
    ),
    int(
        "vertical_scroll_sensitivity",
        1,
        100,
        "Distance multiplier of vertical wheel scrolling (14 is 1x).",
    ),
    spec(
        "momentum_scrolling",
        Kind::Object,
        "App-wide momentum scrolling of a smoothed wheel.",
    ),
    spec(
        "momentum_scrolling.enabled",
        Kind::Bool,
        "Glide after the wheel stops.",
    ),
    int(
        "momentum_scrolling.pixels_per_notch",
        10,
        60,
        "Pixels one wheel notch scrolls; clamped into range.",
    ),
    choice(
        "momentum_scrolling.glide_shape",
        GLIDE,
        "Deceleration curve of the glide.",
    ),
    int(
        "momentum_scrolling.max_glide_speed",
        2000,
        15000,
        "Fastest glide in pixels per second; clamped into range.",
    ),
    int(
        "momentum_scrolling.glide_ms",
        150,
        1200,
        "Glide duration in milliseconds; clamped into range.",
    ),
    int(
        "momentum_scrolling.repeat_boost",
        100,
        200,
        "Percent a repeated flick adds; clamped into range.",
    ),
    choice("app_icon", &["openlogi", "prism"], "Application icon."),
    spec(
        "auto_download_assets",
        Kind::Bool,
        "Download device images automatically.",
    ),
    choice(
        "asset_source",
        &["automatic", "openlogi", "cloudflare", "fastly"],
        "Mirror used for device assets.",
    ),
    spec(
        "language",
        Kind::Text,
        "UI language code such as en, de, pt-BR; unset follows the system.",
    ),
    int(
        "thumbwheel_sensitivity",
        1,
        100,
        "Thumb-wheel responsiveness (14 is 1x).",
    ),
    choice(
        "appearance",
        &["system", "light", "dark"],
        "Light or dark appearance.",
    ),
    choice(
        "ui_scale",
        &["small", "normal", "large", "extra_large"],
        "Interface scale (90, 100, 110, 125 percent).",
    ),
    choice(
        "device_view_mode",
        &["grid", "list", "carousel"],
        "Layout of the Home device gallery.",
    ),
    spec(
        "theme_light",
        Kind::Text,
        "Theme name used in light mode; unset uses the brand theme.",
    ),
    spec(
        "theme_dark",
        Kind::Text,
        "Theme name used in dark mode; unset uses the brand theme.",
    ),
    int(
        "ui_radius",
        0,
        255,
        "Corner radius override in pixels; unset keeps each theme's own.",
    ),
    choice(
        "mouse_profile_target",
        &["pointer", "focused"],
        "Which application selects per-app mouse bindings.",
    ),
    spec(
        "low_battery_notifications",
        Kind::Bool,
        "Overlay notice when a battery runs low.",
    ),
    spec(
        "lock_key_notifications",
        Kind::Bool,
        "Overlay notice when a keyboard lock key changes.",
    ),
    spec(
        "animate_desktop_switch",
        Kind::Bool,
        "Slide to the neighbouring desktop (macOS).",
    ),
    spec(
        "show_desktop_switcher",
        Kind::Bool,
        "Show the desktop switcher overlay after Previous/Next Desktop (macOS).",
    ),
    spec(
        "desktop_switcher_show_apps",
        Kind::Bool,
        "Map each desktop in the switcher: window outlines and app icons; needs show_desktop_switcher (macOS).",
    ),
    choice(
        "desktop_switcher_size",
        &["small", "medium", "large"],
        "How large the desktop switcher is drawn (macOS).",
    ),
    spec(
        "scroll_acceleration",
        Kind::Bool,
        "Let macOS accelerate the smoothed wheel; false scrolls every notch the same distance (macOS).",
    ),
    int(
        "smooth_wheel_duration_ms",
        40,
        400,
        "Milliseconds one smoothed wheel step takes to play out; clamped into range.",
    ),
    choice(
        "smooth_wheel_easing",
        &["smooth", "ease_out", "linear"],
        "Curve a smoothed wheel step follows.",
    ),
];

const DEVICE: &[Spec] = &[
    with_default(
        spec(
            "enabled",
            Kind::Bool,
            "Let OpenLogi manage this device; false leaves it fully native.",
        ),
        "true",
    ),
    spec("custom_name", Kind::Text, "User-assigned device name."),
    int(
        "dpi",
        0,
        65535,
        "Saved sensor DPI, re-applied on reconnect; the device clamps it to what it supports.",
    ),
    spec(
        "dpi_presets",
        Kind::Array,
        "Ordered DPI presets for the cycle and set-preset actions, e.g. [400, 800, 1600].",
    ),
    with_default(
        spec(
            "invert_scroll",
            Kind::Bool,
            "Invert the wheel direction relative to the OS.",
        ),
        "false",
    ),
    with_default(
        spec(
            "smooth_wheel",
            Kind::Bool,
            "Smooth this device's main wheel in software.",
        ),
        "false",
    ),
    choice(
        "scroll_resolution",
        &["low", "high"],
        "HID++ wheel resolution.",
    ),
    int(
        "thumbwheel_sensitivity",
        1,
        100,
        "Per-device thumb-wheel sensitivity; unset follows the app-wide value.",
    ),
    spec(
        "momentum_scrolling",
        Kind::Object,
        "Per-device momentum scrolling override; same fields as the app-wide one.",
    ),
    spec(
        "fn_lock",
        Kind::Bool,
        "Keyboard Fn lock: true sends F1-F12 without Fn.",
    ),
    spec(
        "keep_os_layout",
        Kind::Bool,
        "Keep a multi-platform keyboard on the host OS layout (unset behaves as true).",
    ),
    int(
        "report_rate",
        1,
        65535,
        "Report rate in hertz; applied only where the device lists it for its connection.",
    ),
    int(
        "pointer_speed",
        32,
        768,
        "Pointer speed of mice without adjustable DPI, in 1/256 steps (256 is 1x).",
    ),
    int(
        "force_sensing",
        0,
        65535,
        "Press force of a force-sensing button; the agent clamps it to the device range.",
    ),
    spec(
        "smartshift",
        Kind::Object,
        "SmartShift wheel as {\"mode\": \"free\"|\"ratchet\", \"auto_disengage\": 8-255 (255 is permanent ratchet), \"tunable_torque\": 0-100 (0 is none)}.",
    ),
    choice(
        "smartshift.mode",
        &["free", "ratchet"],
        "SmartShift wheel mode; only on a device whose smartshift object already exists.",
    ),
    int(
        "smartshift.auto_disengage",
        8,
        255,
        "SmartShift auto-release threshold; 255 keeps the ratchet permanently engaged.",
    ),
    int(
        "smartshift.tunable_torque",
        0,
        100,
        "SmartShift wheel force percent; 0 means none.",
    ),
    spec(
        "lighting",
        Kind::Object,
        "Keyboard RGB as {\"enabled\": bool, \"color\": \"#rrggbb\", \"brightness\": 0-100}.",
    ),
    spec(
        "disabled_keys",
        Kind::Array,
        "Keys to disable on reconnect: caps_lock, num_lock, scroll_lock, insert, windows_command.",
    ),
    spec(
        "host_switch_targets",
        Kind::Array,
        "Config keys of pointing devices that follow this keyboard's host switch.",
    ),
    spec(
        "bindings",
        Kind::Object,
        "Button bindings; edit with `config bindings`.",
    ),
    spec(
        "per_app_bindings",
        Kind::Object,
        "Per-application button bindings; edit with `config bindings --app`.",
    ),
    spec(
        "action_ring",
        Kind::Object,
        "Actions Ring layout and settings.",
    ),
];

const ROOT: &[Spec] = &[spec(
    "selected_device",
    Kind::Text,
    "Config key of the device the GUI shows first.",
)];

/// Fields the core still accepts only to migrate old files.
const LEGACY_DEVICE_FIELDS: &[&str] = &["button_bindings", "gesture_bindings", "gesture_owner"];

fn table(scope: Scope) -> &'static [Spec] {
    match scope {
        Scope::App => APP,
        Scope::Device => DEVICE,
        Scope::Root => ROOT,
    }
}

/// The names a core type lists in its "unknown field" rejection.
fn listed_fields(message: &str) -> Vec<String> {
    message
        .split('`')
        .skip(3)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

fn scraped(config_probe: Value) -> Vec<String> {
    serde_json::from_value::<Config>(config_probe)
        .err()
        .map(|error| listed_fields(&error.to_string()))
        .unwrap_or_default()
}

fn default_document(scope: Scope) -> Value {
    match scope {
        Scope::App => serde_json::to_value(AppSettings::default()),
        Scope::Device => serde_json::to_value(DeviceConfig::default()),
        Scope::Root => serde_json::to_value(Config::default()),
    }
    .unwrap_or(Value::Null)
}

/// Leaf and object keys of the default document, dotted.
fn flatten(prefix: &str, value: &Value, out: &mut Vec<String>) {
    if let Value::Object(map) = value {
        for (key, child) in map {
            let dotted = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            out.push(dotted.clone());
            flatten(&dotted, child, out);
        }
    }
}

/// Every key of `scope` the core accepts, annotated or not, in a stable order.
fn keys(scope: Scope) -> Vec<String> {
    let mut found: Vec<String> = table(scope).iter().map(|s| s.key.to_owned()).collect();
    let mut discovered = Vec::new();
    flatten("", &default_document(scope), &mut discovered);
    match scope {
        Scope::App => discovered.extend(scraped(json!({
            "schema_version": SCHEMA_VERSION, "app_settings": { "__probe__": 0 }
        }))),
        Scope::Device => discovered.extend(
            scraped(json!({
                "schema_version": SCHEMA_VERSION, "devices": { "__probe__": { "__probe__": 0 } }
            }))
            .into_iter()
            .filter(|field| !LEGACY_DEVICE_FIELDS.contains(&field.as_str())),
        ),
        Scope::Root => discovered.retain(|key| key == "selected_device"),
    }
    for key in discovered {
        if !found.contains(&key) {
            found.push(key);
        }
    }
    found
}

fn lookup<'a>(value: &'a Value, dotted: &str) -> &'a Value {
    dotted
        .split('.')
        .try_fold(value, |node, segment| node.get(segment))
        .unwrap_or(&Value::Null)
}

fn entry(scope: Scope, key: &str, defaults: &Value) -> Value {
    let annotation = table(scope).iter().find(|s| s.key == key);
    let default = annotation
        .and_then(|s| s.default)
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_else(|| lookup(defaults, key).clone());
    let kind = annotation.map_or_else(
        || match &default {
            Value::Bool(_) => "boolean",
            Value::Number(_) => "integer",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
            Value::Null => "unknown",
        },
        |s| s.kind.label(),
    );
    let mut out = Map::new();
    out.insert("key".into(), key.into());
    out.insert("scope".into(), scope.label().into());
    out.insert("type".into(), kind.into());
    out.insert("default".into(), default.clone());
    out.insert("unset_allowed".into(), (default.is_null()).into());
    if let Some(spec) = annotation {
        if !spec.values.is_empty() {
            out.insert("values".into(), json!(spec.values));
        }
        if let Some((min, max)) = spec.range {
            out.insert("min".into(), min.into());
            out.insert("max".into(), max.into());
        }
    }
    out.insert(
        "description".into(),
        annotation.map_or(Value::Null, |s| s.description.into()),
    );
    out.insert("annotated".into(), annotation.is_some().into());
    Value::Object(out)
}

/// Whether `rest` (a key below `scope`) is a key the core accepts, or sits
/// below an object or array key that is.
pub(super) fn knows(scope: Scope, rest: &[String]) -> bool {
    let dotted = rest.join(".");
    keys(scope).iter().any(|key| {
        key == &dotted
            || dotted
                .strip_prefix(key.as_str())
                .is_some_and(|tail| tail.starts_with('.'))
    })
}

/// The default value of a key, or `None` when it is not a known key.
pub(super) fn default_of(scope: Scope, rest: &[String]) -> Option<Value> {
    let dotted = rest.join(".");
    keys(scope)
        .contains(&dotted)
        .then(|| entry(scope, &dotted, &default_document(scope))["default"].clone())
}

/// The names the core lists in the rejection of an unknown action.
fn action_names() -> Vec<String> {
    serde_json::from_value::<Action>(json!("__probe__"))
        .err()
        .map(|error| listed_fields(&error.to_string()))
        .unwrap_or_default()
}

/// The complete machine-readable schema.
pub(super) fn document() -> Value {
    let mut settings = Vec::new();
    for scope in [Scope::App, Scope::Device, Scope::Root] {
        let defaults = default_document(scope);
        for key in keys(scope) {
            let mut item = entry(scope, &key, &defaults);
            let full = match scope {
                Scope::App => format!("app_settings.{key}"),
                Scope::Device => format!("devices.<device>.{key}"),
                Scope::Root => key.clone(),
            };
            item["path"] = full.into();
            settings.push(item);
        }
    }
    json!({
        "settings": settings,
        "usage": {
            "app": "config set app_settings.<key> <value>; the app_settings. prefix may be omitted",
            "device": "config set --device <key|name> <key> <value>",
            "value": "JSON (true, 12, \"text\", [1,2], {\"a\":1}); a bare word is taken as a string; null unsets",
            "key_quoting": "quote a segment that contains a dot: devices.\"unit:1\".per_app_bindings.\"com.apple.Safari\"",
        },
        "bindings": {
            "button": "mouse button name (LeftClick, Back, Thumbwheel, ...), a keypad slot or control:0x<cid>",
            "forms": [
                { "form": "action", "example": "BrowserBack" },
                { "form": "long_press", "example": { "short": "Copy", "long": "Paste" } },
                { "form": "long_press_hold", "example": { "short": "Copy", "long": "Paste", "hold_ms": 1000 }, "hold_ms": "200..=3000, default 500; also valid in a per-app binding" },
                { "form": "app_switcher", "example": { "AppSwitcher": { "step": 60, "hold_ms": 250 } }, "step": "10..=1000 mouse counts, default 60; hold_ms 200..=3000, default 250; plain \"AppSwitcher\" uses the defaults" },
                { "form": "wheel_hold", "example": { "click": "Back", "wheel": "HorizontalScroll" } },
                { "form": "gesture", "example": { "Click": "Back", "Up": "MissionControl" } },
            ],
            "actions": action_names(),
            "per_app": "a per-app binding takes the same value forms the core accepts for per_app_bindings",
        },
    })
}

/// One line per key, for people.
pub(super) fn text() -> String {
    let doc = document();
    let mut out = String::new();
    for item in doc["settings"].as_array().into_iter().flatten() {
        let _ = writeln!(
            out,
            "{} ({}) default {} - {}",
            item["path"].as_str().unwrap_or_default(),
            item["type"].as_str().unwrap_or_default(),
            item["default"],
            item["description"].as_str().unwrap_or("unannotated"),
        );
    }
    out
}

/// The scope and key below it for a normalized path.
pub(super) fn classify(path: &[String]) -> (Scope, Vec<String>) {
    match path {
        [first, rest @ ..] if first == "app_settings" => (Scope::App, rest.to_vec()),
        [first, _, rest @ ..] if first == "devices" => (Scope::Device, rest.to_vec()),
        _ => (Scope::Root, path.to_vec()),
    }
}

/// The annotated specs of a scope, for the tests that hold them to the core:
/// key, type label, integer bounds and enum values.
#[cfg(test)]
pub(super) fn annotated(
    scope: Scope,
) -> impl Iterator<
    Item = (
        &'static str,
        &'static str,
        Option<(i64, i64)>,
        &'static [&'static str],
    ),
> {
    table(scope)
        .iter()
        .map(|s| (s.key, s.kind.label(), s.range, s.values))
}

/// The top-level keys of the configuration document.
pub(super) fn root_fields() -> Vec<String> {
    scraped(json!({ "schema_version": SCHEMA_VERSION, "__probe__": 0 }))
}
