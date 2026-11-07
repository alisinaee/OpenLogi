//! The desktop switcher controls, shown in the thumb wheel inspector while the
//! wheel is bound to Previous / Next Desktop.

use gpui::{App, IntoElement, ParentElement, Styled, div};
use gpui_component::{
    Disableable as _, Selectable as _,
    radio::{Radio, RadioGroup},
    v_flex,
};
use openlogi_core::config::{AppSettings, DesktopSwitcherSize};

use crate::state::AppState;
use crate::ui::components::{Toggle, toggle_row};
use crate::ui::theme::{Palette, Typography as _};

/// The overlay that Previous and Next Desktop show: whether it is shown, its
/// apps, whether the desktop switch animates, and its size.
pub(super) fn desktop_switcher_panel(pal: Palette, cx: &App) -> impl IntoElement {
    let app: AppSettings = AppState::try_read(cx)
        .map(|state| state.app_settings().clone())
        .unwrap_or_default();
    let size_index = DesktopSwitcherSize::ALL
        .iter()
        .position(|choice| *choice == app.desktop_switcher_size)
        .unwrap_or_default();
    let sizes = RadioGroup::vertical("mouse-desktop-switcher-size")
        .child(
            Radio::new("mouse-desktop-switcher-size-small")
                .label(tr!("app.desktop_switcher_size_small")),
        )
        .child(
            Radio::new("mouse-desktop-switcher-size-medium")
                .label(tr!("app.desktop_switcher_size_medium")),
        )
        .child(
            Radio::new("mouse-desktop-switcher-size-large")
                .label(tr!("app.desktop_switcher_size_large")),
        )
        .child(
            Radio::new("mouse-desktop-switcher-size-extra-large")
                .label(tr!("app.desktop_switcher_size_extra_large")),
        )
        .child(
            Radio::new("mouse-desktop-switcher-size-maximum")
                .label(tr!("app.desktop_switcher_size_maximum")),
        )
        .selected_index(Some(size_index))
        .on_change(|index, _, cx| {
            if let Some(size) = DesktopSwitcherSize::ALL.get(*index).copied() {
                AppState::apply(cx, |state| state.commit_desktop_switcher_size(size));
            }
        });
    v_flex()
        .w_full()
        .p_3()
        .gap_3()
        .rounded_lg()
        .border_1()
        .border_color(pal.border)
        .bg(pal.panel)
        .child(
            div()
                .text_body()
                .text_color(pal.text_primary)
                .child(tr!("app.desktop_switcher_card_title")),
        )
        .child(toggle_row(
            "mouse-desktop-switcher-shown-row",
            pal,
            tr!("app.show_desktop_switcher"),
            tr!("app.show_desktop_switcher_description"),
            Toggle::new("mouse-desktop-switcher-shown-toggle")
                .selected(app.show_desktop_switcher)
                .on_change(|shown, _, cx| {
                    AppState::apply(cx, |state| state.commit_show_desktop_switcher(*shown));
                }),
        ))
        .child(toggle_row(
            "mouse-desktop-switcher-apps-row",
            pal,
            tr!("app.desktop_switcher_show_apps"),
            tr!("app.desktop_switcher_show_apps_description"),
            Toggle::new("mouse-desktop-switcher-apps-toggle")
                .selected(app.desktop_switcher_show_apps)
                .disabled(!app.show_desktop_switcher)
                .on_change(|apps, _, cx| {
                    AppState::apply(cx, |state| state.commit_desktop_switcher_show_apps(*apps));
                }),
        ))
        .child(toggle_row(
            "mouse-animate-desktop-switch-row",
            pal,
            tr!("app.animate_desktop_switch"),
            tr!("app.animate_desktop_switch_description"),
            Toggle::new("mouse-animate-desktop-switch-toggle")
                .selected(app.animate_desktop_switch)
                .on_change(|animate, _, cx| {
                    AppState::apply(cx, |state| state.commit_animate_desktop_switch(*animate));
                }),
        ))
        .child(
            v_flex()
                .gap_2()
                .child(div().text_body().child(tr!("app.desktop_switcher_size")))
                .child(
                    div()
                        .text_caption()
                        .text_color(pal.text_muted)
                        .child(tr!("app.desktop_switcher_size_description")),
                )
                .child(sizes),
        )
}
