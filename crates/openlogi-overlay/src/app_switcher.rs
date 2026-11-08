//! The app switcher: its rows of windows and apps, drawn while the switcher
//! button is held and the mouse has moved.
//!
//! The agent decides what the rows hold and which entry the cursor is on; this
//! module only draws the [`SwitcherView`] it sends. Row 0 is the open windows,
//! one tile each. The rows below it are the pinned, recent and most used apps,
//! and an app that is not running is drawn faded. The window is centred like
//! the desktop switcher, and it closes when the agent stops sending the
//! notice, which it does when the press ends.

use std::path::PathBuf;

use gpui::{
    AnyWindowHandle, App, AppContext as _, Context, FontWeight, Hsla, IntoElement, ParentElement,
    Render, SharedString, Size, Styled, Window, WindowHandle, div, img, px, size,
};
use openlogi_core::switcher_rows::{SwitcherRow, SwitcherView};
use openlogi_ipc::{Notice, NoticeKind};
use tracing::warn;

use crate::platform::NoticePlacement;

const TILE_W: f32 = 88.0;
const TILE_H: f32 = 88.0;
const TILE_GAP: f32 = 8.0;
const ICON: f32 = 36.0;
const ROW_LABEL_H: f32 = 16.0;
const ROW_GAP: f32 = 12.0;
const CARD_PAD: f32 = 14.0;

const CARD: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.10,
    a: 0.94,
};
const TILE: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.20,
    a: 1.0,
};
const SELECTED: Hsla = Hsla {
    h: 0.58,
    s: 0.75,
    l: 0.56,
    a: 1.0,
};
const LABEL: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.62,
    a: 1.0,
};
const TEXT: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.92,
    a: 1.0,
};

/// Take the switcher out of the agent's notices. Returns the newest one, if
/// any is on show, and the other notices.
pub(crate) fn split(notices: Vec<Notice>) -> (Option<SwitcherView>, Vec<Notice>) {
    let mut shown = None;
    let mut rest = Vec::with_capacity(notices.len());
    for notice in notices {
        match notice.kind {
            NoticeKind::AppSwitcher { view } => shown = Some(view),
            _ => rest.push(notice),
        }
    }
    (shown, rest)
}

/// One tile of a row: what it names, its icon when the agent sent one, and
/// whether its app is running.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    icon: Option<PathBuf>,
    running: bool,
}

/// The entries of `row`, in the order the agent sent them.
fn entries(view: &SwitcherView, row: SwitcherRow) -> Vec<Entry> {
    let entry = |app: &openlogi_core::app::ForegroundApp, running: bool, title: &str| Entry {
        name: if title.is_empty() {
            app.display_name.clone()
        } else {
            title.to_owned()
        },
        icon: view.icons.get(&app.id).map(PathBuf::from),
        running,
    };
    match row {
        SwitcherRow::Windows => view
            .windows
            .iter()
            .map(|window| entry(&window.app, true, &window.title))
            .collect(),
        app_row => view
            .rows
            .row(app_row)
            .iter()
            .map(|app| entry(&app.app, app.open, ""))
            .collect(),
    }
}

/// The rows that hold at least one entry, top first, with their entries.
fn filled_rows(view: &SwitcherView) -> Vec<(SwitcherRow, Vec<Entry>)> {
    SwitcherRow::ALL
        .into_iter()
        .map(|row| (row, entries(view, row)))
        .filter(|(_, entries)| !entries.is_empty())
        .collect()
}

/// The size of the switcher: as wide as its longest row, and one labelled
/// band per filled row.
fn panel_size(view: &SwitcherView) -> Size<gpui::Pixels> {
    let rows = filled_rows(view);
    let widest = rows
        .iter()
        .map(|(_, entries)| entries.len())
        .max()
        .unwrap_or(1);
    let count = widest as f32;
    let bands = rows.len() as f32;
    size(
        px(count * TILE_W + (count - 1.0).max(0.0) * TILE_GAP + 2.0 * CARD_PAD),
        px(bands * (ROW_LABEL_H + TILE_H) + (bands - 1.0).max(0.0) * ROW_GAP + 2.0 * CARD_PAD),
    )
}

