//! The OpenLogi agent skill: a `SKILL.md` that teaches an AI coding agent to
//! drive the CLI, embedded in the binary and installable into an agent's skill
//! directory.
//!
//! This module is the single owner of where the skill lives and how it is
//! written. The `openlogi agent-skill` command and the desktop app's
//! install button both call [`install`] / [`uninstall`] with a
//! [`SkillTarget`]; nothing else knows the layout.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use atomic_write_file::AtomicWriteFile;

/// The skill document, embedded at build time.
pub const SKILL_MD: &str = include_str!("../assets/agent-skill/SKILL.md");

/// The skill's directory name under an agent's `skills/` folder.
const SKILL_DIR: &str = "openlogi";
const SKILL_FILE: &str = "SKILL.md";

/// Where to put the skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillTarget {
    /// Claude Code: `$CLAUDE_CONFIG_DIR` (else `~/.claude`) `/skills/openlogi/`.
    Claude,
    /// An arbitrary directory that will directly contain `SKILL.md`.
    Dir(PathBuf),
}

/// What an install or uninstall did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A new file was written.
    Installed,
    /// The file already held exactly this skill.
    Unchanged,
    /// A different file was replaced (`force`).
    Replaced,
    /// The file was removed.
    Removed,
    /// There was nothing to remove.
    Absent,
}

impl Outcome {
    /// Stable machine-readable name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Unchanged => "unchanged",
            Self::Replaced => "replaced",
            Self::Removed => "removed",
            Self::Absent => "absent",
        }
    }
}

/// Why an install or uninstall did not happen.
#[derive(Debug)]
pub enum SkillError {
    /// No home directory could be determined.
    NoHome(String),
    /// A different `SKILL.md` is there; pass `force` to replace or remove it.
    Differs(PathBuf),
    /// Reading, writing or removing failed.
    Io(PathBuf, io::Error),
}

impl SkillError {
    /// Stable machine-readable code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoHome(_) => "config_error",
            Self::Differs(_) => "skill_differs",
            Self::Io(..) => "skill_io_error",
        }
    }
}

impl std::fmt::Display for SkillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoHome(why) => write!(f, "cannot find the home directory: {why}"),
            Self::Differs(path) => write!(
                f,
                "{} exists and differs from the bundled skill; use --force to replace it",
                path.display()
            ),
            Self::Io(path, error) => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for SkillError {}

/// The `SKILL.md` path for `target`, using the process environment.
///
/// # Errors
///
/// [`SkillError::NoHome`] when the Claude target needs a home directory that
/// cannot be determined.
pub fn skill_path(target: &SkillTarget) -> Result<PathBuf, SkillError> {
    let claude_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
    skill_path_with(target, claude_dir, || {
        crate::paths::home_dir().map_err(|error| error.to_string())
    })
}

fn skill_path_with(
    target: &SkillTarget,
    claude_dir: Option<PathBuf>,
    home: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<PathBuf, SkillError> {
    Ok(match target {
        SkillTarget::Dir(dir) => dir.join(SKILL_FILE),
        SkillTarget::Claude => {
            let root = match claude_dir.filter(|dir| !dir.as_os_str().is_empty()) {
                Some(dir) => dir,
                None => home().map_err(SkillError::NoHome)?.join(".claude"),
            };
            root.join("skills").join(SKILL_DIR).join(SKILL_FILE)
        }
    })
}

fn read_existing(path: &Path) -> Result<Option<String>, SkillError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        // Not UTF-8 is certainly not our skill.
        Err(error) if error.kind() == io::ErrorKind::InvalidData => Ok(Some(String::new())),
        Err(error) => Err(SkillError::Io(path.to_owned(), error)),
    }
}

/// Write the skill to `path`, refusing to replace a different file unless `force`.
///
/// # Errors
///
/// [`SkillError::Differs`] without `force`; [`SkillError::Io`] on failure.
pub fn install_at(path: &Path, force: bool) -> Result<Outcome, SkillError> {
    let io_err = |error| SkillError::Io(path.to_owned(), error);
    let existing = read_existing(path)?;
    let outcome = match existing.as_deref() {
        Some(text) if text == SKILL_MD => return Ok(Outcome::Unchanged),
        Some(_) if !force => return Err(SkillError::Differs(path.to_owned())),
        Some(_) => Outcome::Replaced,
        None => Outcome::Installed,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    let mut file = AtomicWriteFile::open(path).map_err(io_err)?;
    file.write_all(SKILL_MD.as_bytes()).map_err(io_err)?;
    file.commit().map_err(io_err)?;
    Ok(outcome)
}

/// Remove the skill at `path`, refusing a different file unless `force`. An
/// emptied skill directory is removed too.
///
/// # Errors
///
/// [`SkillError::Differs`] without `force`; [`SkillError::Io`] on failure.
pub fn uninstall_at(path: &Path, force: bool) -> Result<Outcome, SkillError> {
    match read_existing(path)? {
        None => return Ok(Outcome::Absent),
        Some(text) if text != SKILL_MD && !force => {
            return Err(SkillError::Differs(path.to_owned()));
        }
        Some(_) => {}
    }
    fs::remove_file(path).map_err(|error| SkillError::Io(path.to_owned(), error))?;
    if let Some(parent) = path.parent() {
        // Only succeeds when empty; a directory with other files stays.
        let _ = fs::remove_dir(parent);
    }
    Ok(Outcome::Removed)
}

/// What is on disk at a skill path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// No `SKILL.md` there.
    Absent,
    /// Exactly the bundled skill.
    Current,
    /// A different `SKILL.md` (an older release, or the user's own).
    Differs,
}

