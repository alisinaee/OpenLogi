//! Config commands against a temporary file: no hardware, no running agent.

use std::fs;
use std::future::pending;
use std::path::PathBuf;

use openlogi_core::config::Config;
use openlogi_ipc::testing::in_memory_agent;
use openlogi_ipc::{AgentRequest, AgentResponse, ConfigReloadError};
use tempfile::TempDir;

use super::*;
use crate::cmd::api::envelope;

const DEVICE: &str = "unit:aabbccdd";

fn fixture(extra: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        format!(
            "# my notes\nschema_version = 11\n\n[devices.\"{DEVICE}\"]\ncustom_name = \"Desk mouse\"\n{extra}"
        ),
    )
    .unwrap();
    (dir, path)
}

fn applied(command: ConfigCommand, path: &Path) -> Applied {
    execute(command, path).unwrap_or_else(|error| panic!("{}", envelope(Err(error))))
}

fn data(command: ConfigCommand, path: &Path) -> Value {
    Value::Object(applied(command, path).data)
}

fn failure(command: ConfigCommand, path: &Path) -> Value {
    let Err(error) = execute(command, path) else {
        panic!("expected a failure");
    };
    envelope(Err(error))["error"].clone()
}

fn set_cmd(key: &str, value: &str, device: Option<&str>) -> ConfigCommand {
    ConfigCommand::Set {
        key: key.into(),
        value: value.into(),
        device: device.map(Into::into),
    }
}

fn get_cmd(key: &str, device: Option<&str>) -> ConfigCommand {
    ConfigCommand::Get {
        key: key.into(),
        device: device.map(Into::into),
    }
}

fn bind(app: Option<&str>) -> bindings::Target {
    bindings::Target::new(DEVICE, app)
}

fn load(path: &Path) -> Config {
    Config::load_from_path(path).unwrap()
}

