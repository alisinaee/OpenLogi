//! `config bindings`: a device's button bindings, global or for one application.

use clap::{Args, Subcommand};
use openlogi_core::binding::{Action, ButtonId};
use serde_json::{Map, Value, json};

use super::doc::{self, Session};
use super::{Applied, finish_edit, select_device};
use crate::cmd::api::ApiError;

/// Which bindings a command addresses.
#[derive(Debug, Args)]
pub struct Target {
    /// Device config key or name; see `config devices`.
    #[arg(long)]
    device: String,
    /// Application key (bundle id, `exe:<file>` or `app:<alias>`). Omit for the
    /// device's global bindings.
    #[arg(long)]
    app: Option<String>,
}

/// Read or change button bindings.
#[derive(Debug, Subcommand)]
pub enum BindingsCommand {
    /// List the stored bindings (a missing button runs its built-in default).
    List(Target),
    /// Bind a button. The value is an action (`BrowserBack`), a long-press pair
    /// (`{"short":"Copy","long":"Paste"}` or TOML `{ short = "Copy", long = "Paste" }`),
    /// or any other binding form the configuration accepts.
    Set {
        /// Which bindings to change.
        #[command(flatten)]
        target: Target,
        /// Button name, as `bindings list` prints it.
        button: String,
        /// The binding, as JSON, a TOML inline value, or a bare action name.
        value: String,
    },
    /// Remove a button's stored binding so its built-in default applies.
    Clear {
        /// Which bindings to change.
        #[command(flatten)]
        target: Target,
        /// Button name, as `bindings list` prints it.
        button: String,
    },
}

impl Target {
    /// The bindings of `device`, or of its profile for `app`.
    #[cfg(test)]
    pub(super) fn new(device: &str, app: Option<&str>) -> Self {
        Self {
            device: device.to_owned(),
            app: app.map(str::to_owned),
        }
    }
}

fn map_path(device: &str, app: Option<&str>) -> Vec<String> {
    let mut path = vec!["devices".to_owned(), device.to_owned()];
    match app {
        Some(app) => path.extend(["per_app_bindings".to_owned(), app.to_owned()]),
        None => path.push("bindings".to_owned()),
    }
    path
}

fn button(name: &str) -> Result<ButtonId, ApiError> {
    name.parse().map_err(|error| {
        ApiError::new(
            "invalid_value",
            format!("{error}; use a name printed by `config bindings list`"),
        )
    })
}

/// Remove every spelling of `button` from the table at `path`.
fn remove_spellings(doc: &mut Value, path: &[String], button: ButtonId) -> bool {
    let Some(Value::Object(table)) = path
        .iter()
        .try_fold(&mut *doc, |node, segment| node.get_mut(segment))
    else {
        return false;
    };
    let spellings: Vec<String> = table
        .keys()
        .filter(|name| name.parse::<ButtonId>().is_ok_and(|id| id == button))
        .cloned()
        .collect();
    for name in &spellings {
        table.remove(name);
    }
    !spellings.is_empty()
}

pub(super) fn run(session: &mut Session, command: BindingsCommand) -> Result<Applied, ApiError> {
    match command {
        BindingsCommand::List(target) => list(session, &target),
        BindingsCommand::Set {
            target,
            button: name,
            value,
        } => set(session, &target, &name, &value),
        BindingsCommand::Clear {
            target,
            button: name,
        } => clear(session, &target, &name),
    }
}

fn list(session: &Session, target: &Target) -> Result<Applied, ApiError> {
    let device = select_device(&session.doc, &target.device)?;
    let table = |path: &[String]| {
        doc::get(&session.doc, path)
            .cloned()
            .unwrap_or_else(|| json!({}))
    };
    let mut data = Map::new();
    data.insert("device".into(), device.clone().into());
    if let Some(app) = &target.app {
        data.insert("app".into(), app.clone().into());
        data.insert(
            "bindings".into(),
            table(&map_path(&device, Some(app.as_str()))),
        );
    } else {
        data.insert("bindings".into(), table(&map_path(&device, None)));
        data.insert(
            "per_app".into(),
            table(&["devices".into(), device, "per_app_bindings".into()]),
        );
    }
    Ok(Applied::read(data))
}

fn set(
    session: &mut Session,
    target: &Target,
    name: &str,
    value: &str,
) -> Result<Applied, ApiError> {
    let device = select_device(&session.doc, &target.device)?;
    let id = button(name)?;
    let table = map_path(&device, target.app.as_deref());
    let mut base = session.doc.clone();
    remove_spellings(&mut base, &table, id);
    let mut path = table;
    path.push(id.config_name().into_owned());
    let (edited, config) =
        doc::with_value(&base, &path, doc::candidates(value)).map_err(|error| {
            // An untagged binding reports no cause; a misspelt action name does.
            match serde_json::from_value::<Action>(Value::String(value.to_owned())) {
                Err(cause) if !value.trim_start().starts_with(['{', '[']) => {
                    ApiError::new("invalid_value", format!("{value:?}: {cause}"))
                }
                _ => error,
            }
        })?;
    let stored = doc::get(&edited, &path).cloned().unwrap_or(Value::Null);
    let mut data = Map::new();
    data.insert("device".into(), device.into());
    data.insert("button".into(), id.config_name().into_owned().into());
    if let Some(app) = &target.app {
        data.insert("app".into(), app.clone().into());
    }
    data.insert("binding".into(), stored);
    finish_edit(session, &edited, &config, data)
}

fn clear(session: &mut Session, target: &Target, name: &str) -> Result<Applied, ApiError> {
    let device = select_device(&session.doc, &target.device)?;
    let id = button(name)?;
    let table = map_path(&device, target.app.as_deref());
    let mut edited = session.doc.clone();
    remove_spellings(&mut edited, &table, id);
    // A per-app profile with no buttons left is no profile.
    if target.app.is_some()
        && doc::get(&edited, &table).is_some_and(|node| node.as_object().is_some_and(Map::is_empty))
    {
        doc::unset(&mut edited, &table);
    }
    let config = doc::validate(&edited)?;
    let mut data = Map::new();
    data.insert("device".into(), device.into());
    data.insert("button".into(), id.config_name().into_owned().into());
    if let Some(app) = &target.app {
        data.insert("app".into(), app.clone().into());
    }
    finish_edit(session, &edited, &config, data)
}