/// Whether the skill is installed at `path`.
///
/// # Errors
///
/// [`SkillError::Io`] when the file exists but cannot be read.
pub fn presence_at(path: &Path) -> Result<Presence, SkillError> {
    Ok(match read_existing(path)?.as_deref() {
        None => Presence::Absent,
        Some(text) if text == SKILL_MD => Presence::Current,
        Some(_) => Presence::Differs,
    })
}

/// Install the skill for `target`; the call the desktop app should make.
///
/// Returns the path written together with what happened.
///
/// # Errors
///
/// See [`install_at`] and [`skill_path`].
pub fn install(target: &SkillTarget, force: bool) -> Result<(PathBuf, Outcome), SkillError> {
    let path = skill_path(target)?;
    let outcome = install_at(&path, force)?;
    Ok((path, outcome))
}

/// Remove the skill for `target`.
///
/// # Errors
///
/// See [`uninstall_at`] and [`skill_path`].
pub fn uninstall(target: &SkillTarget, force: bool) -> Result<(PathBuf, Outcome), SkillError> {
    let path = skill_path(target)?;
    let outcome = uninstall_at(&path, force)?;
    Ok((path, outcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("skills").join("openlogi").join("SKILL.md")
    }

    #[test]
    fn bundled_skill_has_frontmatter() {
        assert!(SKILL_MD.starts_with("---\nname: openlogi\ndescription: "));
    }

    #[test]
    fn install_reinstall_and_uninstall() {
        let dir = tempfile::tempdir().unwrap();
        let path = target(&dir);
        assert_eq!(install_at(&path, false).unwrap(), Outcome::Installed);
        assert_eq!(fs::read_to_string(&path).unwrap(), SKILL_MD);
        assert_eq!(install_at(&path, false).unwrap(), Outcome::Unchanged);
        assert_eq!(uninstall_at(&path, false).unwrap(), Outcome::Removed);
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists());
        assert_eq!(uninstall_at(&path, false).unwrap(), Outcome::Absent);
    }

    #[test]
    fn different_file_needs_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = target(&dir);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "mine").unwrap();
        let error = install_at(&path, false).unwrap_err();
        assert_eq!(error.code(), "skill_differs");
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
        assert_eq!(
            uninstall_at(&path, false).unwrap_err().code(),
            "skill_differs"
        );
        assert_eq!(install_at(&path, true).unwrap(), Outcome::Replaced);
        assert_eq!(fs::read_to_string(&path).unwrap(), SKILL_MD);
    }

    #[test]
    fn uninstall_keeps_directory_with_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = target(&dir);
        install_at(&path, false).unwrap();
        fs::write(path.with_file_name("notes.txt"), "x").unwrap();
        assert_eq!(uninstall_at(&path, false).unwrap(), Outcome::Removed);
        assert!(path.parent().unwrap().exists());
    }

    #[test]
    fn claude_path_honours_config_dir() {
        let home = || Ok(PathBuf::from("/home/u"));
        let custom = skill_path_with(&SkillTarget::Claude, Some("/c".into()), home).unwrap();
        assert_eq!(custom, PathBuf::from("/c/skills/openlogi/SKILL.md"));
        let default = skill_path_with(&SkillTarget::Claude, None, home).unwrap();
        assert_eq!(
            default,
            PathBuf::from("/home/u/.claude/skills/openlogi/SKILL.md")
        );
        let empty = skill_path_with(&SkillTarget::Claude, Some(PathBuf::new()), home).unwrap();
        assert_eq!(empty, default);
        let dir = skill_path_with(&SkillTarget::Dir("/d".into()), None, home).unwrap();
        assert_eq!(dir, PathBuf::from("/d/SKILL.md"));
    }
}
