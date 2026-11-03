//! `openlogi config`: read and change every setting without the GUI.
//!
//! All commands but `path`, `show` and `schema` without `--json` print one
//! envelope in the `openlogi api` shape (`schema_version`, `ok`, `data` or
//! `error`) and exit nonzero on failure. Edits go through [`doc`]: the core types
//! validate them, [`openlogi_core::config::ConfigFile`] saves them atomically
//! without clobbering concurrent edits, and the running agent is then asked to
//! reload. No agent running is a warning, not a failure.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand};
use openlogi_ipc::AgentClient;
use serde_json::{Map, Value, json};
use tarpc::context;

use super::api::{ApiError, envelope};
use crate::agent;

mod bindings;
mod doc;
mod schema;

use doc::Session;

/// Options shared by the `config` commands.
#[derive(Debug, Args)]
pub struct ConfigArgs {
    /// Use this config file instead of the user's (for scripts and tests); the
    /// agent is not asked to reload it.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Save without asking the running agent to reload.
    #[arg(long, global = true)]
    no_reload: bool,
    /// The operation to execute.
    #[command(subcommand)]
    command: ConfigCommand,
}

/// Configuration commands.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the config file path.
    Path,
    /// List the devices the configuration knows, with the keys `--device` takes.
    Devices,
    /// Print the whole configuration (TOML, or a JSON envelope with --json).
    Show {
        /// Emit a JSON envelope instead of TOML.
        #[arg(long)]
        json: bool,
        /// Only this device's section.
        #[arg(long)]
        device: Option<String>,
    },
    /// Read one setting by dotted key.
    Get {
        /// Dotted key such as `app_settings.smooth_scroll` (the `app_settings.`
        /// prefix may be omitted) or, with --device, `smartshift.mode`.
        key: String,
        /// Device config key or name; the key is then relative to the device.
        #[arg(long)]
        device: Option<String>,
    },
    /// Change one setting, validate it, save it and reload the agent.
    Set {
        /// Dotted key; see `config schema`.
        key: String,
        /// JSON value, TOML inline value, or a bare string. `null` unsets.
        value: String,
        /// Device config key or name; the key is then relative to the device.
        #[arg(long)]
        device: Option<String>,
    },
    /// Remove a setting so its default applies.
    Unset {
        /// Dotted key; see `config schema`.
        key: String,
        /// Device config key or name; the key is then relative to the device.
        #[arg(long)]
        device: Option<String>,
    },
    /// Read or change button bindings.
    #[command(subcommand)]
    Bindings(bindings::BindingsCommand),
    /// List every settable key with its type, range, default and description.
    Schema {
        /// Emit a JSON envelope instead of text.
        #[arg(long)]
        json: bool,
    },
}

/// What a command did before any agent is involved.
pub(crate) struct Applied {
    data: Map<String, Value>,
    saved: bool,
}

impl Applied {
    fn read(data: Map<String, Value>) -> Self {
        Self { data, saved: false }
    }
}

/// Validate and save an edited document, describing the outcome.
fn finish_edit(
    session: &mut Session,
    edited: &Value,
    config: &openlogi_core::config::Config,
    mut data: Map<String, Value>,
) -> Result<Applied, ApiError> {
    let saved = session.save(edited, config)?;
    data.insert("changed".into(), saved.into());
    data.insert(
        "persistence".into(),
        if saved { "saved" } else { "unchanged" }.into(),
    );
    Ok(Applied { data, saved })
}

/// The devices the document stores, by config key.
fn device_keys(doc: &Value) -> Vec<(&String, &Value)> {
    doc.get("devices")
        .and_then(Value::as_object)
        .map(|devices| devices.iter().collect())
        .unwrap_or_default()
}