#[test]
fn quoted_segments_keep_their_dots() {
    assert_eq!(
        doc::parse_key(r#"devices."unit:1".per_app_bindings."com.apple.Safari""#).unwrap(),
        ["devices", "unit:1", "per_app_bindings", "com.apple.Safari"]
    );
    doc::parse_key("a..b").unwrap_err();
    doc::parse_key("\"open").unwrap_err();
}

#[test]
fn app_setting_round_trips_and_keeps_comments() {
    let (_dir, path) = fixture("");
    let result = applied(set_cmd("smooth_scroll", "true", None), &path);
    assert!(result.saved);
    assert_eq!(result.data["key"], "app_settings.smooth_scroll");
    assert_eq!(result.data["previous"], false);
    assert_eq!(result.data["persistence"], "saved");
    assert!(load(&path).app_settings.smooth_scroll);
    assert!(fs::read_to_string(&path).unwrap().contains("# my notes"));

    let read = data(get_cmd("app_settings.smooth_scroll", None), &path);
    assert_eq!(read["value"], true);
    assert_eq!(read["set"], true);
    assert_eq!(read["default"], false);

    let again = applied(set_cmd("smooth_scroll", "true", None), &path);
    assert!(!again.saved, "an identical value must not rewrite the file");
    assert_eq!(again.data["persistence"], "unchanged");
}

#[test]
fn rejects_unknown_keys_and_out_of_range_values_without_touching_the_file() {
    let (_dir, path) = fixture("");
    let before = fs::read_to_string(&path).unwrap();
    for (key, value, code) in [
        ("no_such_setting", "true", "unknown_key"),
        ("vertical_scroll_sensitivity", "0", "invalid_value"),
        ("vertical_scroll_sensitivity", "101", "invalid_value"),
        ("vertical_scroll_sensitivity", "\"fast\"", "invalid_value"),
        ("appearance", "neon", "invalid_value"),
        ("smooth_scroll", "maybe", "invalid_value"),
        ("smooth_scroll.deeper", "1", "invalid_value"),
        ("schema_version", "3", "invalid_value"),
    ] {
        let error = failure(set_cmd(key, value, None), &path);
        assert_eq!(error["code"], code, "{key}={value}: {error}");
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn bare_words_are_strings_and_null_unsets() {
    let (_dir, path) = fixture("");
    applied(set_cmd("appearance", "dark", None), &path);
    applied(set_cmd("language", "pt-BR", None), &path);
    let config = load(&path);
    assert_eq!(config.app_settings.language.as_deref(), Some("pt-BR"));
    applied(set_cmd("language", "null", None), &path);
    assert_eq!(load(&path).app_settings.language, None);
    let unset = applied(
        ConfigCommand::Unset {
            key: "appearance".into(),
            device: None,
        },
        &path,
    );
    assert!(unset.saved);
    assert_eq!(data(get_cmd("appearance", None), &path)["value"], "system");
}

#[test]
fn device_settings_resolve_by_key_or_name() {
    let (_dir, path) = fixture("");
    for selector in [DEVICE, "desk mouse"] {
        applied(set_cmd("dpi", "1600", Some(selector)), &path);
    }
    assert_eq!(load(&path).devices[DEVICE].dpi, Some(1600.into()));
    applied(
        set_cmd(
            "smartshift",
            r#"{"mode":"ratchet","auto_disengage":20,"tunable_torque":0}"#,
            Some(DEVICE),
        ),
        &path,
    );
    applied(set_cmd("smartshift.mode", "free", Some(DEVICE)), &path);
    assert_eq!(
        data(get_cmd("smartshift.mode", Some(DEVICE)), &path)["value"],
        "free"
    );
    // The core's own floor on the threshold is enforced.
    let error = failure(
        set_cmd("smartshift.auto_disengage", "3", Some(DEVICE)),
        &path,
    );
    assert_eq!(error["code"], "invalid_value");
    let error = failure(set_cmd("dpi", "1600", Some("nobody")), &path);
    assert_eq!(error["code"], "device_not_found");
    let error = failure(set_cmd("warp_drive", "1", Some(DEVICE)), &path);
    assert_eq!(error["code"], "unknown_key");
}

#[test]
fn ambiguous_device_names_are_refused() {
    let (_dir, path) = fixture("\n[devices.\"unit:11111111\"]\ncustom_name = \"desk MOUSE\"\n");
    let error = failure(set_cmd("dpi", "800", Some("Desk Mouse")), &path);
    assert_eq!(error["code"], "ambiguous_device");
}

#[test]
fn bindings_accept_plain_long_press_and_toml_forms() {
    let (_dir, path) = fixture("");
    let run = |command| {
        bindings::run(&mut Session::open(&path).unwrap(), command)
            .unwrap_or_else(|error| panic!("{}", envelope(Err(error))))
    };
    let set = |button: &str, value: &str| bindings::BindingsCommand::Set {
        target: bind(None),
        button: button.into(),
        value: value.into(),
    };
    assert!(run(set("Back", "BrowserBack")).saved);
    assert!(run(set("Forward", r#"{"short":"Copy","long":"Paste"}"#)).saved);
    assert!(run(set("MiddleClick", r#"{ short = "Copy", long = "Paste" }"#)).saved);
    let stored = load(&path).stored_bindings(DEVICE);
    assert_eq!(stored.len(), 3);
    let listed = run(bindings::BindingsCommand::List(bind(None))).data;
    assert_eq!(listed["bindings"]["Back"], "BrowserBack");
    assert_eq!(listed["bindings"]["Forward"]["long"], "Paste");

    assert!(
        run(bindings::BindingsCommand::Clear {
            target: bind(None),
            button: "Back".into()
        })
        .saved
    );
    assert_eq!(load(&path).stored_bindings(DEVICE).len(), 2);
}

#[test]
fn bad_bindings_report_the_cause_and_leave_the_file_alone() {
    let (_dir, path) = fixture("");
    let before = fs::read_to_string(&path).unwrap();
    let mut session = Session::open(&path).unwrap();
    let Err(error) = bindings::run(
        &mut session,
        bindings::BindingsCommand::Set {
            target: bind(None),
            button: "Back".into(),
            value: "NotAnAction".into(),
        },
    ) else {
        panic!("expected a failure");
    };
    let error = envelope(Err(error))["error"].clone();
    assert_eq!(error["code"], "invalid_value");
    assert!(
        error["message"].as_str().unwrap().contains("NotAnAction"),
        "{error}"
    );
    let Err(error) = bindings::run(
        &mut session,
        bindings::BindingsCommand::Set {
            target: bind(None),
            button: "NoSuchButton".into(),
            value: "Copy".into(),
        },
    ) else {
        panic!("expected a failure");
    };
    assert_eq!(envelope(Err(error))["error"]["code"], "invalid_value");
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn per_app_bindings_use_dotted_application_ids() {
    let (_dir, path) = fixture("");
    let mut session = Session::open(&path).unwrap();
    let app = "com.apple.Safari";
    let result = bindings::run(
        &mut session,
        bindings::BindingsCommand::Set {
            target: bind(Some(app)),
            button: "Back".into(),
            value: "Copy".into(),
        },
    )
    .unwrap();
    assert!(result.saved);
    let config = load(&path);
    assert!(config.per_app_overrides(DEVICE, app).is_some());

    let mut session = Session::open(&path).unwrap();
    let listed = bindings::run(
        &mut session,
        bindings::BindingsCommand::List(bind(Some(app))),
    )
    .unwrap();
    assert_eq!(listed.data["bindings"]["Back"], "Copy");
    let cleared = bindings::run(
        &mut session,
        bindings::BindingsCommand::Clear {
            target: bind(Some(app)),
            button: "Back".into(),
        },
    )
    .unwrap();
    assert!(cleared.saved);
    assert_eq!(load(&path).app_profiles(DEVICE).count(), 0);
}

#[test]
fn clearing_an_unbound_button_changes_nothing() {
    let (_dir, path) = fixture("");
    let mut session = Session::open(&path).unwrap();
    let result = bindings::run(
        &mut session,
        bindings::BindingsCommand::Clear {
            target: bind(None),
            button: "Back".into(),
        },
    )
    .unwrap();
    assert!(!result.saved);
}

#[test]
fn a_concurrent_edit_is_a_conflict_not_an_overwrite() {
    let (_dir, path) = fixture("");
    let mut session = Session::open(&path).unwrap();
    let (edited, config) = doc::with_value(
        &session.doc,
        &["app_settings".into(), "smooth_scroll".into()],
        vec![Value::Bool(true)],
    )
    .unwrap();
    fs::write(&path, "schema_version = 11\n# edited elsewhere\n").unwrap();
    let error = session.save(&edited, &config).unwrap_err();
    let envelope = envelope(Err(error));
    assert_eq!(envelope["error"]["code"], "config_conflict");
    assert_eq!(envelope["error"]["persistence"], "not_saved");
}

#[test]
fn a_missing_file_is_created_from_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("config.toml");
    let shown = data(
        ConfigCommand::Show {
            json: true,
            device: None,
        },
        &path,
    );
    assert_eq!(shown["exists"], false);
    applied(set_cmd("show_in_menu_bar", "false", None), &path);
    assert!(!load(&path).app_settings.show_in_menu_bar);
}

#[test]
fn the_document_round_trips_through_the_core_types() {
    let (_dir, path) = fixture(
        "dpi = 1200\nfn_lock = true\n[devices.\"unit:aabbccdd\".bindings]\nBack = \"BrowserBack\"\nForward = { short = \"Copy\", long = \"Paste\" }\n[devices.\"unit:aabbccdd\".per_app_bindings.\"com.apple.Safari\"]\nBack = \"Copy\"\n",
    );
    let session = Session::open(&path).unwrap();
    let rebuilt = doc::validate(&session.doc).unwrap();
    assert_eq!(
        toml::to_string(&rebuilt).unwrap(),
        toml::to_string(&load(&path)).unwrap()
    );
}

#[test]
fn schema_lists_discovered_and_annotated_keys() {
    let schema = schema::document();
    let settings = schema["settings"].as_array().unwrap();
    let find = |path: &str| {
        settings
            .iter()
            .find(|item| item["path"] == path)
            .unwrap_or_else(|| panic!("{path} missing from the schema"))
    };
    let sensitivity = find("app_settings.vertical_scroll_sensitivity");
    assert_eq!(sensitivity["min"], 1);
    assert_eq!(sensitivity["max"], 100);
    assert_eq!(sensitivity["default"], 14);
    assert_eq!(find("app_settings.appearance")["values"][2], "dark");
    assert_eq!(find("devices.<device>.enabled")["default"], true);
    assert_eq!(find("devices.<device>.fn_lock")["unset_allowed"], true);
    assert!(
        schema["bindings"]["actions"]
            .as_array()
            .is_some_and(|actions| actions.len() > 10),
        "the action vocabulary comes from the core"
    );
    assert!(schema::root_fields().contains(&"app_settings".to_owned()));
    assert!(!settings.iter().any(|item| {
        item["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("gesture_owner"))
    }));
}

/// Every annotation is checked against the core types, so a range, enum value or
/// default written down here cannot drift from what the core accepts.
#[test]
fn annotations_match_what_the_core_accepts() {
    let (_dir, path) = fixture("");
    let session = Session::open(&path).unwrap();
    for (scope, prefix) in [
        (schema::Scope::App, vec!["app_settings".to_owned()]),
        (
            schema::Scope::Device,
            vec!["devices".to_owned(), DEVICE.to_owned()],
        ),
    ] {
        for (key, kind, range, values) in schema::annotated(scope) {
            let mut at = prefix.clone();
            at.extend(key.split('.').map(str::to_owned));
            let accepts = |value: Value| {
                // A sub-key of an unset object needs its parent first.
                let mut base = session.doc.clone();
                if key == "smartshift.mode"
                    || key == "smartshift.auto_disengage"
                    || key == "smartshift.tunable_torque"
                {
                    let mut parent = prefix.clone();
                    parent.push("smartshift".into());
                    doc::set(
                        &mut base,
                        &parent,
                        json!({"mode":"free","auto_disengage":16,"tunable_torque":0}),
                    )
                    .unwrap();
                }
                doc::with_value(&base, &at, vec![value]).is_ok()
            };
            for value in values {
                assert!(accepts(json!(value)), "{key} should accept {value:?}");
            }
            if let Some((min, max)) = range {
                assert!(accepts(json!(min)), "{key} should accept its minimum {min}");
                assert!(accepts(json!(max)), "{key} should accept its maximum {max}");
            }
            assert!(
                kind == "string" || !accepts(json!("__not_a_value__")),
                "{key} accepted garbage"
            );
        }
    }
}

#[test]
fn schema_command_matches_the_document() {
    let (_dir, path) = fixture("");
    let shown = data(ConfigCommand::Schema { json: true }, &path);
    assert!(shown["settings"].as_array().unwrap().len() > 30);
}

#[tokio::test]
async fn saving_without_an_agent_warns_instead_of_failing() {
    let (_dir, path) = fixture("");
    let result = applied(set_cmd("smooth_scroll", "true", None), &path);
    let out = finish(result, &Agent::Unreachable("no agent socket".to_owned()))
        .await
        .unwrap();
    assert_eq!(out["reload"], "agent_unavailable");
    assert!(out["warnings"][0].as_str().unwrap().contains("no agent"));
    assert!(load(&path).app_settings.smooth_scroll);
}

#[tokio::test]
async fn reads_and_unchanged_saves_never_reload() {
    let (_dir, path) = fixture("");
    let read = applied(get_cmd("smooth_scroll", None), &path);
    let out = finish(read, &Agent::Unreachable("x".into())).await.unwrap();
    assert!(out.get("reload").is_none());
}

fn agent_replying(reply: Result<(), ConfigReloadError>) -> Agent {
    Agent::Connected(in_memory_agent(
        move |request| {
            assert!(matches!(request, AgentRequest::ReloadConfig {}));
            let reply = reply.clone();
            Box::pin(async move { Ok(AgentResponse::ReloadConfig(reply)) })
        },
        pending(),
    ))
}

#[tokio::test]
async fn a_saved_change_asks_the_agent_to_reload() {
    let (_dir, path) = fixture("");
    let result = applied(set_cmd("smooth_scroll", "true", None), &path);
    let out = finish(result, &agent_replying(Ok(()))).await.unwrap();
    assert_eq!(out["reload"], "reloaded");
    assert_eq!(out["warnings"], json!([]));
}

#[tokio::test]
async fn a_rejected_reload_is_an_error_after_a_successful_save() {
    let (_dir, path) = fixture("");
    let result = applied(set_cmd("smooth_scroll", "true", None), &path);
    let agent = agent_replying(Err(ConfigReloadError {
        message: "nope".into(),
    }));
    let error = finish(result, &agent).await.unwrap_err();
    let error = envelope(Err(error))["error"].clone();
    assert_eq!(error["code"], "config_reload_failed");
    assert_eq!(error["persistence"], "saved");
}

#[tokio::test]
async fn no_reload_skips_the_agent() {
    let (_dir, path) = fixture("");
    let result = applied(set_cmd("smooth_scroll", "true", None), &path);
    let out = finish(result, &Agent::Skipped).await.unwrap();
    assert_eq!(out["reload"], "skipped");
}
