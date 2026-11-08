//! The app switcher's rows, for Settings → General: switch each row on or off,
//! give each row an allow list ("Only show") and a block list ("Hide"), and
//! keep a list of apps no row shows ("Never show"). Apps are added by searching
//! the installed ones or picking one seen in front recently.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, Task, Window, div,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::{IconName, Selectable as _, Sizable as _, h_flex, v_flex};
use openlogi_core::switcher_rows::{RowPolicy, SwitcherRow, SwitcherRowRules};

use super::pinned_apps::{Installed, discover_installed, suggestions};
use crate::state::{AppState, StateEvent};
use crate::ui::components::{Toggle, control_input, localize_placeholder, toggle_row};
use crate::ui::theme::{self, Typography as _};

/// The most suggestions listed under the search box.
const MAX_SUGGESTIONS: usize = 6;

/// One list the user edits: a row's allow or block list, or the list no row
/// shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum List {
    /// The row shows only these apps, when the list is not empty.
    Allow(SwitcherRow),
    /// The row never shows these apps.
    Block(SwitcherRow),
    /// No row shows these apps.
    Never,
}

fn policy(rules: &SwitcherRowRules, row: SwitcherRow) -> &RowPolicy {
    rules.policy(row)
}

fn policy_mut(rules: &mut SwitcherRowRules, row: SwitcherRow) -> &mut RowPolicy {
    match row {
        SwitcherRow::Windows => &mut rules.windows,
        SwitcherRow::Pinned => &mut rules.pinned,
        SwitcherRow::Recent => &mut rules.recent,
        SwitcherRow::MostUsed => &mut rules.most_used,
    }
}

/// The identifiers `list` holds under `rules`.
fn list_of(rules: &SwitcherRowRules, list: List) -> &[String] {
    match list {
        List::Allow(row) => &policy(rules, row).allow,
        List::Block(row) => &policy(rules, row).block,
        List::Never => &rules.never_show,
    }
}

fn list_mut(rules: &mut SwitcherRowRules, list: List) -> &mut Vec<String> {
    match list {
        List::Allow(row) => &mut policy_mut(rules, row).allow,
        List::Block(row) => &mut policy_mut(rules, row).block,
        List::Never => &mut rules.never_show,
    }
}

/// `rules` with `id` added to `list`; unchanged when it is already there.
fn with_added(rules: &SwitcherRowRules, list: List, id: &str) -> SwitcherRowRules {
    let mut next = rules.clone();
    let target = list_mut(&mut next, list);
    if !target.iter().any(|existing| existing == id) {
        target.push(id.to_owned());
    }
    next
}

/// `rules` without the entry at `index` of `list`; unchanged when out of range.
fn without(rules: &SwitcherRowRules, list: List, index: usize) -> SwitcherRowRules {
    let mut next = rules.clone();
    let target = list_mut(&mut next, list);
    if index < target.len() {
        target.remove(index);
    }
    next
}

/// `rules` with `row` switched on or off.
fn with_enabled(rules: &SwitcherRowRules, row: SwitcherRow, enabled: bool) -> SwitcherRowRules {
    let mut next = rules.clone();
    policy_mut(&mut next, row).enabled = enabled;
    next
}

/// The settings label of a row.
fn row_name(row: SwitcherRow) -> SharedString {
    match row {
        SwitcherRow::Windows => tr!("app.switcher_row_windows"),
        SwitcherRow::Pinned => tr!("app.switcher_row_pinned"),
        SwitcherRow::Recent => tr!("app.switcher_row_recent"),
        SwitcherRow::MostUsed => tr!("app.switcher_row_most_used"),
    }
}

/// A stable key for a row's element ids.
const fn row_key(row: SwitcherRow) -> &'static str {
    match row {
        SwitcherRow::Windows => "windows",
        SwitcherRow::Pinned => "pinned",
        SwitcherRow::Recent => "recent",
        SwitcherRow::MostUsed => "most-used",
    }
}

/// The selector a row's toggle is found by in layout tests.
const fn row_selector(row: SwitcherRow) -> &'static str {
    match row {
        SwitcherRow::Windows => "app-switcher-row-windows",
        SwitcherRow::Pinned => "app-switcher-row-pinned",
        SwitcherRow::Recent => "app-switcher-row-recent",
        SwitcherRow::MostUsed => "app-switcher-row-most-used",
    }
}

/// The name of the list the user is editing, and the row it belongs to.
fn list_names(list: List) -> (SharedString, SharedString) {
    match list {
        List::Allow(row) => (tr!("app.switcher_only_show"), row_name(row)),
        List::Block(row) => (tr!("app.switcher_hide"), row_name(row)),
        List::Never => (tr!("app.switcher_never_show"), tr!("app.switcher_all_rows")),
    }
}

