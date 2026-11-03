//! AI agent settings page: installs the OpenLogi skill that lets AI coding
//! agents drive the `openlogi` command line.
//!
//! The skill's layout and write rules are owned by
//! [`openlogi_core::agent_skill`]; this page only reports its state and calls
//! it. All file I/O runs on the background executor, never in `render`.

use std::path::PathBuf;

use openlogi_core::agent_skill::{self, Outcome, Presence, SkillError, SkillTarget};

use super::{
    App, Button, Context, Disableable as _, Entity, IconName, ParentElement, SettingField,
    SettingGroup, SettingItem, SettingPage, SettingsView, SharedString, Sizable as _, Styled, Tag,
    Window, div, h_flex, v_flex,
};
use crate::ui::theme::Typography as _;

/// What the page can do to the installed skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SkillAction {
    Install,
    /// Overwrite a differing file.
    Reinstall,
    Remove,
}

/// A probed snapshot of the skill for one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SkillStatus {
    Located {
        path: PathBuf,
        presence: Presence,
    },
    /// The path could not be resolved or read.
    Unavailable(String),
}

/// The page's view state: the last probe and the last action's outcome.
#[derive(Debug, Default)]
pub(super) struct SkillPanel {
    /// `None` until the first probe lands.
    pub(super) status: Option<SkillStatus>,
    pub(super) busy: bool,
    /// The last action's result, shown inline until the next action.
    pub(super) result: Option<Result<Outcome, String>>,
}

/// Blocking: read where the skill lives and whether it is installed.
pub(super) fn probe(target: &SkillTarget) -> SkillStatus {
    let located = agent_skill::skill_path(target).and_then(|path| {
        let presence = agent_skill::presence_at(&path)?;
        Ok(SkillStatus::Located { path, presence })
    });
    located.unwrap_or_else(|error: SkillError| SkillStatus::Unavailable(error.to_string()))
}

/// Blocking: perform `action`, then re-probe.
pub(super) fn perform(
    target: &SkillTarget,
    action: SkillAction,
) -> (Result<Outcome, String>, SkillStatus) {
    let result = match action {
        SkillAction::Install => agent_skill::install(target, false),
        SkillAction::Reinstall => agent_skill::install(target, true),
        SkillAction::Remove => agent_skill::uninstall(target, false),
    }
    .map(|(_, outcome)| outcome)
    .map_err(|error| error.to_string());
    (result, probe(target))
}

