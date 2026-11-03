//! `openlogi agent-skill`: install the bundled AI-agent skill.
//!
//! Prints one JSON envelope (`openlogi api` shape) except for `path` and
//! `show`, which print plain text.

use std::io::{self, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use super::api::{ApiError, envelope};
use crate::agent_skill::{self, SKILL_MD, SkillError, SkillTarget};

/// Where to install or remove the skill.
#[derive(Debug, Args)]
pub struct TargetArgs {
    /// Agent to install for. Only `claude` (Claude Code) is built in; use
    /// `--dir` for any other agent.
    #[arg(long, value_enum, default_value = "claude", conflicts_with = "dir")]
    target: Target,
    /// Directory that will directly contain `SKILL.md`.
    #[arg(long, value_name = "PATH")]
    dir: Option<PathBuf>,
    /// Replace (or remove) a `SKILL.md` that differs from the bundled one.
    #[arg(long)]
    force: bool,
}

/// Built-in install targets.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum Target {
    /// `$CLAUDE_CONFIG_DIR` (else `~/.claude`) `/skills/openlogi/`.
    Claude,
}

impl TargetArgs {
    fn target(&self) -> SkillTarget {
        match (&self.dir, self.target) {
            (Some(dir), _) => SkillTarget::Dir(dir.clone()),
            (None, Target::Claude) => SkillTarget::Claude,
        }
    }
}

/// Agent-skill commands.
#[derive(Debug, Subcommand)]
pub enum AgentSkillCommand {
    /// Print where the skill is (or would be) installed.
    Path(TargetPaths),
    /// Print the bundled SKILL.md.
    Show,
    /// Write SKILL.md into the agent's skills directory.
    Install(TargetArgs),
    /// Remove the installed SKILL.md.
    Uninstall(TargetArgs),
}

/// Target selection for `path`.
#[derive(Debug, Args)]
pub struct TargetPaths {
    /// Agent to resolve for.
    #[arg(long, value_enum, default_value = "claude", conflicts_with = "dir")]
    target: Target,
    /// Directory that will directly contain `SKILL.md`.
    #[arg(long, value_name = "PATH")]
    dir: Option<PathBuf>,
}

/// Options for `agent-skill`.
#[derive(Debug, Args)]
pub struct AgentSkillArgs {
    /// The operation to execute.
    #[command(subcommand)]
    command: AgentSkillCommand,
}

fn failure(error: &SkillError) -> ApiError {
    ApiError::new(error.code(), error.to_string())
}

fn execute(command: AgentSkillCommand) -> Result<Value, ApiError> {
    match command {
        AgentSkillCommand::Path(args) => {
            let target = TargetArgs {
                target: args.target,
                dir: args.dir,
                force: false,
            }
            .target();
            let path = agent_skill::skill_path(&target).map_err(|e| failure(&e))?;
            Ok(json!({ "path": path.display().to_string(), "exists": path.exists() }))
        }
        AgentSkillCommand::Show => Ok(json!({ "skill": SKILL_MD })),
        AgentSkillCommand::Install(args) => {
            let (path, outcome) =
                agent_skill::install(&args.target(), args.force).map_err(|e| failure(&e))?;
            Ok(json!({ "path": path.display().to_string(), "result": outcome.as_str() }))
        }
        AgentSkillCommand::Uninstall(args) => {
            let (path, outcome) =
                agent_skill::uninstall(&args.target(), args.force).map_err(|e| failure(&e))?;
            Ok(json!({ "path": path.display().to_string(), "result": outcome.as_str() }))
        }
    }
}

/// Execute an `agent-skill` command, printing its result and choosing the status.
pub fn run(args: AgentSkillArgs) -> Result<ExitCode> {
    let plain = matches!(
        args.command,
        AgentSkillCommand::Path(_) | AgentSkillCommand::Show
    );
    let result = execute(args.command);
    let success = result.is_ok();
    let mut stdout = io::stdout().lock();
    match result {
        Ok(data) if plain => {
            let text = data["path"].as_str().or_else(|| data["skill"].as_str());
            write!(stdout, "{}", text.unwrap_or_default())?;
            if data.get("path").is_some() {
                writeln!(stdout)?;
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