/// The label of a row, in the active locale.
fn row_label(row: SwitcherRow) -> SharedString {
    let key = match row {
        SwitcherRow::Windows => "app_switcher.windows",
        SwitcherRow::Pinned => "app_switcher.pinned",
        SwitcherRow::Recent => "app_switcher.recent",
        SwitcherRow::MostUsed => "app_switcher.most_used",
    };
    rust_i18n::t!(key).into_owned().into()
}

/// One tile: its icon, or the first letter of its name without one, and its
/// name under it. The selected tile is outlined in the accent colour.
fn tile(entry: &Entry, selected: bool) -> impl IntoElement {
    let face = match &entry.icon {
        Some(icon) => img(icon.clone()).w(px(ICON)).h(px(ICON)).into_any_element(),
        None => {
            let letter = entry
                .name
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default();
            div()
                .w(px(ICON))
                .h(px(ICON))
                .rounded(px(8.0))
                .bg(LABEL)
                .flex()
                .items_center()
                .justify_center()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(TEXT)
                .child(letter)
                .into_any_element()
        }
    };
    div()
        .w(px(TILE_W))
        .h(px(TILE_H))
        .p(px(6.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(4.0))
        .rounded(px(10.0))
        .border_2()
        .border_color(if selected { SELECTED } else { TILE })
        .bg(TILE)
        .opacity(if entry.running { 1.0 } else { 0.55 })
        .child(face)
        .child(
            div()
                .max_w(px(TILE_W - 12.0))
                .truncate()
                .text_size(px(11.0))
                .text_color(TEXT)
                .child(SharedString::from(entry.name.clone())),
        )
}

/// The switcher window's view: the rows of the last view the agent sent.
pub(crate) struct Panel {
    view: SwitcherView,
}

impl Render for Panel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let view = &self.view;
        let selected = view.row.zip(view.column);
        let bands = filled_rows(view)
            .into_iter()
            .map(|(row, entries)| {
                let tiles = entries.iter().enumerate().map(|(index, entry)| {
                    let is_selected = selected
                        .is_some_and(|(r, column)| r == row && usize::from(column) == index);
                    tile(entry, is_selected)
                });
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .h(px(ROW_LABEL_H))
                            .text_size(px(11.0))
                            .text_color(LABEL)
                            .child(row_label(row)),
                    )
                    .child(div().flex().flex_row().justify_center().gap(px(TILE_GAP)).children(tiles))
            })
            .collect::<Vec<_>>();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(ROW_GAP))
                    .items_center()
                    .p(px(CARD_PAD))
                    .rounded(px(14.0))
                    .bg(CARD)
                    .shadow_lg()
                    .children(bands),
            )
    }
}

fn open_windows(cx: &App) -> Vec<WindowHandle<Panel>> {
    cx.windows()
        .iter()
        .filter_map(AnyWindowHandle::downcast::<Panel>)
        .collect()
}

