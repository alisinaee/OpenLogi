//! The desktop switcher: one numbered box per desktop of the pointer's display,
//! the current one highlighted, shown after Previous or Next Desktop.
//!
//! The agent decides what shows and for how long (a
//! [`NoticeKind::DesktopSwitch`] notice that lives for the hold plus the fade);
//! this module only draws it. A newer switch updates the open window in place
//! and restarts the fade, so rolling through several desktops moves the
//! highlight without the window closing and opening again.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyWindowHandle, App, AppContext as _, Context, FontWeight, Hsla,
    IntoElement, ParentElement, Render, Size, Styled, Window, WindowHandle, div, px, size,
};
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
const CURRENT_TEXT: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 1.0,
    a: 1.0,
};

/// What the switcher shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Switcher {
    /// The notice's id: new for every switch, so each one restarts the fade.
    pub(crate) serial: u64,
    pub(crate) count: u16,
    /// The current desktop, counted from 1.
    pub(crate) current: u16,
}

/// Take the switcher out of the agent's notices. Returns the newest one and
/// the other notices, which are cards for the notice shelf.
pub(crate) fn split(notices: Vec<Notice>) -> (Option<Switcher>, Vec<Notice>) {
    let mut switcher = None;
    let mut cards = Vec::with_capacity(notices.len());
    for notice in notices {
        match notice.kind {
            NoticeKind::DesktopSwitch { count, current } => {
                switcher = Some(Switcher {
                    serial: notice.id,
                    count,
                    current,
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

fn boxes(switcher: Switcher) -> u16 {
    switcher.count.clamp(1, MAX_BOXES)
}

fn window_size(boxes: u16) -> Size<gpui::Pixels> {
    let boxes = f32::from(boxes);
    size(
        px(boxes * BOX_SIZE + (boxes - 1.0) * BOX_GAP + 2.0 * (CARD_PAD + INSET)),
        px(BOX_SIZE + 2.0 * (CARD_PAD + INSET)),
    )
}

/// The switcher window's view.
pub(crate) struct SwitcherView {
    switcher: Switcher,
}

impl Render for SwitcherView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let Switcher {
            serial, current, ..
        } = self.switcher;
        let row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(BOX_GAP))
            .p(px(CARD_PAD))
            .rounded(px(14.0))
            .bg(CARD)
            .shadow_lg()
            .children((1..=boxes(self.switcher)).map(|number| {
                let is_current = number == current;
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
            .is_ok_and(|view| boxes(view.switcher) == boxes(switcher));
        if same_row && !reused {
            reused = view
                .update(cx, |view, _, cx| {
                    view.switcher = switcher;
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
    let mut placement = match NoticePlacement::capture(cx, window_size(boxes(switcher))) {
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
        cx.open_window(options, |_, cx| cx.new(|_| SwitcherView { switcher }))
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
            kind: NoticeKind::DesktopSwitch { count, current },
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
                current: 2
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

    #[test]
    fn the_row_never_exceeds_the_boxes_macos_offers() {
        let many = Switcher {
            serial: 1,
            count: 40,
            current: 2,
        };
        assert_eq!(boxes(many), MAX_BOXES);
        assert_eq!(boxes(Switcher { count: 0, ..many }), 1);
    }
}
