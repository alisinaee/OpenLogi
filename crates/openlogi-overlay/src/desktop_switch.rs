//! The desktop switcher: one box per desktop of the pointer's display, the
//! current one highlighted, shown after Previous or Next Desktop.
//!
//! A box is a bare number, or, when the agent sent a [`DesktopSchema`], a small
//! map of its desktop: an outline per window at its scaled frame, tinted per
//! app, with the icons of the apps that take the most room beside the number.
//! A full-screen desktop shows its app's icon and name instead. The schema
//! never holds window titles, and a missing or unreadable one just leaves the
//! numbers.
//!
//! The agent decides what shows and for how long (a
//! [`NoticeKind::DesktopSwitch`] notice that lives for the hold plus the fade);
//! this module only draws it. A newer switch updates the open window in place
//! and restarts the fade, so rolling through several desktops moves the
//! highlight without the window closing and opening again.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, AnyWindowHandle, App, AppContext as _, Context,
    FontWeight, Hsla, IntoElement, ParentElement, Render, SharedString, Size, Styled, Window,
    WindowHandle, div, hsla, img, px, size,
};
use openlogi_core::desktop_schema::{DesktopSchema, MAX_ICONS, SchemaApp, tint_hue};
use openlogi_ipc::{DESKTOP_SWITCH_FADE, DESKTOP_SWITCH_HOLD, Notice, NoticeKind};
use tracing::warn;

use crate::platform::NoticePlacement;

/// The most boxes drawn. macOS offers sixteen desktops at most.
const MAX_BOXES: u16 = 16;
const BOX_SIZE: f32 = 40.0;
const BOX_GAP: f32 = 8.0;
/// Padding of the card around the boxes, and clear space for its shadow.
const CARD_PAD: f32 = 14.0;
const INSET: f32 = 12.0;

/// A mapped box: the map of the windows, then a row with the number and icons.
const MAP_W: f32 = 96.0;
const MAP_H: f32 = 60.0;
const TILE_PAD: f32 = 6.0;
const TILE_W: f32 = MAP_W + 2.0 * TILE_PAD;
const ROW_H: f32 = 22.0;
const TILE_H: f32 = TILE_PAD + MAP_H + 4.0 + ROW_H + TILE_PAD;
const ICON: f32 = 18.0;
const BIG_ICON: f32 = 30.0;
/// A window outline is never thinner than this, so a sliver stays visible.
const MIN_OUTLINE: f32 = 3.0;
/// Past this many desktops the mapped row would outgrow a display, so the
/// boxes stay plain numbers.
const MAX_MAPPED_BOXES: u16 = 10;

const CARD: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.10,
    a: 0.94,
};
const IDLE_BOX: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.24,
    a: 1.0,
};
const IDLE_TEXT: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.70,
    a: 1.0,
};
const CURRENT_BOX: Hsla = Hsla {
    h: 0.58,
    s: 0.75,
    l: 0.56,
    a: 1.0,
};
/// Behind a desktop's window outlines.
const MAP_BACKDROP: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.15,
    a: 1.0,
};
/// Behind an idle box's number, in a mapped box.
const IDLE_TEXT_BACKING: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.32,
    a: 1.0,
};
const CURRENT_TEXT: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 1.0,
    a: 1.0,
};

/// What the switcher shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Switcher {
    /// The notice's id: new for every switch, so each one restarts the fade.
    pub(crate) serial: u64,
    pub(crate) count: u16,
    /// The current desktop, counted from 1.
    pub(crate) current: u16,
    /// What each desktop holds, when the agent could read it.
    pub(crate) schema: Option<Arc<DesktopSchema>>,
}

/// Take the switcher out of the agent's notices. Returns the newest one and
/// the other notices, which are cards for the notice shelf.
pub(crate) fn split(notices: Vec<Notice>) -> (Option<Switcher>, Vec<Notice>) {
    let mut switcher = None;
    let mut cards = Vec::with_capacity(notices.len());
    for notice in notices {
        match notice.kind {
            NoticeKind::DesktopSwitch {
                count,
                current,
                schema,
            } => {
                switcher = Some(Switcher {
                    serial: notice.id,
                    count,
                    current,
                    schema: schema.map(Arc::new),
                });
            }
            NoticeKind::LowBattery { .. } => cards.push(notice),
        }
    }
    (switcher, cards)
}

