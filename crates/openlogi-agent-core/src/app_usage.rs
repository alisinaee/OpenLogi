//! The persisted app-usage history behind the app switcher's Recent and Most
//! used rows.
//!
//! [`openlogi_core::switcher_rows::AppUsage`] owns the model and its bounds;
//! this is only the file it lives in, `app-usage.toml` in the profile data
//! directory. A missing or unreadable file starts an empty history: it is a
//! convenience, never worth failing the agent over.

use std::path::{Path, PathBuf};

use openlogi_core::app::ForegroundApp;
use openlogi_core::switcher_rows::AppUsage;
use tracing::warn;

/// File name of the history inside the data directory.
const FILE_NAME: &str = "app-usage.toml";

/// An [`AppUsage`] that saves itself after every change when it has a file.
#[derive(Debug, Default)]
pub struct AppUsageStore {
    usage: AppUsage,
    path: Option<PathBuf>,
}

impl AppUsageStore {
    /// A history that is never written to disk.
    #[must_use]
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// The history in the profile data directory, empty when it has none yet.
    #[must_use]
    pub fn open_default() -> Self {
        match openlogi_core::paths::data_dir() {
            Ok(dir) => Self::open(dir.join(FILE_NAME)),
            Err(error) => {
                warn!(%error, "no data directory — app usage is kept in memory only");
                Self::in_memory()
            }
        }
    }

    /// The history stored at `path`.
    #[must_use]
    pub fn open(path: PathBuf) -> Self {
        let usage = match std::fs::read_to_string(&path) {
            Ok(text) => AppUsage::from_toml(&text).unwrap_or_else(|error| {
                warn!(%error, path = %path.display(), "unreadable app usage — starting empty");
                AppUsage::default()
            }),
            Err(_) => AppUsage::default(),
        };
        Self {
            usage,
            path: Some(path),
        }
    }

    /// The history.
    #[must_use]
    pub const fn usage(&self) -> &AppUsage {
        &self.usage
    }

    /// Note that `app` came to the front, and save.
    pub fn record(&mut self, app: &ForegroundApp) {
        self.usage.record(app);
        if let Some(path) = &self.path
            && let Err(error) = save(path, &self.usage)
        {
            warn!(%error, path = %path.display(), "could not save app usage");
        }
    }
}

fn save(path: &Path, usage: &AppUsage) -> std::io::Result<()> {
    let text = usage.to_toml().map_err(std::io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str) -> ForegroundApp {
        ForegroundApp::unnamed(id.to_owned())
    }

    #[test]
    fn history_survives_a_reopen() {
        let dir = std::env::temp_dir().join(format!("openlogi-usage-{}", std::process::id()));
        let path = dir.join(FILE_NAME);
        let mut store = AppUsageStore::open(path.clone());
        store.record(&app("a"));
        store.record(&app("a"));
        let reopened = AppUsageStore::open(path);
        assert_eq!(reopened.usage().count("a"), 2);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn corrupt_file_starts_empty() {
        let dir = std::env::temp_dir().join(format!("openlogi-usage-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        std::fs::write(&path, "recent = 7").unwrap();
        assert_eq!(AppUsageStore::open(path).usage().count("a"), 0);
        std::fs::remove_dir_all(dir).ok();
    }
}
