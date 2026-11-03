---
name: openlogi
description: Control Logitech mice and keyboards through the OpenLogi CLI with no GUI. Use when the user wants to list Logitech devices, remap buttons (globally or per app), set long-press actions, change DPI, SmartShift, scrolling or any OpenLogi setting, or check whether the OpenLogi agent and macOS permissions are healthy.
---

# OpenLogi CLI

OpenLogi is a local-first replacement for Logitech Options+. Everything the GUI
does is available from the `openlogi` binary. Every command below except
`config path` and text-mode `config show`/`config schema` prints one JSON
envelope: `{"schema_version":1,"ok":true,"data":{...}}` or
`{"ok":false,"error":{"code","message"}}`, and exits nonzero on failure. Always
check `ok`, never parse the message text.

If `openlogi` is not on PATH, the app bundle ships it (macOS:
`/Applications/OpenLogi.app/Contents/MacOS/openlogi`).

## Two interfaces

- `openlogi config ...` edits `config.toml` (every setting, bindings, per-app
  bindings). It works with **no agent running**; after a save it asks a running
  agent to reload. `"reload":"agent_unavailable"` is a warning, not a failure:
  the change applies when the agent next starts.
- `openlogi api ...` talks to the **running agent** and the hardware (live DPI,
  SmartShift, Fn lock, report rate, Easy-Switch). It fails with
  `agent_unavailable` if the agent is not running. Add `--save` to also persist.

## Workflow

1. Health: `openlogi api status` (agent version, `inventory`, `hook_installed`,
   `accessibility_granted`, `input_monitoring_granted`, `virtual_keyboard`).
   On macOS, `inventory` stuck on unavailable with `input_monitoring_granted:
   false` means the user must grant Input Monitoring in System Settings; you
   cannot do that for them. A remap that does nothing usually means
   `hook_installed:false` or `accessibility_granted:false`.
2. Devices: `openlogi api devices` (opaque route `id`s for `api` commands,
   battery, capabilities) and `openlogi config devices` (config keys such as
   `unit:aabbccdd` that `--device` takes; custom name, model name or codename
   also work if unambiguous). The two ID spaces differ: never pass an `api` id
   to `config --device`.
3. Discover settings: `openlogi config schema --json` once, then read
   `data.settings` (`path`, `scope` app/device/root, `type`, `default`, `min`,
   `max`, `values`, `description`) and `data.bindings` (binding forms and the
   action vocabulary). Do not guess key names or action names.
4. Back up before bulk edits: `openlogi config show --json > backup.json`
   (the `data.config` object is the whole configuration).
5. Change, then verify: `config set` then `config get` (or `bindings list`).
   `set` returns `previous`, `value`, `changed`, `persistence`.

## Settings

```sh
openlogi config get smooth_scroll
openlogi config set vertical_scroll_sensitivity 20         # app-wide (app_settings.*)
openlogi config set appearance dark                        # bare word = string
openlogi config set language null                          # null (or `unset`) restores default
openlogi config unset theme_dark
openlogi config set dpi 1600 --device "Desk mouse"         # per device
openlogi config set smartshift.mode free --device unit:aabbccdd
openlogi config set smartshift '{"mode":"ratchet","auto_disengage":20,"tunable_torque":0}' --device unit:aabbccdd
```

Keys are dotted; without `--device` a key that is not a root table lives under
`app_settings`. Values are JSON, a TOML inline value, or a bare string. Invalid
keys/values are rejected with `unknown_key` / `invalid_value` and nothing is
written. Quote dotted segments: `devices."unit:1".per_app_bindings."com.apple.Safari"`.

## Bindings

```sh
openlogi config bindings list --device unit:aabbccdd
openlogi config bindings set --device unit:aabbccdd Back BrowserBack
openlogi config bindings set --device unit:aabbccdd Forward '{"short":"Copy","long":"Paste"}'
openlogi config bindings set --device unit:aabbccdd --app com.apple.Safari Back Copy
openlogi config bindings clear --device unit:aabbccdd Back      # back to built-in default
```

- Button names are those `openlogi list` / `bindings list` print (`Back`,
  `Forward`, `Thumbwheel`, `control:0x010a`, ...).