/// The window's opacity `elapsed` after a switch: fully visible for the hold,
/// then fading linearly to nothing.
fn opacity(elapsed: Duration) -> f32 {
    let into_fade = elapsed.saturating_sub(DESKTOP_SWITCH_HOLD);
    (1.0 - into_fade.as_secs_f32() / DESKTOP_SWITCH_FADE.as_secs_f32()).clamp(0.0, 1.0)
}

fn boxes(switcher: &Switcher) -> u16 {
    switcher.count.clamp(1, MAX_BOXES)
}

/// Whether the boxes map their desktops.
fn mapped(switcher: &Switcher) -> bool {
    switcher.schema.is_some() && boxes(switcher) <= MAX_MAPPED_BOXES
}

/// What decides the window's size: two switchers with the same shape fit the
/// same window, so the open one is updated in place. Whether a report carries
/// the schema is deliberately not part of it: a roll mixes mapped and plain
/// reports, and the window must never be replaced mid-roll. A row that can be
/// mapped always gets the mapped size; its plain boxes sit centred inside it.
fn shape(switcher: &Switcher) -> (u16, bool) {
    let boxes = boxes(switcher);
    (boxes, boxes <= MAX_MAPPED_BOXES)
}

/// The width and height of one box.
fn tile_size(mapped: bool) -> (f32, f32) {
    if mapped {
        (TILE_W, TILE_H)
    } else {
        (BOX_SIZE, BOX_SIZE)
    }
}

fn window_size((boxes, mapped): (u16, bool)) -> Size<gpui::Pixels> {
    let boxes = f32::from(boxes);
    let (tile_w, tile_h) = tile_size(mapped);
    size(
        px(boxes * tile_w + (boxes - 1.0) * BOX_GAP + 2.0 * (CARD_PAD + INSET)),
        px(tile_h + 2.0 * (CARD_PAD + INSET)),
    )
}

/// One window of a desktop's map.
#[derive(Clone, Debug, PartialEq)]
struct Outline {
    hue: f32,
    /// `[x, y, width, height]` in the map, in points.
    rect: [f32; 4],
}

/// What a mapped box draws besides its number.
#[derive(Clone, Debug, PartialEq)]
enum Contents {
    /// Window outlines, back to front, and the icons of the biggest apps.
    Windows {
        outlines: Vec<Outline>,
        icons: Vec<Option<PathBuf>>,
    },
    /// An app's full-screen desktop.
    Fullscreen { name: String, icon: Option<PathBuf> },
}