/// Show `shown`, or close the switcher when it is `None` or holds no entry.
///
/// A new view of the same size updates the open window in place, so moving
/// the cursor does not close and reopen it; a view of another size replaces it.
pub(crate) fn present(cx: &mut App, shown: Option<SwitcherView>) {
    let open = open_windows(cx);
    let Some(view) = shown.filter(|view| !filled_rows(view).is_empty()) else {
        for handle in open {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        return;
    };
    let wanted = panel_size(&view);
    let mut reused = false;
    for handle in open {
        let same_size = handle
            .read(cx)
            .is_ok_and(|panel| panel_size(&panel.view) == wanted);
        if same_size && !reused {
            let next = view.clone();
            reused = handle
                .update(cx, |panel, _, cx| {
                    panel.view = next;
                    cx.notify();
                })
                .is_ok();
        } else {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }
    if reused {
        return;
    }
    // Centred like the desktop switcher; Windows keeps its taskbar-anchored spot.
    #[cfg(not(target_os = "windows"))]
    let mut placement = NoticePlacement::capture_centred(cx, wanted);
    #[cfg(target_os = "windows")]
    let mut placement = match NoticePlacement::capture(cx, wanted) {
        Ok(placement) => placement,
        Err(error) => {
            warn!(%error, "could not locate the app switcher display");
            return;
        }
    };
    let alternatives = placement
        .openings()
        .into_iter()
        .map(|options| vec![options])
        .collect();
    let opened = crate::platform::open_first(alternatives, |options| {
        cx.open_window(options, |_, cx| cx.new(|_| Panel { view: view.clone() }))
    });
    let Some(handle) = opened else {
        return;
    };
    let placed = handle
        .update(cx, |_, window, cx| {
            placement.show(window).map(|()| window.bounds_changed(cx))
        })
        .and_then(std::convert::identity);
    if let Err(error) = placed {
        warn!(%error, "could not position the app switcher window");
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use openlogi_core::app::ForegroundApp;
    use openlogi_core::switcher_rows::{RowApp, SwitcherRows, SwitcherWindow};

    use super::*;

    fn app(id: &str) -> ForegroundApp {
        ForegroundApp {
            id: id.into(),
            display_name: id.to_uppercase(),
        }
    }

    fn view() -> SwitcherView {
        SwitcherView::new(
            vec![SwitcherWindow {
                id: 1,
                pid: 10,
                app: app("a"),
                title: String::new(),
            }],
            SwitcherRows {
                pinned: vec![RowApp {
                    app: app("p"),
                    open: false,
                }],
                recent: Vec::new(),
                most_used: Vec::new(),
            },
            BTreeMap::from([("a".to_owned(), "/icons/a.png".to_owned())]),
        )
    }

    #[test]
    fn the_newest_switcher_is_kept_apart_from_the_cards() {
        let notice = |id, kind| Notice { id, kind };
        let (shown, rest) = split(vec![
            notice(
                1,
                NoticeKind::AppSwitcher {
                    view: SwitcherView::default(),
                },
            ),
            notice(2, NoticeKind::AppSwitcher { view: view() }),
        ]);
        assert_eq!(shown, Some(view()), "the last one shows");
        assert!(rest.is_empty());
        assert_eq!(split(Vec::new()), (None, Vec::new()));
    }

    #[test]
    fn rows_without_entries_are_not_drawn() {
        let rows = filled_rows(&view());
        let kinds: Vec<_> = rows.iter().map(|(row, _)| *row).collect();
        assert_eq!(kinds, [SwitcherRow::Windows, SwitcherRow::Pinned]);
    }

    #[test]
    fn a_window_is_named_by_its_app_and_an_app_by_its_row_entry() {
        let rows = filled_rows(&view());
        assert_eq!(
            rows[0].1,
            [Entry {
                name: "A".into(),
                icon: Some(PathBuf::from("/icons/a.png")),
                running: true,
            }],
            "titles are empty, so the app's name stands in"
        );
        assert_eq!(
            rows[1].1,
            [Entry {
                name: "P".into(),
                icon: None,
                running: false,
            }],
            "a pinned app that is not running is drawn faded, with no icon sent"
        );
    }

    #[test]
    fn the_panel_is_as_wide_as_its_longest_row_and_as_tall_as_its_bands() {
        let one = panel_size(&view());
        let mut wide = view();
        wide.windows.extend((2..=5).map(|id| SwitcherWindow {
            id,
            pid: 10,
            app: app("a"),
            title: String::new(),
        }));
        let widened = panel_size(&wide);
        assert!(widened.width > one.width);
        assert!(one.height > px(2.0 * TILE_H), "two bands are drawn");
    }
}
