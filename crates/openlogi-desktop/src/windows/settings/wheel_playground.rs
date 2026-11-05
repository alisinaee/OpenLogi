//! The wheel playground, a Settings page for trying the scroll settings.
//!
//! A tall list of numbered rows sits beside the controls that shape the wheel:
//! smooth scrolling, direction, easing, duration, sensitivity and momentum.
//! The controls are the General page's own entities and `AppState` setters, so
//! a change here is the same change there. The agent already transforms the
//! real wheel, so scrolling the list shows how the current settings feel. The
//! readout counts the wheel events the list receives and shows the last delta.
//!
//! Strings are English only: the locale catalogs are not extended yet.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, Entity, InteractiveElement, IntoElement, ParentElement, Pixels, Point, Render,
    ScrollHandle, StatefulInteractiveElement, Styled, Window, div, point, px,
};
use gpui_component::input::InputState;
use gpui_component::select::SelectState;
use gpui_component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_component::slider::SliderState;
use gpui_component::{Disableable as _, IconName, Selectable as _, h_flex, v_flex};
use openlogi_core::config::{VerticalScrollSensitivity, WheelFeel};

use super::general::{SliderWithEntry, WheelEasingOption};
use crate::features::pointer::momentum::MomentumPanel;
use crate::state::AppState;
use crate::ui::commit_slider::SliderUnit;
use crate::ui::components::{Toggle, control_button, control_select, toggle_row};
use crate::ui::slider_row::{SliderRow, slider_with_entry};
use crate::ui::theme::{self, Typography as _};

/// Rows in the test surface.
const ROWS: usize = 200;
/// Height of one row.
const ROW_H: f32 = 32.;
/// Height of the test surface.
const SURFACE_H: f32 = 380.;
/// Stand-in line height for a wheel that reports lines rather than pixels.
const LINE_H: f32 = 16.;

/// The scrollable test surface and its live readout.
pub(super) struct WheelSurface {
    scroll: ScrollHandle,
    events: usize,
    last_delta: Option<Point<Pixels>>,
}

impl WheelSurface {
    pub(super) fn new() -> Self {
        Self {
            scroll: ScrollHandle::new(),
            events: 0,
            last_delta: None,
        }
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.events = 0;
        self.last_delta = None;
        cx.notify();
    }
}

impl Render for WheelSurface {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::palette(cx);
        let delta = self.last_delta.map_or_else(
            || "none yet".to_owned(),
            |d| format!("{:.1}, {:.1} px", f32::from(d.x), f32::from(d.y)),
        );
        let offset = f32::from(-self.scroll.offset().y);
        v_flex()
            .flex_1()
            .min_w(px(260.))
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        v_flex()
                            .child(
                                div()
                                    .text_caption()
                                    .text_color(pal.text_muted)
                                    .child(format!("Wheel events: {}", self.events)),
                            )
                            .child(
                                div()
                                    .text_caption()
                                    .text_color(pal.text_muted)
                                    .child(format!("Last delta: {delta}")),
                            )
                            .child(
                                div()
                                    .text_caption()
                                    .text_color(pal.text_muted)
                                    .child(format!("Scrolled: {offset:.0} px")),
                            ),
                    )
                    .child(
                        control_button("wheel-playground-reset")
                            .label("Reset surface")
                            .on_click(cx.listener(|this, _, _, cx| this.reset(cx))),
                    ),
            )
            .child(
                div()
                    .id("wheel-playground-surface")
                    .h(px(SURFACE_H))
                    .w_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .rounded_md()
                    .border_1()
                    .border_color(pal.border)
                    .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                        this.events += 1;
                        this.last_delta = Some(event.delta.pixel_delta(px(LINE_H)));
                        cx.notify();
                    }))
                    .children((1..=ROWS).map(|n| {
                        div()
                            .h(px(ROW_H))
                            .flex_shrink_0()
                            .px_3()
                            .flex()
                            .items_center()
                            .text_color(pal.text_primary)
                            .when(n % 2 == 0, |row| row.bg(pal.border.opacity(0.25)))
                            .child(format!("Row {n:03}"))
                    })),
            )
    }
}

