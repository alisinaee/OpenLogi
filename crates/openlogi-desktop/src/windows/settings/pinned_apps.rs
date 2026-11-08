//! The app switcher's Pinned row, for Settings → General: the apps that stay
//! at the top of the switcher, in order. Pin an app by searching the installed
//! ones (or picking one seen in front recently), move it up or down, or unpin
//! it.

use appcatalog::{ApplicationIdentity, IdentityKind};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, Task, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::{Disableable as _, IconName, Sizable as _, h_flex, v_flex};
use openlogi_core::switcher_rows::MAX_PINNED;

use crate::state::{AppState, StateEvent};
use crate::ui::components::{control_input, localize_placeholder};
use crate::ui::theme::{self, Typography as _};

/// Suggestions listed under the search box.
const MAX_SUGGESTIONS: usize = 6;

/// One installed application: display name and the identifier the switcher
/// matches on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Installed {
    pub(crate) name: String,
    pub(crate) id: String,
}

/// The installed applications, listed on the background executor. Empty when
/// the platform cannot list them (the failure is logged).
pub(crate) async fn discover_installed(cx: &gpui::AsyncApp) -> Vec<Installed> {
    let found = cx
        .background_executor()
        .spawn(async { appcatalog::applications() })
        .await;
    match found {
        Ok(applications) => installed_from(&applications),
        Err(error) => {
            tracing::warn!(%error, "failed to list apps to pin");
            Vec::new()
        }
    }
}

pub(crate) struct PinnedAppsPanel {
    search: Entity<InputState>,
    installed: Vec<Installed>,
    _search_obs: Subscription,
    _state_obs: Subscription,
    _discovery: Task<()>,
}

impl PinnedAppsPanel {
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
            _search_obs: search_obs,
            _state_obs: state_obs,
            _discovery: discovery,
        }
    }
}

/// The identifier the switcher matches `application` by: the bundle identifier
/// on macOS, the first one the platform gave elsewhere.
fn installed_from(applications: &[appcatalog::Application]) -> Vec<Installed> {
    let mut installed: Vec<Installed> = applications
        .iter()
        .filter_map(|application| {
            let identity = application
                .identities
                .iter()
                .find(|identity| identity.kind() == IdentityKind::MacBundleIdentifier)
                .or_else(|| application.identities.first())
                .map(ApplicationIdentity::value)?;
            Some(Installed {
                name: application.name.clone(),
                id: identity.to_string(),
            })
        })
        .collect();
    installed.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    installed.dedup_by(|a, b| a.id == b.id);
    installed
}

/// `pinned` with the app at `from` moved to `to`; unchanged when either is out
/// of range.
fn moved(pinned: &[String], from: usize, to: usize) -> Vec<String> {
    let mut next = pinned.to_vec();
    if from < next.len() && to < next.len() {
        let app = next.remove(from);
        next.insert(to, app);
    }
    next
}

/// `pinned` without the app at `index`.
fn removed(pinned: &[String], index: usize) -> Vec<String> {
    let mut next = pinned.to_vec();
    if index < next.len() {
        next.remove(index);
    }
    next
}

/// The apps to offer for `query`: matches among the installed apps, or the
/// recently used ones while the query is empty; never one already pinned.
pub(crate) fn suggestions(
    query: &str,
    installed: &[Installed],
    recent: &[Installed],
    pinned: &[String],
) -> Vec<Installed> {
    let query = query.trim().to_lowercase();
    let source = if query.is_empty() { recent } else { installed };
    source
        .iter()
        .filter(|app| !pinned.contains(&app.id))
        .filter(|app| {
            query.is_empty()
                || app.name.to_lowercase().contains(&query)
                || app.id.to_lowercase().contains(&query)
        })
        .take(MAX_SUGGESTIONS)
        .cloned()
        .collect()
}

fn commit(cx: &mut gpui::App, apps: Vec<String>) {
    AppState::apply(cx, |state| state.commit_pinned_apps(apps));
}

/// What a pinned row carries while it is dragged: the row it came from.
#[derive(Clone)]
struct DraggedPin {
    from: usize,
    name: SharedString,
}

impl Render for DraggedPin {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme::accent_tint())
            .child(self.name.clone())
    }
}