/// What desktop `index` (from 0) draws. An empty desktop is an empty map.
fn contents(schema: &DesktopSchema, index: usize) -> Contents {
    let top = schema.top_apps(index, MAX_ICONS);
    let icon_of = |app: &SchemaApp| app.icon.as_ref().map(PathBuf::from);
    let space = schema.spaces.get(index);
    if space.is_some_and(|space| space.fullscreen)
        && let Some(app) = top.first()
    {
        return Contents::Fullscreen {
            name: app.name.clone(),
            icon: icon_of(app),
        };
    }
    let outlines = space
        .map(|space| {
            // The schema is front-most first; painted back to front.
            space
                .windows
                .iter()
                .rev()
                .filter_map(|window| {
                    let app = schema.app(window.app)?;
                    Some(Outline {
                        hue: tint_hue(&app.id),
                        rect: window.frame.scaled(MAP_W, MAP_H, MIN_OUTLINE),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Contents::Windows {
        outlines,
        icons: top.into_iter().map(icon_of).collect(),
    }
}

fn icon_element(path: &Path, side: f32) -> AnyElement {
    img(path.to_path_buf())
        .w(px(side))
        .h(px(side))
        .into_any_element()
}

/// One mapped box.
fn mapped_tile(number: u16, is_current: bool, contents: &Contents) -> impl IntoElement {
    let text = if is_current { CURRENT_TEXT } else { IDLE_TEXT };
    let face = match contents {
        Contents::Windows { outlines, .. } => div()
            .relative()
            .w(px(MAP_W))
            .h(px(MAP_H))
            .rounded(px(4.0))
            .overflow_hidden()
            .bg(MAP_BACKDROP)
            .children(outlines.iter().map(|outline| {
                let [x, y, width, height] = outline.rect;
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(width))
                    .h(px(height))
                    .rounded(px(2.0))
                    .border_1()
                    .border_color(hsla(outline.hue, 0.70, 0.66, 0.9))
                    .bg(hsla(outline.hue, 0.70, 0.55, 0.38))
            })),
        Contents::Fullscreen { name, icon } => div()
            .w(px(MAP_W))
            .h(px(MAP_H))
            .rounded(px(4.0))
            .bg(MAP_BACKDROP)
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(3.0))
            .children(icon.as_ref().map(|icon| icon_element(icon, BIG_ICON)))
            .child(
                div()
                    .max_w(px(MAP_W - 8.0))
                    .truncate()
                    .text_size(px(10.0))
                    .text_color(IDLE_TEXT)
                    .child(SharedString::from(name.clone())),
            ),
    };
    let icons: &[Option<PathBuf>] = match contents {
        Contents::Windows { icons, .. } => icons,
        Contents::Fullscreen { .. } => &[],
    };
    div()
        .w(px(TILE_W))
        .h(px(TILE_H))
        .p(px(TILE_PAD))
        .flex()
        .flex_col()
        .gap(px(4.0))
        .rounded(px(8.0))
        .border_2()
        .border_color(if is_current { CURRENT_BOX } else { IDLE_BOX })
        .bg(IDLE_BOX)
        .child(face)
        .child(
            div()
                .h(px(ROW_H))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .min_w(px(ROW_H))
                        .h(px(ROW_H - 4.0))
                        .px(px(5.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(7.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(12.0))
                        .bg(if is_current {
                            CURRENT_BOX
                        } else {
                            IDLE_TEXT_BACKING
                        })
                        .text_color(text)
                        .child(number.to_string()),
                )
                .children(
                    icons
                        .iter()
                        .filter_map(|icon| icon.as_ref().map(|icon| icon_element(icon, ICON))),
                ),
        )
}

/// The switcher window's view.
pub(crate) struct SwitcherView {
    switcher: Switcher,
}

impl Render for SwitcherView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let (serial, current) = (self.switcher.serial, self.switcher.current);
        let schema = self
            .switcher
            .schema
            .clone()
            .filter(|_| mapped(&self.switcher));
        let row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(BOX_GAP))
            .p(px(CARD_PAD))
            .rounded(px(14.0))
            .bg(CARD)
            .shadow_lg()
            .children((1..=boxes(&self.switcher)).map(|number| {
                let is_current = number == current;
                if let Some(schema) = &schema {
                    let contents = contents(schema, usize::from(number - 1));
                    return mapped_tile(number, is_current, &contents).into_any_element();
                }
                div()
                    .w(px(BOX_SIZE))
                    .h(px(BOX_SIZE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .bg(if is_current { CURRENT_BOX } else { IDLE_BOX })
                    .text_color(if is_current { CURRENT_TEXT } else { IDLE_TEXT })
                    .child(number.to_string())
                    .into_any_element()
            }));
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(row)
            // A new serial is a new animation: each switch holds the box fully
            // visible again before it fades.
            .with_animation(
                ("desktop-switch", serial),
                Animation::new(DESKTOP_SWITCH_HOLD + DESKTOP_SWITCH_FADE),
                |element, progress| {
                    let total = DESKTOP_SWITCH_HOLD + DESKTOP_SWITCH_FADE;
                    element.opacity(opacity(total.mul_f32(progress)))
                },
            )
    }
}

fn open_windows(cx: &App) -> Vec<WindowHandle<SwitcherView>> {
    cx.windows()
        .iter()
        .filter_map(AnyWindowHandle::downcast::<SwitcherView>)
        .collect()
}

/// Show `shown`, or close the switcher when it is `None`.
///
/// A switch that leaves the number of desktops as it was updates the open
/// window in place; otherwise the window is replaced.
pub(crate) fn present(cx: &mut App, shown: Option<Switcher>) {
    let open = open_windows(cx);
    let Some(switcher) = shown else {
        for view in open {
            let _ = view.update(cx, |_, window, _| window.remove_window());
        }
        return;
    };
    let mut reused = false;
    for view in open {
        let same_row = view
            .read(cx)
            .is_ok_and(|view| shape(&view.switcher) == shape(&switcher));
        if same_row && !reused {
            let next = switcher.clone();
            reused = view
                .update(cx, |view, _, cx| {
                    view.switcher = next;
                    cx.notify();
                })
                .is_ok();
        } else {
            let _ = view.update(cx, |_, window, _| window.remove_window());
        }
    }
    if reused {
        return;
    }
    // Centred like the system app switcher; Windows keeps its taskbar-anchored notice spot.
    #[cfg(not(target_os = "windows"))]
    let mut placement = NoticePlacement::capture_centred(cx, window_size(shape(&switcher)));
    #[cfg(target_os = "windows")]
    let mut placement = match NoticePlacement::capture(cx, window_size(shape(&switcher))) {
        Ok(placement) => placement,
        Err(error) => {
            warn!(%error, "could not locate the desktop switcher display");
            return;
        }
    };
    let alternatives = placement
        .openings()
        .into_iter()
        .map(|options| vec![options])
        .collect();
    let opened = crate::platform::open_first(alternatives, |options| {
        cx.open_window(options, |_, cx| {
            cx.new(|_| SwitcherView {
                switcher: switcher.clone(),
            })
        })
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
        warn!(%error, "could not position the desktop switcher window");
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

#[cfg(test)]
mod tests {
    use openlogi_core::desktop_schema::{SchemaWindow, SpaceSchema, UnitRect};
    use openlogi_core::device::{BatteryInfo, BatteryLevel, BatteryStatus, DeviceKind};

    use super::*;

    fn low_battery(id: u64) -> Notice {
        Notice {
            id,
            kind: NoticeKind::LowBattery {
                name: None,
                kind: DeviceKind::Mouse,
                battery: BatteryInfo {
                    percentage: Some(10),
                    level: BatteryLevel::Low,
                    status: BatteryStatus::Discharging,
                },
            },
        }
    }

    fn switch(id: u64, count: u16, current: u16) -> Notice {
        Notice {
            id,
            kind: NoticeKind::DesktopSwitch {
                count,
                current,
                schema: None,
            },
        }
    }

    #[test]
    fn the_switcher_is_kept_apart_from_the_cards() {
        let (switcher, cards) = split(vec![low_battery(1), switch(2, 4, 3), switch(3, 4, 2)]);
        assert_eq!(
            switcher,
            Some(Switcher {
                serial: 3,
                count: 4,
                current: 2,
                schema: None,
            }),
            "the newest switch shows"
        );
        assert_eq!(cards, vec![low_battery(1)]);
        assert_eq!(split(Vec::new()), (None, Vec::new()));
    }

    #[test]
    fn it_holds_then_fades_to_nothing() {
        assert!((opacity(Duration::ZERO) - 1.0).abs() < f32::EPSILON);
        assert!((opacity(DESKTOP_SWITCH_HOLD) - 1.0).abs() < f32::EPSILON);
        let midway = opacity(DESKTOP_SWITCH_HOLD + DESKTOP_SWITCH_FADE / 2);
        assert!((midway - 0.5).abs() < 0.01, "{midway}");
        assert!(opacity(DESKTOP_SWITCH_HOLD + DESKTOP_SWITCH_FADE).abs() < f32::EPSILON);
    }

    fn plain(count: u16) -> Switcher {
        Switcher {
            serial: 1,
            count,
            current: 2,
            schema: None,
        }
    }

    #[test]
    fn the_row_never_exceeds_the_boxes_macos_offers() {
        assert_eq!(boxes(&plain(40)), MAX_BOXES);
        assert_eq!(boxes(&plain(0)), 1);
    }

    fn app(id: &str, name: &str, icon: Option<&str>) -> SchemaApp {
        SchemaApp {
            id: id.into(),
            name: name.into(),
            icon: icon.map(str::to_owned),
        }
    }

    fn window(app: u16, x: u16, y: u16, width: u16, height: u16) -> SchemaWindow {
        SchemaWindow {
            app,
            frame: UnitRect {
                x,
                y,
                width,
                height,
            },
        }
    }

    fn schema() -> DesktopSchema {
        DesktopSchema {
            apps: vec![
                app("com.a", "A", Some("/icons/a.png")),
                app("com.b", "B", None),
            ],
            spaces: vec![
                SpaceSchema {
                    fullscreen: false,
                    windows: vec![window(1, 500, 0, 500, 1000), window(0, 0, 0, 600, 1000)],
                },
                SpaceSchema {
                    fullscreen: true,
                    windows: vec![window(1, 0, 0, 1000, 1000)],
                },
                SpaceSchema::default(),
            ],
        }
    }

    #[test]
    fn a_desktop_maps_its_windows_back_to_front_with_the_biggest_icons() {
        let Contents::Windows { outlines, icons } = contents(&schema(), 0) else {
            panic!("a windowed desktop maps its windows");
        };
        assert_eq!(outlines.len(), 2);
        // Painted back to front: the schema's last (rear) window first.
        assert_eq!(outlines[0].rect, [0.0, 0.0, 0.6 * MAP_W, MAP_H]);
        assert_eq!(outlines[1].rect, [0.5 * MAP_W, 0.0, 0.5 * MAP_W, MAP_H]);
        assert!((outlines[0].hue - tint_hue("com.a")).abs() < f32::EPSILON);
        assert_eq!(icons, vec![Some(PathBuf::from("/icons/a.png")), None]);
    }

    #[test]
    fn a_fullscreen_desktop_shows_its_app() {
        assert_eq!(
            contents(&schema(), 1),
            Contents::Fullscreen {
                name: "B".into(),
                icon: None
            }
        );
    }

    #[test]
    fn an_empty_or_unknown_desktop_is_an_empty_map() {
        for index in [2, 9] {
            assert_eq!(
                contents(&schema(), index),
                Contents::Windows {
                    outlines: Vec::new(),
                    icons: Vec::new()
                }
            );
        }
    }

    #[test]
    fn a_window_naming_an_app_outside_the_table_is_not_drawn() {
        let mut hostile = schema();
        hostile.spaces[0].windows.push(window(77, 0, 0, 1000, 1000));
        let Contents::Windows { outlines, .. } = contents(&hostile, 0) else {
            panic!("windowed");
        };
        assert_eq!(outlines.len(), 2);
    }

    #[test]
    fn consecutive_reports_with_and_without_schema_reuse_one_window() {
        let with = Switcher {
            schema: Some(Arc::new(schema())),
            ..plain(4)
        };
        for count in 1..=MAX_MAPPED_BOXES + 3 {
            let (plain_report, mapped_report) = (
                Switcher {
                    count,
                    ..plain(count)
                },
                Switcher {
                    count,
                    ..with.clone()
                },
            );
            assert_eq!(shape(&plain_report), shape(&mapped_report), "{count}");
            assert_eq!(
                window_size(shape(&plain_report)),
                window_size(shape(&mapped_report))
            );
        }
    }

    #[test]
    fn mapped_boxes_size_the_window_and_too_many_stay_plain() {
        let mut mapped_switcher = plain(4);
        mapped_switcher.schema = Some(Arc::new(schema()));
        assert_eq!(shape(&mapped_switcher), (4, true));
        assert_eq!(
            shape(&plain(4)),
            (4, true),
            "schema presence keeps the size"
        );
        let wide = Switcher {
            count: MAX_MAPPED_BOXES + 1,
            ..mapped_switcher
        };
        assert_eq!(shape(&wide), (MAX_MAPPED_BOXES + 1, false));
        let (plain_w, mapped_w) = (window_size((3, false)).width, window_size((3, true)).width);
        assert!(mapped_w > plain_w);
    }
}