/// A sensitivity-style slider row laid out like the General page's.
fn slider_row<T: SliderUnit + PartialEq>(
    title: &'static str,
    control: &SliderWithEntry,
    max: T,
    default: T,
    format: fn(T) -> String,
    cx: &gpui::App,
) -> SliderRow {
    let SliderWithEntry { slider, entry } = control;
    let value = T::from_slider(slider.read(cx).value().start());
    SliderRow::new(title, slider_with_entry(slider, entry))
        .value(format(value), [format(max)])
        .when(value == default, |row| row.marker("Default"))
}

/// The controls beside the surface.
fn controls(
    vertical_scroll: &SliderWithEntry,
    wheel_duration: &SliderWithEntry,
    wheel_easing: &Entity<SelectState<Vec<WheelEasingOption>>>,
    cx: &mut gpui::App,
) -> impl IntoElement {
    let pal = theme::palette(cx);
    let (smooth, inverted, invertible) =
        AppState::try_read(cx).map_or((false, false, false), |s| {
            (
                s.app_settings().smooth_scroll,
                s.current_invert_scroll(),
                s.current_scroll_inversion_available(),
            )
        });
    v_flex()
        .flex_1()
        .min_w(px(260.))
        .gap_4()
        .child(toggle_row(
            "wheel-playground-smooth",
            pal,
            "Smooth scrolling".into(),
            "Animate wheel steps instead of jumping.".into(),
            Toggle::new("wheel-playground-smooth-toggle")
                .selected(smooth)
                .on_change(|enabled, _, cx| {
                    AppState::apply(cx, |state| state.commit_smooth_scroll(*enabled));
                }),
        ))
        .child(toggle_row(
            "wheel-playground-invert",
            pal,
            "Invert scroll direction".into(),
            "Applies to the selected mouse.".into(),
            Toggle::new("wheel-playground-invert-toggle")
                .selected(inverted)
                .disabled(!invertible)
                .on_change(|inverted, _, cx| {
                    AppState::apply(cx, |state| state.commit_invert_scroll(*inverted));
                }),
        ))
        .child(
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_body()
                        .text_color(pal.text_primary)
                        .child("Easing"),
                )
                .child(
                    div().h_6().w(px(220.)).child(
                        control_select(wheel_easing)
                            .w(px(220.))
                            .menu_width(px(220.)),
                    ),
                ),
        )
        .child(slider_row(
            "Wheel duration",
            wheel_duration,
            WheelFeel::DURATION_MS_MAX,
            WheelFeel::DURATION_MS_DEFAULT,
            |value| format!("{value} ms"),
            cx,
        ))
        .child(slider_row(
            "Vertical scroll sensitivity",
            vertical_scroll,
            VerticalScrollSensitivity::MAX,
            VerticalScrollSensitivity::DEFAULT,
            |value| {
                format!(
                    "{value} ({:.1}\u{d7})",
                    VerticalScrollSensitivity::scroll_multiplier(value)
                )
            },
            cx,
        ))
}

/// The Playground page.
pub(super) fn wheel_playground_page(
    surface: Entity<WheelSurface>,
    vertical_scroll: (Entity<SliderState>, Entity<InputState>),
    wheel_duration: (Entity<SliderState>, Entity<InputState>),
    wheel_easing: Entity<SelectState<Vec<WheelEasingOption>>>,
    momentum: Entity<MomentumPanel>,
) -> SettingPage {
    let vertical_scroll = SliderWithEntry {
        slider: vertical_scroll.0,
        entry: vertical_scroll.1,
    };
    let wheel_duration = SliderWithEntry {
        slider: wheel_duration.0,
        entry: wheel_duration.1,
    };
    let test = SettingGroup::new()
        .title("Test the wheel")
        .description(
            "Scroll the list with your wheel. It feels the settings beside it as you change them.",
        )
        .item(SettingItem::render(move |_, _, cx| {
            h_flex()
                .w_full()
                .items_start()
                .gap_6()
                .flex_wrap()
                .child(surface.clone())
                .child(controls(
                    &vertical_scroll,
                    &wheel_duration,
                    &wheel_easing,
                    cx,
                ))
        }));
    let glide = SettingGroup::new()
        .title("Momentum scrolling")
        .item(SettingItem::render(move |_, _, _| momentum.clone()));
    SettingPage::new("Wheel playground")
        .icon(IconName::Settings)
        .resettable(false)
        .group(test)
        .group(glide)
}