/// Resolve `selector` (a config key, custom name, model name or codename) to a
/// config key.
fn select_device(doc: &Value, selector: &str) -> Result<String, ApiError> {
    let devices = device_keys(doc);
    if devices.iter().any(|(key, _)| key.as_str() == selector) {
        return Ok(selector.to_owned());
    }
    let wanted = selector.to_lowercase();
    let matches: Vec<&String> = devices
        .iter()
        .filter(|(_, device)| {
            [
                device.get("custom_name"),
                device.pointer("/identity/display_name"),
                device.pointer("/identity/codename"),
            ]
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|name| name.to_lowercase() == wanted)
        })
        .map(|(key, _)| *key)
        .collect();
    match matches.as_slice() {
        [key] => Ok((*key).clone()),
        [] => Err(ApiError::new(
            "device_not_found",
            format!(
                "no configured device is named {selector:?}; known keys: {}",
                devices
                    .iter()
                    .map(|(key, _)| key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
        many => Err(ApiError::new(
            "ambiguous_device",
            format!(
                "{selector:?} names several devices ({}); use the config key",
                many.iter()
                    .map(|key| key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

fn display_key(path: &[String]) -> String {
    path.iter()
        .map(|segment| {
            if segment.contains('.') {
                format!("\"{segment}\"")
            } else {
                segment.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// The full document path a user key addresses.
fn resolve_path(doc: &Value, key: &str, device: Option<&str>) -> Result<Vec<String>, ApiError> {
    let relative = doc::parse_key(key)?;
    if let Some(device) = device {
        let mut path = vec!["devices".to_owned(), select_device(doc, device)?];
        path.extend(relative);
        return Ok(path);
    }
    if relative[0] == "schema_version" {
        return Err(ApiError::new(
            "invalid_value",
            "schema_version is owned by the migrations and cannot be changed",
        ));
    }
    if schema::root_fields().contains(&relative[0]) {
        Ok(relative)
    } else {
        let mut path = vec!["app_settings".to_owned()];
        path.extend(relative);
        Ok(path)
    }
}

fn get(session: &Session, key: &str, device: Option<&str>) -> Result<Applied, ApiError> {
    let path = resolve_path(&session.doc, key, device)?;
    let (scope, rest) = schema::classify(&path);
    let stored = doc::get(&session.doc, &path);
    if stored.is_none() && !schema::knows(scope, &rest) {
        return Err(ApiError::new(
            "unknown_key",
            format!(
                "{} is not a setting; see `config schema`",
                display_key(&path)
            ),
        ));
    }
    let default = schema::default_of(scope, &rest).unwrap_or(Value::Null);
    let mut data = Map::new();
    data.insert("key".into(), display_key(&path).into());
    data.insert("set".into(), stored.is_some().into());
    data.insert(
        "value".into(),
        stored.cloned().unwrap_or_else(|| default.clone()),
    );
    data.insert("default".into(), default);
    Ok(Applied::read(data))
}

fn set(
    session: &mut Session,
    key: &str,
    value: &str,
    device: Option<&str>,
) -> Result<Applied, ApiError> {
    let path = resolve_path(&session.doc, key, device)?;
    let (edited, config) = doc::with_value(&session.doc, &path, doc::candidates(value))
        .map_err(|error| prefixed(&path, error))?;
    let mut data = Map::new();
    data.insert("key".into(), display_key(&path).into());
    let (scope, rest) = schema::classify(&path);
    data.insert(
        "previous".into(),
        doc::get(&session.doc, &path)
            .cloned()
            .or_else(|| schema::default_of(scope, &rest))
            .unwrap_or(Value::Null),
    );
    data.insert(
        "value".into(),
        doc::get(&edited, &path).cloned().unwrap_or(Value::Null),
    );
    finish_edit(session, &edited, &config, data)
}

fn unset(session: &mut Session, key: &str, device: Option<&str>) -> Result<Applied, ApiError> {
    let path = resolve_path(&session.doc, key, device)?;
    let mut edited = session.doc.clone();
    doc::unset(&mut edited, &path);
    let config = doc::validate(&edited).map_err(|error| prefixed(&path, error))?;
    let mut data = Map::new();
    data.insert("key".into(), display_key(&path).into());
    finish_edit(session, &edited, &config, data)
}

fn prefixed(path: &[String], error: ApiError) -> ApiError {
    error.within(&display_key(path))
}

fn show(session: &Session, device: Option<&str>, path: &Path) -> Result<Applied, ApiError> {
    let config = match device {
        Some(selector) => {
            let key = select_device(&session.doc, selector)?;
            json!({ "device": key, "config": session.doc["devices"][&key] })
        }
        None => session.doc.clone(),
    };
    let mut data = Map::new();
    data.insert("path".into(), path.display().to_string().into());
    data.insert("exists".into(), path.exists().into());
    data.insert("config".into(), config);
    Ok(Applied::read(data))
}

fn devices(session: &Session) -> Applied {
    let list: Vec<Value> = device_keys(&session.doc)
        .into_iter()
        .map(|(key, device)| {
            json!({
                "key": key,
                "custom_name": device.get("custom_name"),
                "name": device.pointer("/identity/display_name"),
                "codename": device.pointer("/identity/codename"),
                "kind": device.pointer("/identity/kind"),
            })
        })
        .collect();
    Applied::read(Map::from_iter([("devices".to_owned(), Value::Array(list))]))
}

/// Run one command against the config file at `path`, without any agent.
pub(crate) fn execute(command: ConfigCommand, path: &Path) -> Result<Applied, ApiError> {
    let mut session = Session::open(path)?;
    match command {
        ConfigCommand::Path => Ok(Applied::read(Map::from_iter([(
            "path".to_owned(),
            path.display().to_string().into(),
        )]))),
        ConfigCommand::Devices => Ok(devices(&session)),
        ConfigCommand::Show { device, .. } => show(&session, device.as_deref(), path),
        ConfigCommand::Get { key, device } => get(&session, &key, device.as_deref()),
        ConfigCommand::Set { key, value, device } => {
            set(&mut session, &key, &value, device.as_deref())
        }
        ConfigCommand::Unset { key, device } => unset(&mut session, &key, device.as_deref()),
        ConfigCommand::Bindings(command) => bindings::run(&mut session, command),
        ConfigCommand::Schema { .. } => Ok(Applied::read(
            schema::document().as_object().cloned().unwrap_or_default(),
        )),
    }
}

/// How the running agent is reached after a save.
pub(crate) enum Agent {
    /// `--no-reload`, `--config` pointing elsewhere than the user's file, or
    /// nothing was saved.
    Skipped,
    /// Connected; reload is requested.
    Connected(AgentClient),
    /// No usable agent; carries why.
    Unreachable(String),
}

/// Ask the agent to reload; no agent is a warning, a rejection is an error.
pub(crate) async fn finish(mut applied: Applied, agent: &Agent) -> Result<Value, ApiError> {
    if applied.saved {
        let (status, warning) = match agent {
            Agent::Skipped => ("skipped", None),
            Agent::Unreachable(why) => (
                "agent_unavailable",
                Some(format!(
                    "configuration was saved but no agent reloaded it ({why}); it applies when the agent next starts or reloads"
                )),
            ),
            Agent::Connected(client) => {
                match agent::call(client.reload_config(context::current())).await {
                    Ok(Ok(())) => ("reloaded", None),
                    Ok(Err(error)) => {
                        return Err(ApiError::new(
                            "config_reload_failed",
                            format!(
                                "configuration was saved, but the agent rejected reload: {}",
                                error.message
                            ),
                        )
                        .after_write("saved"));
                    }
                    Err(error) => {
                        return Err(ApiError::new(
                            "config_reload_unknown",
                            format!(
                                "configuration was saved, but reload acknowledgement was lost: {error:?}"
                            ),
                        )
                        .after_write("saved"));
                    }
                }
            }
        };
        applied.data.insert("reload".into(), status.into());
        let warnings: Vec<Value> = warning.into_iter().map(Value::from).collect();
        applied
            .data
            .insert("warnings".into(), Value::Array(warnings));
    }
    Ok(Value::Object(applied.data))
}

/// Execute a `config` command, printing its result and choosing the status.
pub async fn run(args: ConfigArgs) -> Result<ExitCode> {
    // The agent reads the user's own file, so an alternate file is never offered
    // to it for reload.
    let custom_file = args.config.is_some();
    let path = match args.config {
        Some(path) => Ok(path),
        None => openlogi_core::paths::config_path()
            .map_err(|error| ApiError::new("config_error", error.to_string())),
    };
    let text = matches!(args.command, ConfigCommand::Schema { json: false });
    let plain_path = matches!(args.command, ConfigCommand::Path);
    let toml_show = matches!(args.command, ConfigCommand::Show { json: false, .. });
    let result = async {
        let path = path?;
        let applied = execute(args.command, &path)?;
        let agent = if applied.saved && !args.no_reload && !custom_file {
            match agent::connect().await {
                Ok(client) => Agent::Connected(client),
                Err(error) => Agent::Unreachable(error.to_string()),
            }
        } else {
            Agent::Skipped
        };
        finish(applied, &agent).await
    }
    .await;
    let success = result.is_ok();
    let mut stdout = io::stdout().lock();
    match result {
        Ok(data) if plain_path => {
            writeln!(stdout, "{}", data["path"].as_str().unwrap_or_default())?;
        }
        Ok(_) if text => write!(stdout, "{}", schema::text())?,
        Ok(data) if toml_show => {
            let config = data["config"].clone();
            match toml::to_string_pretty(&config) {
                Ok(body) => write!(stdout, "{body}")?,
                Err(_) => serde_json::to_writer_pretty(&mut stdout, &config)?,
            }
        }
        result => {
            serde_json::to_writer(&mut stdout, &envelope(result))?;
            writeln!(stdout)?;
        }
    }
    Ok(if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests;