/// `rules` with `id` moved from `from` to `to`, keeping the order of the other
/// entries. Dropping an app onto the list it came from changes nothing.
fn moved_between(rules: &SwitcherRowRules, from: List, to: List, id: &str) -> SwitcherRowRules {
    if from == to {
        return rules.clone();
    }
    let mut next = rules.clone();
    list_mut(&mut next, from).retain(|existing| existing != id);
    let target = list_mut(&mut next, to);
    if !target.iter().any(|existing| existing == id) {
        target.push(id.to_owned());
    }
    next
}

/// What an app chip carries while it is dragged: the list it came from.
#[derive(Clone)]
struct DraggedApp {
    from: List,
    id: String,
    name: SharedString,
}

impl Render for DraggedApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme::accent_tint())
            .child(self.name.clone())
    }
}

pub(crate) struct AppSwitcherRowsPanel {
    search: Entity<InputState>,
    installed: Vec<Installed>,
    /// The list new apps are being added to, when the picker is open.
    target: Option<List>,
    _search_obs: Subscription,
    _state_obs: Subscription,
    _discovery: Task<()>,
}

impl AppSwitcherRowsPanel {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx));
        let search_obs = cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let state_obs = AppState::repaint_on(cx, |event| {
            matches!(
                event,
                StateEvent::SettingsChanged | StateEvent::AgentChanged
            )
        });
        let discovery = cx.spawn(async move |panel, cx| {
            let installed = discover_installed(cx).await;
            panel
                .update(cx, |panel, cx| {
                    panel.installed = installed;
                    cx.notify();
                })
                .ok();
        });
        Self {
            search,
            installed: Vec::new(),
            target: None,
            _search_obs: search_obs,
            _state_obs: state_obs,
            _discovery: discovery,
        }
    }

    /// One editable list: its heading, the add button, and a chip per app that
    /// removes it when pressed.
    fn list_block(
        &self,
        cx: &mut Context<Self>,
        pal: theme::Palette,
        list: List,
        rules: &SwitcherRowRules,
        name_of: &dyn Fn(&str) -> SharedString,
    ) -> impl IntoElement + use<> {
        let items = list_of(rules, list).to_vec();
        let chips = items
            .iter()
            .enumerate()
            .map(|(index, id)| {
                let next = without(rules, list, index);
                let dragged = DraggedApp {
                    from: list,
                    id: id.clone(),
                    name: name_of(id),
                };
                div()
                    .id(format!("rule-chip-{list:?}-{index}"))
                    .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                    .child(
                        Button::new(format!("rule-remove-{list:?}-{index}"))
                            .ghost()
                            .small()
                            .icon(IconName::Close)
                            .label(name_of(id))
                            .on_click(move |_, _, cx| {
                                AppState::apply(cx, |state| state.commit_app_switcher_rules(next.clone()));
                            }),
                    )
            })
            .collect::<Vec<_>>();
        let (title, _) = list_names(list);
        let body = if items.is_empty() {
            div()
                .text_caption()
                .text_color(pal.text_muted)
                .child(tr!("app.switcher_empty_list"))
                .into_any_element()
        } else {
            h_flex()
                .flex_wrap()
                .gap_1()
                .children(chips)
                .into_any_element()
        };
        v_flex()
            .gap_1()
            .id(format!("rule-list-{list:?}"))
            .drag_over::<DraggedApp>(|style, _, _, _| style.bg(theme::accent_tint()))
            .on_drop({
                let base = rules.clone();
                move |dragged: &DraggedApp, _, cx| {
                    let next = moved_between(&base, dragged.from, list, &dragged.id);
                    AppState::apply(cx, |state| state.commit_app_switcher_rules(next));
                }
            })
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_caption()
                            .text_color(pal.text_muted)
                            .child(title),
                    )
                    .child(
                        Button::new(format!("rule-add-{list:?}"))
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .label(tr!("app.switcher_add"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.target = Some(list);
                                cx.notify();
                            })),
                    ),
            )
            .child(body)
    }

    /// The picker for the open list: the search box and the apps it offers.
    fn picker(
        &self,
        cx: &mut Context<Self>,
        pal: theme::Palette,
        list: List,
        rules: &SwitcherRowRules,
        recent: &[Installed],
    ) -> impl IntoElement + use<> {
        let query = self.search.read(cx).value().to_string();
        let existing = list_of(rules, list).to_vec();
        let offered = suggestions(&query, &self.installed, recent, &existing);
        let (list_name, row) = list_names(list);
        let offers: Vec<Button> = offered
            .into_iter()
            .take(MAX_SUGGESTIONS)
            .enumerate()
            .map(|(index, app)| {
                let next = with_added(rules, list, &app.id);
                Button::new(format!("rule-pick-{list:?}-{index}"))
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .label(app.name)
                    .on_click(move |_, _, cx| {
                        AppState::apply(cx, |state| state.commit_app_switcher_rules(next.clone()));
                    })
            })
            .collect();
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_body()
                            .text_color(pal.text_primary)
                            .child(tr!("app.switcher_add_to", list => list_name.to_string(), row => row.to_string())),
                    )
                    .child(
                        Button::new("rule-picker-done")
                            .ghost()
                            .small()
                            .label(tr!("app.switcher_done"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.target = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(control_input(&self.search))
            .child(v_flex().items_start().children(offers))
    }
}

impl Render for AppSwitcherRowsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::palette(cx);
        localize_placeholder(&self.search, tr!("app.switcher_search"), window, cx);
        let (rules, recent) = AppState::try_read(cx).map_or_else(
            || (SwitcherRowRules::default(), Vec::new()),
            |state| {
                let recent: Vec<Installed> = state
                    .recent_apps()
                    .map(|(id, name)| Installed {
                        id: id.to_string(),
                        name: name.to_string(),
                    })
                    .collect();
                (state.app_settings().app_switcher_rules.clone(), recent)
            },
        );
        let name_of = |id: &str| -> SharedString {
            self.installed
                .iter()
                .chain(&recent)
                .find(|app| app.id == id)
                .map_or_else(|| id.to_string(), |app| app.name.clone())
                .into()
        };

        let rows = SwitcherRow::ALL
            .into_iter()
            .map(|row| {
                let enabled = policy(&rules, row).enabled;
                let base = rules.clone();
                let toggle = Toggle::new(format!("switcher-row-{}", row_key(row)))
                    .selected(enabled)
                    .on_change(move |on, _, cx| {
                        let next = with_enabled(&base, row, *on);
                        AppState::apply(cx, |state| state.commit_app_switcher_rules(next));
                    });
                let mut section = v_flex().gap_2().child(toggle_row(
                    row_selector(row),
                    pal,
                    row_name(row),
                    SharedString::default(),
                    toggle,
                ));
                if enabled {
                    section = section
                        .child(self.list_block(cx, pal, List::Allow(row), &rules, &name_of))
                        .child(self.list_block(cx, pal, List::Block(row), &rules, &name_of));
                }
                section
            })
            .collect::<Vec<_>>();

        let picker = self
            .target
            .map(|list| self.picker(cx, pal, list, &rules, &recent));

        v_flex()
            .gap_4()
            .w_full()
            .children(rows)
            .child(div().h(gpui::px(1.)).w_full().bg(pal.border))
            .child(self.list_block(cx, pal, List::Never, &rules, &name_of))
            .when_some(picker, |this, picker| {
                this.child(div().h(gpui::px(1.)).w_full().bg(pal.border))
                    .child(picker)
            })
    }
}