- A binding is a plain action name or a structured form from the schema:
  long-press `{"short":A,"long":B,"hold_ms":N}` (`hold_ms` 200-3000, default
  500, also valid in a per-app binding), wheel-hold, per-direction gesture, and
  the app switcher `"AppSwitcher"` or `{"AppSwitcher":{"step":60,"hold_ms":250}}`
  (`step` 10-1000 mouse counts). A shortcut action is `{"CustomShortcut":"Cmd+Alt+Left"}`.
- `--app` is the key as stored: a bundle id (`com.apple.Safari`), `exe:<file>`
  or `app:<alias>`. Find a macOS bundle id with
  `osascript -e 'id of app "Arc"'`. Omit `--app` for the device-wide binding.
- Clearing a button restores its built-in default; there is no "disabled"
  except the `None`/disable action if the schema lists one.

## Live hardware (`openlogi api`, needs the agent)

```sh
openlogi api dpi --device "$ID"                      # read
openlogi api dpi --device "$ID" --set 1600 --save    # apply now and persist
openlogi api smartshift --device "$ID" --mode free
openlogi api smartshift --device "$ID" --mode ratchet --auto-disengage 255
openlogi api fn-lock --device "$ID" --set on
openlogi api report-rate --device "$ID" --set 1000 --save
openlogi api easy-switch --device "$ID"
```

`$ID` is the exact `id` from `api devices` (quote it). Without `--save` the
change is not persisted. Prefer `config set dpi ... --device` for a persisted
value when no hardware is attached.

## Recipes

Constant-speed scrolling (no acceleration, eased smooth wheel), when listed by
the schema:

```sh
openlogi config set scroll_acceleration false
openlogi config set smooth_wheel_easing <value from schema `values`>
openlogi config set smooth_wheel_duration_ms 150
```

Desktop switcher overlay after Previous/Next Desktop (macOS), with each
desktop's windows and apps mapped in its boxes:

```sh
openlogi config set show_desktop_switcher true
openlogi config set desktop_switcher_show_apps true
```

App switcher on the DPI button: a tap switches to the previous app, a hold opens
the native switcher and sideways mouse movement steps through it:

```sh
D=unit:aabbccdd
openlogi config bindings set --device $D DpiToggle AppSwitcher
openlogi config bindings set --device $D DpiToggle '{"AppSwitcher":{"step":80,"hold_ms":300}}'   # tuned
```

Back/Forward short press, long press for another action, only in Arc
(`company.thebrowser.Browser`):

```sh
D=unit:aabbccdd
openlogi config bindings set --device $D --app company.thebrowser.Browser Back '{"short":"BrowserBack","long":"PreviousTab"}'
openlogi config bindings set --device $D --app company.thebrowser.Browser Forward '{"short":"BrowserForward","long":"NextTab"}'
```

Arc: Back taps Cmd+Alt+Left, holding it for one second sends Cmd+Alt+Right:

```sh
openlogi config bindings set --device $D --app company.thebrowser.Browser Back '{"short":{"CustomShortcut":"Cmd+Alt+Left"},"long":{"CustomShortcut":"Cmd+Alt+Right"},"hold_ms":1000}'
```

Confirm the action names against `config schema --json` first; the names above
are examples.

## Safety rules

- Never hand-edit `config.toml` (or the temp file behind it): use `config set`,
  which validates with the app's own types, keeps comments, saves atomically and
  refuses to clobber a concurrent edit (`config_conflict`: re-read and retry).
- Back up with `config show --json` before bulk edits; to roll back, `set` the
  old values (or `unset` keys that were not set).
- Verify every change with a read-back; do not assume the agent reloaded: check
  `data.reload` (`reloaded`, `skipped`, `agent_unavailable`).
- Do not run `diag`, `fixture`, `backlight`, `camera` or other hardware
  subcommands unless asked; they write to devices.
- Use `--config <PATH>` to experiment on a copy without touching the real file
  or the agent; `--no-reload` saves without contacting the agent.
- Error codes: `unknown_key`, `invalid_value`, `device_not_found`,
  `ambiguous_device`, `config_error`, `config_conflict`, `config_busy`,
  `config_save_failed`, `config_reload_failed`, `config_reload_unknown`,
  `agent_unavailable`, `version_mismatch`.