impl SettingsView {
    /// Probe the skill off the UI thread.
    pub(super) fn refresh_skill(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async { probe(&SkillTarget::Claude) })
                .await;
            this.update(cx, |this, cx| {
                this.skill.status = Some(status);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn run_skill_action(&mut self, action: SkillAction, cx: &mut Context<Self>) {
        if self.skill.busy {
            return;
        }
        self.skill.busy = true;
        self.skill.result = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (result, status) = cx
                .background_executor()
                .spawn(async move { perform(&SkillTarget::Claude, action) })
                .await;
            this.update(cx, |this, cx| {
                this.skill.busy = false;
                this.skill.result = Some(result);
                this.skill.status = Some(status);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

pub(super) fn ai_agent_page(view: Entity<SettingsView>) -> SettingPage {
    SettingPage::new(tr!("ai_agent.ai_agent"))
        .icon(IconName::Bot)
        .resettable(false)
        .description(tr!("ai_agent.ai_agent_description"))
        .group(
            SettingGroup::new().item(
                SettingItem::new(
                    tr!("ai_agent.claude_code_skill"),
                    SettingField::render(move |_, window, cx| skill_field(&view, window, cx)),
                )
                .layout(super::Axis::Vertical),
            ),
        )
}

fn status_line(status: Option<&SkillStatus>) -> (Option<Tag>, SharedString) {
    match status {
        None => (None, tr!("ai_agent.skill_checking")),
        Some(SkillStatus::Unavailable(error)) => (
            None,
            tr!("ai_agent.skill_path_unavailable", error => error.clone()),
        ),
        Some(SkillStatus::Located { path, presence }) => {
            let path = path.display().to_string();
            match presence {
                Presence::Current => (
                    Some(Tag::success().child(tr!("ai_agent.skill_installed_tag"))),
                    tr!("ai_agent.skill_installed", path => path),
                ),
                Presence::Absent => (None, tr!("ai_agent.skill_not_installed", path => path)),
                Presence::Differs => (
                    Some(Tag::warning().child(tr!("ai_agent.skill_differs_tag"))),
                    tr!("ai_agent.skill_differs", path => path),
                ),
            }
        }
    }
}

pub(super) fn outcome_message(result: &Result<Outcome, String>) -> (bool, SharedString) {
    match result {
        Ok(Outcome::Installed) => (true, tr!("ai_agent.skill_installed_done")),
        Ok(Outcome::Unchanged) => (true, tr!("ai_agent.skill_already_current")),
        Ok(Outcome::Replaced) => (true, tr!("ai_agent.skill_replaced_done")),
        Ok(Outcome::Removed) => (true, tr!("ai_agent.skill_removed_done")),
        Ok(Outcome::Absent) => (true, tr!("ai_agent.skill_nothing_to_remove")),
        Err(error) => (false, tr!("ai_agent.skill_failed", error => error.clone())),
    }
}

fn skill_field(view: &Entity<SettingsView>, _: &mut Window, cx: &mut App) -> gpui::Div {
    let pal = crate::ui::theme::palette(cx);
    let (status, busy, result) = {
        let panel = &view.read(cx).skill;
        (panel.status.clone(), panel.busy, panel.result.clone())
    };
    let (tag, line) = status_line(status.as_ref());
    let presence = match &status {
        Some(SkillStatus::Located { presence, .. }) => Some(*presence),
        _ => None,
    };
    let enabled = !busy && presence.is_some();
    let button = |id: &'static str, label: SharedString, action: SkillAction| {
        let view = view.clone();
        Button::new(id)
            .outline()
            .small()
            .label(label)
            .disabled(!enabled)
            .on_click(move |_, _, cx| {
                view.update(cx, |this, cx| this.run_skill_action(action, cx));
            })
    };

    let mut buttons = h_flex().gap_2();
    match presence {
        Some(Presence::Absent) | None => {
            buttons = buttons.child(button(
                "skill-install",
                tr!("ai_agent.skill_install"),
                SkillAction::Install,
            ));
        }
        Some(Presence::Current) => {
            buttons = buttons
                .child(button(
                    "skill-reinstall",
                    tr!("ai_agent.skill_reinstall"),
                    SkillAction::Reinstall,
                ))
                .child(button(
                    "skill-remove",
                    tr!("ai_agent.skill_remove"),
                    SkillAction::Remove,
                ));
        }
        Some(Presence::Differs) => {
            buttons = buttons.child(button(
                "skill-reinstall",
                tr!("ai_agent.skill_reinstall"),
                SkillAction::Reinstall,
            ));
        }
    }

    let mut col = v_flex().w_full().gap_2().child(
        h_flex()
            .w_full()
            .items_center()
            .gap_2()
            .children(tag)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_caption()
                    .text_color(pal.text_muted)
                    .child(line),
            ),
    );
    col = col.child(buttons);
    if let Some(result) = result {
        let (ok, message) = outcome_message(&result);
        col = col.child(
            div()
                .text_caption()
                .text_color(if ok { pal.text_muted } else { pal.text_primary })
                .child(message),
        );
    }
    col
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(dir: &tempfile::TempDir) -> SkillTarget {
        SkillTarget::Dir(dir.path().join("skills").join("openlogi"))
    }

    #[test]
    fn install_reinstall_and_remove_track_the_probe() {
        let dir = tempfile::tempdir().unwrap();
        let target = target(&dir);
        assert!(matches!(
            probe(&target),
            SkillStatus::Located {
                presence: Presence::Absent,
                ..
            }
        ));

        let (result, status) = perform(&target, SkillAction::Install);
        assert_eq!(result, Ok(Outcome::Installed));
        assert!(matches!(
            status,
            SkillStatus::Located {
                presence: Presence::Current,
                ..
            }
        ));

        let SkillStatus::Located { path, .. } = status else {
            unreachable!()
        };
        std::fs::write(&path, "mine").unwrap();
        assert!(matches!(
            probe(&target),
            SkillStatus::Located {
                presence: Presence::Differs,
                ..
            }
        ));
        // A plain remove must not delete a file that is not ours.
        let (result, _) = perform(&target, SkillAction::Remove);
        result.unwrap_err();
        assert!(path.exists());

        let (result, _) = perform(&target, SkillAction::Reinstall);
        assert_eq!(result, Ok(Outcome::Replaced));
        let (result, status) = perform(&target, SkillAction::Remove);
        assert_eq!(result, Ok(Outcome::Removed));
        assert!(matches!(
            status,
            SkillStatus::Located {
                presence: Presence::Absent,
                ..
            }
        ));
    }
}