#[cfg(test)]
mod tests {
    use openlogi_core::switcher_rows::{SwitcherRow, SwitcherRowRules};

    use super::{List, list_of, moved_between, with_added, with_enabled, without};

    #[test]
    fn dragging_moves_app_between_lists_once() {
        let rules = with_added(
            &SwitcherRowRules::default(),
            List::Allow(SwitcherRow::Recent),
            "a",
        );
        let moved = moved_between(&rules, List::Allow(SwitcherRow::Recent), List::Never, "a");
        assert!(list_of(&moved, List::Allow(SwitcherRow::Recent)).is_empty());
        assert_eq!(list_of(&moved, List::Never), ["a"]);
        let dropped_again = moved_between(&moved, List::Never, List::Never, "a");
        assert_eq!(dropped_again, moved);
    }

    #[test]
    fn adding_appends_once() {
        let rules = SwitcherRowRules::default();
        let once = with_added(&rules, List::Block(SwitcherRow::Recent), "com.a");
        let twice = with_added(&once, List::Block(SwitcherRow::Recent), "com.a");
        assert_eq!(list_of(&twice, List::Block(SwitcherRow::Recent)), ["com.a"]);
        assert!(list_of(&twice, List::Allow(SwitcherRow::Recent)).is_empty());
    }

    #[test]
    fn removing_drops_one_and_ignores_out_of_range() {
        let rules = with_added(
            &with_added(&SwitcherRowRules::default(), List::Never, "a"),
            List::Never,
            "b",
        );
        let removed = without(&rules, List::Never, 0);
        assert_eq!(list_of(&removed, List::Never), ["b"]);
        assert_eq!(without(&rules, List::Never, 9), rules);
    }

    #[test]
    fn switching_a_row_leaves_its_lists_alone() {
        let rules = with_added(
            &SwitcherRowRules::default(),
            List::Allow(SwitcherRow::Pinned),
            "a",
        );
        let off = with_enabled(&rules, SwitcherRow::Pinned, false);
        assert!(!off.policy(SwitcherRow::Pinned).enabled);
        assert_eq!(list_of(&off, List::Allow(SwitcherRow::Pinned)), ["a"]);
        assert!(off.policy(SwitcherRow::Recent).enabled);
    }
}
