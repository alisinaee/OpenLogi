//! The configuration as a JSON document.
//!
//! Every edit is a dotted-path change to the serialized [`Config`], and the
//! result only counts if it deserializes back into a [`Config`] and survives the
//! same TOML round trip a load performs. The core types therefore stay the one
//! validator: a key or value they reject is rejected here with their message,
//! and a field added to them becomes readable and settable with no change in
//! this module.

use std::path::Path;

use openlogi_core::config::{Config, ConfigFile};
use serde_json::{Map, Value};

use crate::cmd::api::{ApiError, save_failure_code};

/// Split a dotted key into segments. A segment containing a dot is quoted:
/// `devices."unit:1".per_app_bindings."com.apple.Safari"`.
pub(super) fn parse_key(key: &str) -> Result<Vec<String>, ApiError> {
    let invalid = |why: &str| ApiError::new("unknown_key", format!("invalid key {key:?}: {why}"));
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in key.chars() {
        match ch {
            '"' => quoted = !quoted,
            '.' if !quoted => segments.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    if quoted {
        return Err(invalid("unterminated quote"));
    }
    segments.push(current);
    if segments.iter().any(String::is_empty) {
        return Err(invalid("empty segment"));
    }
    Ok(segments)
}

/// The value at `path`, if every segment names an object member.
pub(super) fn get<'a>(doc: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(doc, |node, segment| node.get(segment))
}

/// Set `path` to `value`, creating the objects on the way.
pub(super) fn set(doc: &mut Value, path: &[String], value: Value) -> Result<(), ApiError> {
    let Some((last, parents)) = path.split_last() else {
        return Err(ApiError::new("unknown_key", "empty key"));
    };
    let mut node = doc;
    for segment in parents {
        let Value::Object(map) = node else {
            return Err(not_a_table(segment));
        };
        node = map
            .entry(segment.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    let Value::Object(map) = node else {
        return Err(not_a_table(last));
    };
    map.insert(last.clone(), value);
    Ok(())
}

fn not_a_table(segment: &str) -> ApiError {
    ApiError::new(
        "unknown_key",
        format!("cannot descend into {segment:?}: its parent is not a table"),
    )
}

/// Remove `path`; true when something was removed.
pub(super) fn unset(doc: &mut Value, path: &[String]) -> bool {
    let Some((last, parents)) = path.split_last() else {
        return false;
    };
    let mut node = doc;
    for segment in parents {
        let Some(next) = node.get_mut(segment) else {
            return false;
        };
        node = next;
    }
    node.as_object_mut()
        .is_some_and(|map| map.remove(last).is_some())
}

/// The error for a document the core types refused. An unknown field is a
/// wrong key, anything else a wrong value.
pub(super) fn rejection(message: &str) -> ApiError {
    let code = if message.contains("unknown field") {
        "unknown_key"
    } else {
        "invalid_value"
    };
    ApiError::new(code, message)
}

/// Whether `doc` is a configuration the core accepts, and if so which.
///
/// Beyond deserializing, the config goes through the file round trip in a
/// scratch directory, so the checks a load applies after parsing (and TOML's
/// own constraints) hold before the real file is touched.
pub(super) fn validate(doc: &Value) -> Result<Config, ApiError> {
    let config: Config =
        serde_json::from_value(doc.clone()).map_err(|error| rejection(&error.to_string()))?;
    let scratch = tempfile::tempdir()
        .map_err(|error| ApiError::new("config_error", format!("scratch directory: {error}")))?;
    let probe = scratch.path().join(openlogi_core::paths::CONFIG_FILE);
    config
        .save_to_path(&probe)
        .and_then(|()| Config::load_from_path(&probe))
        .map_err(|error| rejection(&error.to_string()))?;
    Ok(config)
}

/// The document with `path` set to the first of `candidates` the core accepts.
/// On failure the first candidate's error is the one reported.
pub(super) fn with_value(
    doc: &Value,
    path: &[String],
    candidates: Vec<Value>,
) -> Result<(Value, Config), ApiError> {
    let mut first_error = None;
    for candidate in candidates {
        let mut edited = doc.clone();
        if candidate.is_null() {
            unset(&mut edited, path);
        } else {
            set(&mut edited, path, candidate)?;
        }
        match validate(&edited) {
            Ok(config) => return Ok((edited, config)),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| ApiError::new("invalid_value", "no value given")))
}

/// How a command-line value may be written: JSON first, then a TOML inline
/// value (`{ short = "Copy", long = "Paste" }`), then the bare string.
pub(super) fn candidates(input: &str) -> Vec<Value> {
    let mut out = Vec::new();
    if let Ok(json) = serde_json::from_str::<Value>(input) {
        out.push(json);
    }
    if let Ok(table) = toml::from_str::<toml::Table>(&format!("v = {input}"))
        && let Some(Ok(value)) = table.get("v").map(serde_json::to_value)
        && !out.contains(&value)
    {
        out.push(value);
    }
    let text = Value::String(input.to_owned());
    if !out.contains(&text) {
        out.push(text);
    }
    out
}

/// A config file opened for one edit: the configuration, its document form and
/// the revision-tracked file that saves it.
pub(super) struct Session {
    pub(super) doc: Value,
    file: ConfigFile,
}

impl Session {
    pub(super) fn open(path: &Path) -> Result<Self, ApiError> {
        let (config, file) = ConfigFile::load_from_path(path)
            .map_err(|error| ApiError::new("config_error", error.to_string()))?;
        let doc = serde_json::to_value(&config)
            .map_err(|error| ApiError::new("config_error", error.to_string()))?;
        Ok(Self { doc, file })
    }

    /// Save `config` (the validated form of `edited`) unless `edited` changes
    /// nothing. Returns whether the file was written.
    pub(super) fn save(&mut self, edited: &Value, config: &Config) -> Result<bool, ApiError> {
        if *edited == self.doc {
            return Ok(false);
        }
        self.file.save(config).map_err(|error| {
            ApiError::new(save_failure_code(&error), error.to_string()).after_write("not_saved")
        })?;
        Ok(true)
    }
}