impl Render for PinnedAppsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::palette(cx);
        localize_placeholder(&self.search, tr!("app.pinned_apps_search"), window, cx);
        let (pinned, recent) = AppState::try_read(cx).map_or_else(
            || (Vec::new(), Vec::new()),
            |state| {
                let recent = state
                    .recent_apps()
                    .map(|(id, name)| Installed {
                        id: id.to_string(),
                        name: name.to_string(),
                    })
                    .collect();
                (state.app_settings().pinned_apps.clone(), recent)
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
        let query = self.search.read(cx).value().to_string();
        let full = pinned.len() >= MAX_PINNED;
        let offered = if full {
            Vec::new()
        } else {
            suggestions(&query, &self.installed, &recent, &pinned)
        };
        let last = pinned.len().saturating_sub(1);

        let rows = pinned.iter().enumerate().map(|(index, id)| {
            let name = name_of(id);
            let show_id = name.as_ref() != id.as_str();
            let list = pinned.clone();
            let (up, down, remove, drop) = (list.clone(), list.clone(), list.clone(), list);
            let dragged = DraggedPin {
                from: index,
                name: name.clone(),
            };
            h_flex()
                .id(("pinned-row", index))
                .gap_2()
                .items_center()
                .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                .drag_over::<DraggedPin>(|style, _, _, _| style.bg(theme::accent_tint()))
                .on_drop(move |dragged: &DraggedPin, _, cx| {
                    commit(cx, moved(&drop, dragged.from, index));
                })
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_body().text_color(pal.text_primary).child(name))
                        .when_some(show_id.then(|| id.clone()), |this, id| {
                            this.child(div().text_caption().text_color(pal.text_muted).child(id))
                        }),
                )
                .child(
                    Button::new(("pinned-up", index))
                        .ghost()
                        .small()
                        .icon(IconName::ArrowUp)
                        .disabled(index == 0)
                        .tooltip(tr!("app.pinned_apps_move_up"))
                        .on_click(move |_, _, cx| commit(cx, moved(&up, index, index - 1))),
                )
                .child(
                    Button::new(("pinned-down", index))
                        .ghost()
                        .small()
                        .icon(IconName::ArrowDown)
                        .disabled(index == last)
                        .tooltip(tr!("app.pinned_apps_move_down"))
                        .on_click(move |_, _, cx| commit(cx, moved(&down, index, index + 1))),
                )
                .child(
                    Button::new(("pinned-remove", index))
                        .ghost()
                        .small()
                        .icon(IconName::Close)
                        .tooltip(tr!("app.pinned_apps_remove"))
                        .on_click(move |_, _, cx| commit(cx, removed(&remove, index))),
                )
        });

        let picks = offered.into_iter().enumerate().map(|(index, app)| {
            let mut next = pinned.clone();
            next.push(app.id.clone());
            Button::new(("pinned-add", index))
                .ghost()
                .small()
                .icon(IconName::Plus)
                .label(app.name)
                .on_click(move |_, _, cx| commit(cx, next.clone()))
        });

        v_flex()
            .gap_3()
            .w_full()
            .when(pinned.is_empty(), |this| {
                this.child(
                    div()
                        .text_caption()
                        .text_color(pal.text_muted)
                        .child(tr!("app.pinned_apps_empty")),
                )
            })
            .children(rows)
            .child(div().h(px(1.)).w_full().bg(pal.border))
            .child(if full {
                div()
                    .text_caption()
                    .text_color(pal.text_muted)
                    .child(tr!("app.pinned_apps_full", count => MAX_PINNED))
                    .into_any_element()
            } else {
                v_flex()
                    .gap_2()
                    .child(control_input(&self.search))
                    .child(v_flex().items_start().children(picks))
                    .into_any_element()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Installed, moved, removed, suggestions};

    fn app(name: &str, id: &str) -> Installed {
        Installed {
            name: name.into(),
            id: id.into(),
        }
    }

    fn ids(apps: &[&str]) -> Vec<String> {
        apps.iter().map(|app| (*app).to_string()).collect()
    }

    #[test]
    fn moving_swaps_places_and_ignores_out_of_range() {
        let pinned = ids(&["a", "b", "c"]);
        assert_eq!(moved(&pinned, 2, 1), ids(&["a", "c", "b"]));
        assert_eq!(moved(&pinned, 0, 1), ids(&["b", "a", "c"]));
        assert_eq!(moved(&pinned, 0, 3), pinned);
    }

    #[test]
    fn removing_drops_one_and_ignores_out_of_range() {
        let pinned = ids(&["a", "b"]);
        assert_eq!(removed(&pinned, 0), ids(&["b"]));
        assert_eq!(removed(&pinned, 5), pinned);
    }

    #[test]
    fn suggestions_match_name_or_id_and_skip_pinned() {
        let installed = [
            app("Zen Browser", "app.zen-browser.zen"),
            app("Arc", "company.thebrowser.Browser"),
            app("Safari", "com.apple.Safari"),
        ];
        let pinned = ids(&["com.apple.Safari"]);
        let found = suggestions("browser", &installed, &[], &pinned);
        assert_eq!(
            found.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Zen Browser", "Arc"]
        );
        assert!(suggestions("safari", &installed, &[], &pinned).is_empty());
    }

    #[test]
    fn an_empty_query_offers_the_recent_apps() {
        let installed = [app("Arc", "company.thebrowser.Browser")];
        let recent = [app("Notes", "com.apple.Notes")];
        let found = suggestions("  ", &installed, &recent, &[]);
        assert_eq!(found, recent);
    }
}
