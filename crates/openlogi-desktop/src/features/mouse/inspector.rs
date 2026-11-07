//! Fixed binding inspector for the Buttons workspace.

use crate::ui::theme::FocusRingStyle as _;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use gpui::{
    Context, Entity, FocusHandle, InteractiveElement, IntoElement, ParentElement, Role,
    StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _, px, rgb, svg,
};
use gpui_base::Button as BaseButton;
use gpui_component::{
    Disableable as _, Icon, IconName, Selectable as _, Sizable as _, button::Button, h_flex,
    input::InputState, scroll::ScrollableElement as _, slider::Slider, v_flex,
};
use openlogi_core::binding::{
    Action, AppSwitcherParams, ButtonId, GamingLayout, GestureDirection, LongPressBinding,
    default_binding, default_binding_on,
};
use openlogi_core::config::VerticalScrollSensitivity;
use openlogi_core::hid::Dpi;

use super::app_switcher::AppSwitcherControls;
use super::force_sensing::{PressForce, force_sensing_row};
use super::hotspots::MouseControlId;
use super::thumbwheel::ThumbwheelPreset;
use super::view::MouseModelView;
use crate::features::binding_editor::{
    ActionOffer, GESTURE_BUTTON_ICON, PickFn, action_icon_path, action_rows_with,
    application_editor, command_editor, dpi_preset_editor, editor_section, gesture_direction_icon,
};
use crate::state::{AppOverrides, AppState};
use crate::ui::action::localized_action_label;
use crate::ui::commit_slider::{CommitSlider, SliderRange};
use crate::ui::components::{MenuRow, Toggle, control_button, control_input, toggle_row};
use crate::ui::slider_row::SliderRow;
use crate::ui::theme::{self, ACCENT_BLUE, Palette, Typography as _};

pub(super) const INSPECTOR_W: f32 = 328.;

#[derive(Clone, Copy)]
pub(super) struct BindingInspectorData<'a> {
    pub selected: Option<MouseControlId>,
    pub gesture_direction: Option<GestureDirection>,
    pub bindings: &'a BTreeMap<ButtonId, Action>,
    /// The device's G-series layout, which names the stock functions of its
    /// G buttons. `None` for a device without one.
    pub layout: Option<&'a GamingLayout>,
    pub gesture_maps: &'a BTreeMap<ButtonId, BTreeMap<GestureDirection, Action>>,
    pub dpi_gestures: bool,
    /// The device reports raw wheel on Back/Forward, so holding one can
    /// scroll sideways (`Capabilities::virtual_thumbwheel`).
    pub virtual_thumbwheel: bool,
    /// Buttons that hold the wheel to scroll sideways, from the global profile.
    pub wheel_hold: &'a BTreeSet<ButtonId>,
    pub editing_app: Option<&'a str>,
    pub overridden: Option<AppOverrides<'a>>,
    /// The open application profile's main-wheel speed and whether it sets one.
    pub app_wheel_speed: Option<(VerticalScrollSensitivity, bool)>,
    /// The open application profile's DPI on this device and whether it sets one.
    pub app_dpi: Option<(Dpi, bool)>,
    /// The haptic panel's press-force row, present only while
    /// [`ForceSensingFacts::current`](super::force_sensing::ForceSensingFacts::current)
    /// says it shows.
    pub press_force: Option<PressForce<'a>>,
    pub extra_actions: &'a [Action],
    /// Catalog actions the picker leaves out.
    pub hidden_actions: &'a [Action],
    /// Buttons with a tap and a hold action in the open profile.
    pub long_presses: &'a BTreeMap<ButtonId, LongPressBinding>,
    /// Whether the selected button may take a hold action in the open
    /// profile ([`AppState::can_edit_hold`]).
    pub hold_editable: bool,
    /// The hold time slider of the selected button.
    pub hold_slider: Option<&'a CommitSlider<u16>>,
    /// The app switcher tunables of the selected button, with the controls
    /// that edit them, when it is bound to the app switcher in a profile that
    /// can tune it.
    pub app_switcher: Option<(&'a AppSwitcherControls, AppSwitcherParams)>,
}

/// The hold time slider's range, in milliseconds. The ends are the config's
/// own bounds ([`HoldThreshold`](openlogi_core::binding::HoldThreshold)).
pub(super) fn hold_range() -> SliderRange<u16> {
    SliderRange::new(
        openlogi_core::binding::HoldThreshold::MIN_MS,
        openlogi_core::binding::HoldThreshold::MAX_MS,
    )
    .step(100.)
}

/// A hold time in seconds to one decimal, such as `1.0`.
pub(super) fn hold_seconds(ms: u16) -> String {
    let tenths = (u32::from(ms) + 50) / 100;
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// A tap-and-hold button's card text: its tap action, then the hold time in
/// milliseconds and what holding does, such as `tap: Back / hold 500 ms: Copy`.
pub(super) fn tap_hold_label(tap: &str, binding: &LongPressBinding) -> gpui::SharedString {
    tr!(
        "actions.hold_summary",
        tap => tap.to_string(),
        ms => hold_millis(binding).to_string(),
        action => localized_action_label(binding.long()).to_string()
    )
}

/// A tap-and-hold button's hold time in whole milliseconds.
fn hold_millis(binding: &LongPressBinding) -> u16 {
    u16::try_from(binding.threshold().as_millis()).unwrap_or(u16::MAX)
}

/// Which action of a tap-and-hold button the open picker chooses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PickSlot {
    /// The action of a tap, or the button's only action.
    #[default]
    Tap,
    /// The action of a hold.
    Hold,
}

#[derive(Clone, Copy)]
pub(super) struct ActionPickerContext<'a> {
    pub open: bool,
    /// Which action the open picker chooses.
    pub slot: PickSlot,
    pub search: &'a Entity<InputState>,
    pub shortcut_input: &'a Entity<InputState>,
    pub application_input: &'a Entity<InputState>,
    pub command_input: &'a Entity<InputState>,
    pub shortcut_mode: ShortcutMode,
    /// Whether Record is waiting for the next key press.
    pub shortcut_recording: bool,
    /// The Record button's area, which holds focus while recording.
    pub shortcut_record_focus: &'a FocusHandle,
    pub invalid: InvalidDrafts,
    pub view: &'a Entity<MouseModelView>,
}

/// Which custom-action editors rejected their last Add, so each shows an
/// inline error until its text changes or the picker reopens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct InvalidDrafts {
    pub shortcut: Option<ShortcutError>,
    pub application: bool,
    pub command: bool,
}

/// Why the custom shortcut editor shows an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShortcutError {
    /// The typed text, or the recorded key, is not a chord a shortcut can
    /// send.
    Unrecognized,
    /// Record saw Shift with a digit or symbol key, which it cannot record
    /// (see [`Recording::ShiftedSymbol`]). Typing the chord still works.
    ///
    /// [`Recording::ShiftedSymbol`]: crate::features::binding_editor::Recording::ShiftedSymbol
    ShiftedSymbol,
}

/// Whether the custom shortcut editor records a tapped or a held chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShortcutMode {
    Tap,
    Hold,
}

impl ShortcutMode {
    fn action(self, combo: openlogi_core::binding::KeyCombo) -> Action {
        match self {
            Self::Tap => Action::CustomShortcut(combo),
            Self::Hold => Action::HoldShortcut(combo),
        }
    }
}

pub(super) fn binding_inspector(
    data: BindingInspectorData<'_>,
    picker: ActionPickerContext<'_>,
    cx: &Context<MouseModelView>,
) -> gpui::Div {
    let pal = theme::palette(cx);
    let body = match data.selected {
        None => empty_inspector(
            data.editing_app,
            data.overridden.map_or(0, AppOverrides::len),
            data.app_wheel_speed,
            data.app_dpi,
            pal,
            cx,
        ),
        Some(MouseControlId::ThumbwheelRotation) => thumbwheel_inspector(
            data.bindings,
            data.editing_app,
            data.overridden,
            picker,
            pal,
            cx,
        ),
        Some(MouseControlId::Button(button)) => button_inspector(button, &data, picker, pal, cx),
    };

    v_flex()
        .w(px(INSPECTOR_W))
        .h_full()
        .min_h_0()
        .flex_shrink_0()
        .border_l_1()
        .border_color(pal.border)
        .bg(pal.panel)
        .child(
            div()
                .id("button-inspector-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .p_4()
                .child(body),
        )
}

fn empty_inspector(
    app: Option<&str>,
    override_count: usize,
    app_wheel_speed: Option<(VerticalScrollSensitivity, bool)>,
    app_dpi: Option<(Dpi, bool)>,
    pal: Palette,
    cx: &Context<MouseModelView>,
) -> gpui::Div {
    let summary = match (app, override_count) {
        (Some(app), 0) => tr!(
            "profiles.app_profile_no_overrides",
            app => app.to_string()
        ),
        (Some(app), 1) => tr!(
            "profiles.app_profile_single_override",
            app => app.to_string()
        ),
        (Some(app), count) => tr!(
            "profiles.app_profile_override_count",
            app => app.to_string(),
            count => count.to_string()
        ),
        (None, _) => tr!("profiles.select_device_button_description"),
    };
    v_flex()
        .gap_3()
        .child(inspector_heading(
            tr!("actions.button_inspector"),
            None,
            pal,
        ))
        .child(div().text_body().text_color(pal.text_muted).child(summary))
        .when_some(app_wheel_speed, |panel, speed| {
            panel.child(app_wheel_speed_block(speed, pal))
        })
        .when_some(app_dpi, |panel, dpi| {
            panel.child(app_dpi_block(dpi, pal, cx))
        })
}

/// The open application profile's DPI: step buttons and the device's presets
/// write the profile's own DPI, and a way back to the device DPI once it has one.
fn app_dpi_block((dpi, own): (Dpi, bool), pal: Palette, cx: &Context<MouseModelView>) -> gpui::Div {
    let (values, presets) = AppState::try_read(cx).map_or_else(
        || (Vec::new(), Vec::new()),
        |state| {
            (
                state
                    .active_dpi_capabilities()
                    .map(|caps| caps.values().to_vec())
                    .unwrap_or_default(),
                state.dpi_presets(),
            )
        },
    );
    let lower = values.iter().rev().find(|value| **value < dpi).copied();
    let higher = values.iter().find(|value| **value > dpi).copied();
    let status = if own {
        tr!("pointer.app_dpi_own")
    } else {
        tr!("pointer.app_dpi_inherited")
    };
    let row = h_flex()
        .gap_2()
        .items_center()
        .flex_wrap()
        .child(
            Button::new("inspector-app-dpi-down")
                .small()
                .label("−")
                .disabled(lower.is_none())
                .on_click(move |_, _, cx| {
                    if let Some(next) = lower {
                        AppState::apply(cx, |state| state.commit_current_app_dpi(Some(next)));
                    }
                }),
        )
        .child(div().text_body().child(format!("{dpi}")))
        .child(
            Button::new("inspector-app-dpi-up")
                .small()
                .label("+")
                .disabled(higher.is_none())
                .on_click(move |_, _, cx| {
                    if let Some(next) = higher {
                        AppState::apply(cx, |state| state.commit_current_app_dpi(Some(next)));
                    }
                }),
        )
        .when(own, |row| {
            row.child(
                Button::new("inspector-app-dpi-device")
                    .small()
                    .label(tr!("pointer.app_dpi_use_device"))
                    .on_click(move |_, _, cx| {
                        AppState::apply(cx, |state| state.commit_current_app_dpi(None));
                    }),
            )
        });
    let chips = h_flex()
        .gap_1()
        .flex_wrap()
        .children(presets.into_iter().enumerate().map(|(idx, preset)| {
            Button::new(("inspector-app-dpi-preset", idx))
                .small()
                .label(format!("{preset}"))
                .on_click(move |_, _, cx| {
                    AppState::apply(cx, |state| state.commit_current_app_dpi(Some(preset)));
                })
        }));
    v_flex()
        .gap_2()
        .child(inspector_heading(tr!("pointer.app_dpi_title"), None, pal))
        .child(
            div()
                .text_caption()
                .text_color(pal.text_muted)
                .child(status),
        )
        .child(row)
        .child(chips)
}

/// The open application profile's main-wheel speed: step buttons that write
/// the profile's own speed, and a way back to the global speed once it has one.
fn app_wheel_speed_block(
    (speed, own): (VerticalScrollSensitivity, bool),
    pal: Palette,
) -> gpui::Div {
    let value = u8::from(speed);
    let (min, max) = (
        i16::from(u8::from(VerticalScrollSensitivity::MIN)),
        i16::from(u8::from(VerticalScrollSensitivity::MAX)),
    );
    let stepped = |delta: i16| {
        let next = (i16::from(value) + delta).clamp(min, max);
        u8::try_from(next)
            .ok()
            .and_then(|next| VerticalScrollSensitivity::try_from(next).ok())
    };
    let lower = stepped(-1);
    let higher = stepped(1);
    let status = if own {
        tr!("pointer.app_wheel_speed_own")
    } else {
        tr!("pointer.app_wheel_speed_inherited")
    };
    let row = h_flex()
        .gap_2()
        .items_center()
        .child(
            Button::new("inspector-app-wheel-speed-down")
                .small()
                .label("−")
                .disabled(lower.is_none())
                .on_click(move |_, _, cx| {
                    if let Some(next) = lower {
                        AppState::apply(cx, |state| state.commit_app_wheel_speed(Some(next)));
                    }
                }),
        )
        .child(div().text_body().child(value.to_string()))
        .child(
            Button::new("inspector-app-wheel-speed-up")
                .small()
                .label("+")
                .disabled(higher.is_none())
                .on_click(move |_, _, cx| {
                    if let Some(next) = higher {
                        AppState::apply(cx, |state| state.commit_app_wheel_speed(Some(next)));
                    }
                }),
        )
        .when(own, |row| {
            row.child(
                Button::new("inspector-app-wheel-speed-global")
                    .small()
                    .label(tr!("pointer.app_wheel_speed_use_global"))
                    .on_click(move |_, _, cx| {
                        AppState::apply(cx, |state| state.commit_app_wheel_speed(None));
                    }),
            )
        });
    v_flex()
        .gap_2()
        .child(inspector_heading(
            tr!("pointer.app_wheel_speed_title"),
            None,
            pal,
        ))
        .child(
            div()
                .text_caption()
                .text_color(pal.text_muted)
                .child(status),
        )
        .child(row)
}

#[expect(
    clippy::too_many_lines,
    reason = "the button inspector lays out its rows inline"
)]
fn button_inspector(
    button: ButtonId,
    data: &BindingInspectorData<'_>,
    picker: ActionPickerContext<'_>,
    pal: Palette,
    cx: &Context<MouseModelView>,
) -> gpui::Div {
    let gesture_map = data.gesture_maps.get(&button);
    let overridden = data
        .overridden
        .is_some_and(|overrides| overrides.contains_key(button));
    if data.editing_app.is_none()
        && let Some(gesture_map) = gesture_map
    {
        return gesture_inspector(button, gesture_map, data, picker, pal, cx);
    }
    if let Some(app) = data.editing_app
        && !overridden
        && gesture_map.is_some()
    {
        return inherited_gesture_inspector(button, app, data, picker, pal, cx);
    }

    let action = data
        .bindings
        .get(&button)
        .cloned()
        .unwrap_or_else(|| default_binding_on(data.layout, button));
    let long_press = data.long_presses.get(&button);
    let slot = if long_press.is_some() {
        picker.slot
    } else {
        PickSlot::Tap
    };
    let chosen = match (slot, long_press) {
        (PickSlot::Hold, Some(long_press)) => long_press.long().clone(),
        _ => action.clone(),
    };
    let observer = picker.view.clone();
    let on_pick: PickFn = Rc::new(move |action, _window, cx| {
        AppState::apply(cx, |state| match slot {
            PickSlot::Tap => state.commit_binding(button, action),
            PickSlot::Hold => state.commit_hold_action(button, action),
        });
        observer.update(cx, |view, cx| {
            view.close_action_picker();
            cx.notify();
        });
    });

    v_flex()
        .gap_3()
        .child(inspector_heading(
            tr!(button.translation_key()),
            Some(button_status(button, &action, data, overridden)),
            pal,
        ))
        .child(match long_press {
            Some(long_press) => tap_hold_editor(&action, long_press, data, picker, pal),
            None => current_action_card(&action, picker, pal).into_any_element(),
        })
        .when(action == Action::AppSwitcher, |panel| {
            panel.child(
                div()
                    .text_body()
                    .text_color(pal.text_muted)
                    .child(tr!("actions.app_switcher_description")),
            )
        })
        .children(
            data.app_switcher
                .zip(data.hold_slider)
                .map(|((controls, params), hold_slider)| controls.panel(params, hold_slider)),
        )
        .when(data.hold_editable, |panel| {
            panel.child(tap_hold_row(button, long_press.is_some(), picker.view, pal))
        })
        .children(wheel_hold_section(
            button,
            data,
            overridden,
            picker.view,
            pal,
        ))
        .when(overridden, |panel| {
            let observer = picker.view.clone();
            panel.child(
                control_button("inspector-use-default")
                    .w_full()
                    .icon(IconName::Undo)
                    .label(tr!("profiles.use_the_default_profile"))
                    .on_click(move |_, _, cx| {
                        AppState::apply(cx, |state| state.clear_app_binding(button));
                        observer.update(cx, |view, cx| {
                            view.close_action_picker();
                            cx.notify();
                        });
                    }),
            )
        })
        .when(can_enable_gestures(button, data.editing_app), |panel| {
            let observer = picker.view.clone();
            let unavailable = button == ButtonId::DpiToggle && !data.dpi_gestures;
            panel
                .child(
                    control_button("inspector-use-gestures")
                        .w_full()
                        .icon(Icon::empty().path(GESTURE_BUTTON_ICON))
                        .label(tr!("actions.use_gestures"))
                        .disabled(unavailable)
                        .on_click(move |_, _, cx| {
                            AppState::apply(cx, |state| state.commit_gesture_mode(button, true));
                            observer.update(cx, |view, cx| {
                                view.set_gesture_selected_dir(Some(GestureDirection::Click));
                                cx.notify();
                            });
                        }),
                )
                .when(unavailable, |panel| {
                    panel.child(
                        div()
                            .text_body()
                            .text_color(pal.text_muted)
                            .child(tr!("actions.dpi_gestures_unavailable")),
                    )
                })
        })
        .children(
            data.press_force
                .map(|press_force| force_sensing_row(press_force, pal)),
        )
        .when(picker.open, |panel| {
            panel.child(action_library(
                "inspector-action",
                Some(&chosen),
                picker,
                ActionOffer {
                    extra: data.extra_actions,
                    hidden: data.hidden_actions,
                },
                &on_pick,
                pal,
                cx,
            ))
        })
}

/// A tap-and-hold button's two action cards and its hold time.
fn tap_hold_editor(
    tap: &Action,
    long_press: &LongPressBinding,
    data: &BindingInspectorData<'_>,
    picker: ActionPickerContext<'_>,
    pal: Palette,
) -> gpui::AnyElement {
    let committed = hold_millis(long_press);
    v_flex()
        .gap_3()
        .child(selection_card(
            "inspector-tap-action",
            tr!("actions.tap_action"),
            action_icon_path(tap),
            localized_action_label(tap),
            PickSlot::Tap,
            picker,
            pal,
        ))
        .child(selection_card(
            "inspector-hold-action",
            tr!("actions.hold_action"),
            action_icon_path(long_press.long()),
            localized_action_label(long_press.long()),
            PickSlot::Hold,
            picker,
            pal,
        ))
        .children(data.hold_slider.map(|slider| {
            SliderRow::new(
                tr!("actions.hold_time"),
                Slider::new(slider.slider()).horizontal(),
            )
            .value(format!("{} ms", slider.shown(committed)), ["3000 ms"])
        }))
        .into_any_element()
}

/// The switch that gives a button a second action for holding it.
fn tap_hold_row(
    button: ButtonId,
    enabled: bool,
    view: &Entity<MouseModelView>,
    pal: Palette,
) -> impl IntoElement {
    let observer = view.clone();
    toggle_row(
        "inspector-tap-hold-row",
        pal,
        tr!("actions.tap_and_hold"),
        tr!("actions.tap_and_hold_description"),
        Toggle::new("inspector-tap-hold-toggle")
            .selected(enabled)
            .on_change(move |enabled, _window, cx| {
                AppState::apply(cx, |state| state.commit_tap_hold(button, *enabled));
                observer.update(cx, |view, cx| {
                    view.close_action_picker();
                    cx.notify();
                });
            }),
    )
}

/// The heading's status line. A button that holds the wheel is customized
/// even while its press keeps the default action.
fn button_status(
    button: ButtonId,
    action: &Action,
    data: &BindingInspectorData<'_>,
    overridden: bool,
) -> gpui::SharedString {
    let default =
        *action == default_binding_on(data.layout, button) && !data.wheel_hold.contains(&button);
    match (data.editing_app, overridden, default) {
        (Some(app), true, _) => tr!("actions.overridden_in_app", app => app.to_string()),
        (Some(_), false, _) => tr!("profiles.inherited_from_default"),
        (None, _, true) => tr!("pointer.device_default"),
        (None, _, false) => tr!("profiles.customized"),
    }
}

/// Back/Forward's wheel-hold control: the switch in the global profile, or,
/// in an app profile that has not overridden the button, a note that an
/// action picked there replaces it.
fn wheel_hold_section(
    button: ButtonId,
    data: &BindingInspectorData<'_>,
    overridden: bool,
    view: &Entity<MouseModelView>,
    pal: Palette,
) -> Option<gpui::AnyElement> {
    let enabled = data.wheel_hold.contains(&button);
    match data.editing_app {
        None if button.supports_wheel_hold() && (data.virtual_thumbwheel || enabled) => {
            Some(wheel_hold_row(button, enabled, view, pal).into_any_element())
        }
        Some(app) if enabled && !overridden => Some(
            div()
                .debug_selector(|| "inspector-wheel-hold-app-note".into())
                .text_caption()
                .text_color(pal.text_muted)
                .child(tr!(
                    "actions.app_profile_action_replaces_wheel_hold",
                    app => app.to_string()
                ))
                .into_any_element(),
        ),
        _ => None,
    }
}

/// Back/Forward's wheel-hold switch. Shown only in the global profile: a
/// per-app entry holds one action, so the mode is device-wide. Turning it
/// off stays possible on a device that does not report raw wheel, so a
/// binding carried over from another mouse can be removed.
///
/// The gesture inspector offers it too. Turning it on there leaves gesture
/// mode, stashing the direction map, so the selected direction is cleared
/// as "Use a single action" does.
fn wheel_hold_row(
    button: ButtonId,
    enabled: bool,
    view: &Entity<MouseModelView>,
    pal: Palette,
) -> impl IntoElement {
    let observer = view.clone();
    toggle_row(
        "inspector-wheel-hold-row",
        pal,
        tr!("actions.hold_to_scroll_sideways"),
        tr!("actions.hold_to_scroll_sideways_description"),
        Toggle::new("inspector-wheel-hold-toggle")
            .selected(enabled)
            .on_change(move |enabled, _window, cx| {
                AppState::apply(cx, |state| state.commit_wheel_hold(button, *enabled));
                if *enabled {
                    observer.update(cx, |view, cx| {
                        view.set_gesture_selected_dir(None);
                        cx.notify();
                    });
                }
            }),
    )
}

fn inherited_gesture_inspector(
    button: ButtonId,
    app: &str,
    data: &BindingInspectorData<'_>,
    picker: ActionPickerContext<'_>,
    pal: Palette,
    cx: &Context<MouseModelView>,
) -> gpui::Div {
    let observer = picker.view.clone();
    let on_pick: PickFn = Rc::new(move |action, _window, cx| {
        AppState::apply(cx, |state| state.commit_binding(button, action));
        observer.update(cx, |view, cx| {
            view.close_action_picker();
            cx.notify();
        });
    });
    let edit_default = picker.view.clone();
    v_flex()
        .gap_3()
        .child(inspector_heading(
            tr!(button.translation_key()),
            Some(tr!("profiles.inherited_from_default")),
            pal,
        ))
        .child(gesture_summary_card(picker, pal))
        .child(div().text_caption().text_color(pal.text_muted).child(tr!(
            "actions.app_profile_action_replaces_gestures",
            app => app.to_string()
        )))
        .child(
            Button::new("inspector-edit-default-gestures")
                .small()
                .w_full()
                .label(tr!("actions.edit_default_gestures"))
                .on_click(move |_, _, cx| {
                    AppState::apply(cx, |state| state.set_editing_app(None));
                    edit_default.update(cx, |view, cx| {
                        view.set_gesture_selected_dir(Some(GestureDirection::Click));
                        cx.notify();
                    });
                }),
        )
        .when(picker.open, |panel| {
            panel.child(action_library(
                "inspector-gesture-override",
                None,
                picker,
                // The pick replaces the gestures with a button binding,
                // which gets a release, so held actions fit here.
                ActionOffer {
                    extra: data.extra_actions,
                    hidden: data.hidden_actions,
                },
                &on_pick,
                pal,
                cx,
            ))
        })
}

fn gesture_inspector(
    button: ButtonId,
    gesture_map: &BTreeMap<GestureDirection, Action>,
    data: &BindingInspectorData<'_>,
    picker: ActionPickerContext<'_>,
    pal: Palette,
    cx: &Context<MouseModelView>,
) -> gpui::Div {
    let direction = data.gesture_direction.unwrap_or(GestureDirection::Click);
    // Gesture mode never holds the wheel, so the switch here is off and only
    // turning it on is useful, which needs measured raw wheel.
    let offer_wheel_hold = button.supports_wheel_hold() && data.virtual_thumbwheel;
    let current = gesture_action(gesture_map, button, direction);
    let observer = picker.view.clone();
    let on_pick: PickFn = Rc::new(move |action, _window, cx| {
        AppState::apply(cx, |state| {
            state.commit_gesture_binding(button, direction, action)
        });
        observer.update(cx, |view, cx| {
            view.close_action_picker();
            cx.notify();
        });
    });
    let turn_off = picker.view.clone();

    v_flex()
        .gap_3()
        .child(inspector_heading(
            tr!(button.translation_key()),
            Some(tr!("actions.five_directions")),
            pal,
        ))
        .child(gesture_directions(
            direction,
            gesture_map,
            button,
            picker.view,
            pal,
        ))
        .child(current_action_card(&current, picker, pal))
        .when(offer_wheel_hold, |panel| {
            panel.child(wheel_hold_row(button, false, picker.view, pal))
        })
        .child(
            control_button("inspector-single-action")
                .w_full()
                .label(tr!("actions.use_a_single_action"))
                .on_click(move |_, _, cx| {
                    AppState::apply(cx, |state| state.commit_gesture_mode(button, false));
                    turn_off.update(cx, |view, cx| {
                        view.set_gesture_selected_dir(None);
                        cx.notify();
                    });
                }),
        )
        .children(
            data.press_force
                .map(|press_force| force_sensing_row(press_force, pal)),
        )
        .when(picker.open, |panel| {
            panel.child(action_library(
                "inspector-gesture-action",
                Some(&current),
                picker,
                ActionOffer {
                    extra: &[],
                    hidden: data.hidden_actions,
                },
                &on_pick,
                pal,
                cx,
            ))
        })
}

fn gesture_directions(
    active: GestureDirection,
    gesture_map: &BTreeMap<GestureDirection, Action>,
    button: ButtonId,
    view: &Entity<MouseModelView>,
    pal: Palette,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(editor_section(tr!("actions.direction"), pal))
        .children(
            GestureDirection::ALL
                .into_iter()
                .enumerate()
                .map(|(index, direction)| {
                    let selected = direction == active;
                    let action = gesture_action(gesture_map, button, direction);
                    let view = view.clone();
                    let aria_label = format!(
                        "{}: {}",
                        tr!(direction.translation_key()),
                        localized_action_label(&action)
                    );
                    MenuRow::new(("inspector-direction", index))
                        .selected(selected)
                        .role(Role::Button)
                        .aria_label(aria_label)
                        .child(
                            h_flex()
                                .min_w_0()
                                .gap_2()
                                // `.size_4()` is not decoration: a bare `Icon`
                                // falls through to the current font size, which
                                // would leave these a step under the 16px leading
                                // column the action rows below use.
                                .child(gesture_direction_icon(direction).size_4())
                                .child(
                                    v_flex()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_body()
                                                .child(tr!(direction.translation_key())),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_caption()
                                                .text_color(pal.text_muted)
                                                .child(localized_action_label(&action)),
                                        ),
                                ),
                        )
                        .when(selected, |row| {
                            row.child(
                                Icon::new(IconName::Check)
                                    .size_3()
                                    .text_color(rgb(ACCENT_BLUE)),
                            )
                        })
                        .on_click(move |_, _, cx| {
                            view.update(cx, |view, cx| {
                                view.set_gesture_selected_dir(Some(direction));
                                cx.notify();
                            });
                        })
                }),
        )
}

/// Whether the default-profile inspector may promote `button` into gesture
/// mode. Per-app bindings are single-action overrides, so they cannot carry a
/// direction map.
fn can_enable_gestures(button: ButtonId, editing_app: Option<&str>) -> bool {
    editing_app.is_none() && button.supports_gesture_mode()
}

fn thumbwheel_inspector(
    bindings: &BTreeMap<ButtonId, Action>,
    editing_app: Option<&str>,
    overridden: Option<AppOverrides<'_>>,
    picker: ActionPickerContext<'_>,
    pal: Palette,
    cx: &Context<MouseModelView>,
) -> gpui::Div {
    let backward = bindings
        .get(&ButtonId::ThumbwheelScrollDown)
        .cloned()
        .unwrap_or_else(|| default_binding(ButtonId::ThumbwheelScrollDown));
    let forward = bindings
        .get(&ButtonId::ThumbwheelScrollUp)
        .cloned()
        .unwrap_or_else(|| default_binding(ButtonId::ThumbwheelScrollUp));
    let current = ThumbwheelPreset::recognize(&backward, &forward);
    let is_overridden = overridden.is_some_and(|overrides| {
        overrides.contains_key(ButtonId::ThumbwheelScrollDown)
            || overrides.contains_key(ButtonId::ThumbwheelScrollUp)
    });
    let status = match (editing_app, is_overridden) {
        (Some(app), true) => tr!("actions.overridden_in_app", app => app.to_string()),
        (Some(_), false) => tr!("profiles.inherited_from_default"),
        (None, _) => tr!("profiles.default_profile"),
    };
    let current_label = current.map_or_else(|| tr!("common.custom"), |p| tr!(p.translation_key()));
    let current_icon = current.map_or("action-icons/chevrons-right.svg", ThumbwheelPreset::icon);
    let observer = picker.view.clone();

    v_flex()
        .gap_3()
        .child(inspector_heading(
            tr!("pointer.thumb_wheel"),
            Some(status),
            pal,
        ))
        .child(selection_card(
            "inspector-current-thumbwheel-preset",
            tr!("common.preset"),
            current_icon,
            current_label,
            PickSlot::Tap,
            picker,
            pal,
        ))
        .when(
            matches!(
                current,
                Some(ThumbwheelPreset::Desktops | ThumbwheelPreset::DesktopsReversed)
            ),
            |panel| panel.child(super::desktop_switcher::desktop_switcher_panel(pal, cx)),
        )
        .when(picker.open, |panel| {
            panel.child(
                v_flex()
                    .gap_1()
                    .child(editor_section(tr!("common.preset"), pal))
                    .children(ThumbwheelPreset::ALL.into_iter().enumerate().map(
                        |(index, preset)| {
                            let selected = current == Some(preset);
                            let observer = observer.clone();
                            MenuRow::new(("inspector-thumbwheel", index))
                                .selected(selected)
                                .role(Role::Button)
                                .aria_label(tr!(preset.translation_key()))
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            svg()
                                                .path(preset.icon())
                                                .size_4()
                                                .text_color(pal.text_muted),
                                        )
                                        .child(div().child(tr!(preset.translation_key()))),
                                )
                                .when(selected, |row| {
                                    row.child(
                                        Icon::new(IconName::Check)
                                            .size_3()
                                            .text_color(rgb(ACCENT_BLUE)),
                                    )
                                })
                                .on_click(move |_, _, cx| {
                                    AppState::apply(cx, |state| {
                                        state.commit_thumbwheel_preset(preset)
                                    });
                                    observer.update(cx, |view, cx| {
                                        view.close_action_picker();
                                        cx.notify();
                                    });
                                })
                        },
                    )),
            )
        })
        .when(is_overridden, |panel| {
            let observer = picker.view.clone();
            panel.child(
                Button::new("inspector-thumbwheel-use-default")
                    .small()
                    .w_full()
                    .icon(IconName::Undo)
                    .label(tr!("profiles.use_the_default_profile"))
                    .on_click(move |_, _, cx| {
                        AppState::apply(cx, AppState::clear_app_thumbwheel);
                        observer.update(cx, |view, cx| {
                            view.close_action_picker();
                            cx.notify();
                        });
                    }),
            )
        })
}

fn inspector_heading(
    title: gpui::SharedString,
    status: Option<gpui::SharedString>,
    pal: Palette,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_heading().child(title))
        .children(status.map(|status| {
            div()
                .text_caption()
                .text_color(pal.text_muted)
                .child(status)
        }))
}

fn current_action_card(
    action: &Action,
    picker: ActionPickerContext<'_>,
    pal: Palette,
) -> impl IntoElement {
    selection_card(
        "inspector-current-action",
        tr!("actions.current_action"),
        action_icon_path(action),
        localized_action_label(action),
        PickSlot::Tap,
        picker,
        pal,
    )
}

fn gesture_summary_card(picker: ActionPickerContext<'_>, pal: Palette) -> impl IntoElement {
    selection_card(
        "inspector-current-gesture-summary",
        tr!("actions.current_action"),
        GESTURE_BUTTON_ICON,
        tr!("actions.five_directions"),
        PickSlot::Tap,
        picker,
        pal,
    )
}

fn selection_card(
    id: &'static str,
    caption: gpui::SharedString,
    icon: &'static str,
    value: gpui::SharedString,
    slot: PickSlot,
    picker: ActionPickerContext<'_>,
    pal: Palette,
) -> impl IntoElement {
    let toggle = picker.view.clone();
    let search = picker.search.clone();
    let open = picker.open && picker.slot == slot;
    let opening = !open;
    let accessible_label = value.clone();
    BaseButton::new(id)
        .debug_selector(move || id.to_string())
        .accessibility_label(accessible_label)
        .aria_expanded(open)
        .flex()
        .flex_col()
        .items_stretch()
        .gap_2()
        .rounded(pal.control_radius)
        .border_1()
        .border_color(pal.border)
        .bg(pal.control)
        .p_3()
        .cursor_pointer()
        .hover(move |card| card.bg(pal.control_hover))
        .focus_visible(move |card| card.focus_ring(pal))
        .child(
            div()
                .text_caption()
                .text_color(pal.text_muted)
                .child(caption),
        )
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    h_flex()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .child(
                            svg()
                                .path(icon)
                                .size_4()
                                .flex_none()
                                .text_color(pal.text_muted),
                        )
                        .child(div().min_w_0().truncate().text_body().child(value)),
                )
                .child(
                    svg()
                        .path(if open {
                            "action-icons/chevrons-up.svg"
                        } else {
                            "action-icons/chevrons-down.svg"
                        })
                        .size_3()
                        .flex_none()
                        .text_color(pal.text_muted),
                ),
        )
        .on_click(move |_, window, cx| {
            if opening {
                search.update(cx, |search, cx| search.set_value("", window, cx));
            }
            toggle.update(cx, |view, cx| {
                if opening {
                    view.clear_custom_action_drafts(window, cx);
                }
                view.toggle_action_picker(slot);
                cx.notify();
            });
        })
}

fn action_library(
    id_prefix: &'static str,
    current: Option<&Action>,
    picker: ActionPickerContext<'_>,
    offer: ActionOffer<'_>,
    on_pick: &PickFn,
    pal: Palette,
    cx: &Context<MouseModelView>,
) -> impl IntoElement {
    let query = picker.search.read(cx).value();
    let rows = action_rows_with(id_prefix, current, &query, offer, on_pick, pal);
    v_flex()
        .gap_2()
        .pt_1()
        .child(custom_shortcut_editor(id_prefix, &picker, on_pick, pal))
        .child(application_editor(
            id_prefix,
            picker.application_input,
            picker.invalid.application,
            Rc::new({
                let view = picker.view.clone();
                move |cx| {
                    view.update(cx, |view, cx| {
                        view.invalid_drafts.application = true;
                        cx.notify();
                    });
                }
            }),
            on_pick,
            pal,
        ))
        .child(command_editor(
            id_prefix,
            picker.command_input,
            picker.invalid.command,
            Rc::new({
                let view = picker.view.clone();
                move |cx| {
                    view.update(cx, |view, cx| {
                        view.invalid_drafts.command = true;
                        cx.notify();
                    });
                }
            }),
            on_pick,
            pal,
        ))
        .child(dpi_preset_editor(
            id_prefix,
            AppState::try_read(cx).map_or(0, |s| s.dpi_presets().len()),
            current,
            on_pick,
            pal,
        ))
        .child(editor_section(tr!("actions.actions"), pal))
        .child(control_input(picker.search).cleanable(true))
        .child(
            v_flex()
                .gap_0p5()
                .when(rows.is_empty(), |list| {
                    list.child(
                        div()
                            .py_3()
                            .text_body()
                            .text_color(pal.text_muted)
                            .child(tr!("actions.no_actions_found")),
                    )
                })
                .children(rows),
        )
}

/// A single-field "Custom Shortcut" editor, matching the Action Ring editor's
/// `shortcut_editor` (`features/action_ring/editor.rs`) so the same custom
/// action is reachable from the plain per-button picker, not just the ring.
/// Record fills the field from the next key press; Add commits the field.
fn custom_shortcut_editor(
    id_prefix: &'static str,
    picker: &ActionPickerContext<'_>,
    on_pick: &PickFn,
    pal: Palette,
) -> impl IntoElement {
    let input = picker.shortcut_input;
    let mode = picker.shortcut_mode;
    let recording = picker.shortcut_recording;
    let view = picker.view;
    let submit_input = input.clone();
    let on_pick = on_pick.clone();
    let mode_button = |option: ShortcutMode| {
        let (id, label) = match option {
            ShortcutMode::Tap => ("tap", tr!("actions.shortcut_tap")),
            ShortcutMode::Hold => ("hold", tr!("actions.shortcut_hold")),
        };
        let view = view.clone();
        control_button(format!("{id_prefix}-shortcut-{id}"))
            .debug_selector(move || format!("{id_prefix}-shortcut-{id}"))
            .selected(mode == option)
            .label(label)
            .on_click(move |_, _, cx| {
                view.update(cx, |view, cx| {
                    view.shortcut_mode = option;
                    cx.notify();
                });
            })
    };
    let tap_button = mode_button(ShortcutMode::Tap);
    let hold_button = mode_button(ShortcutMode::Hold);
    let record_view = view.clone();
    let view = view.clone();
    v_flex()
        .gap_1()
        .child(editor_section(tr!("action_ring.custom_shortcut"), pal))
        .child(h_flex().gap_2().child(tap_button).child(hold_button))
        .child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .debug_selector(move || format!("{id_prefix}-custom-shortcut-input"))
                        .child(control_input(input).cleanable(true)),
                )
                .child(
                    div().track_focus(picker.shortcut_record_focus).child(
                        Button::new(format!("{id_prefix}-custom-shortcut-record"))
                            .debug_selector(move || format!("{id_prefix}-custom-shortcut-record"))
                            .compact()
                            .selected(recording)
                            .label(if recording {
                                tr!("common.cancel")
                            } else {
                                tr!("actions.record_shortcut")
                            })
                            .on_click(move |_, window, cx| {
                                record_view.update(cx, |view, cx| {
                                    view.toggle_shortcut_recording(window, cx);
                                });
                            }),
                    ),
                )
                .child(
                    Button::new(format!("{id_prefix}-custom-shortcut-add"))
                        .debug_selector(move || format!("{id_prefix}-custom-shortcut-add"))
                        .compact()
                        .label(tr!("common.add"))
                        .on_click(move |_, window, cx| {
                            let shortcut = submit_input.read(cx).value().to_string();
                            match shortcut.parse::<openlogi_core::binding::KeyCombo>() {
                                Ok(combo) => (on_pick)(mode.action(combo), window, cx),
                                Err(_) => view.update(cx, |view, cx| {
                                    view.invalid_drafts.shortcut =
                                        Some(ShortcutError::Unrecognized);
                                    cx.notify();
                                }),
                            }
                        }),
                ),
        )
        .when(recording, |editor| {
            editor.child(
                div()
                    .debug_selector(move || format!("{id_prefix}-custom-shortcut-recording"))
                    .text_caption()
                    .text_color(pal.text_muted)
                    .child(tr!("actions.press_the_shortcut_to_record")),
            )
        })
        .when_some(picker.invalid.shortcut, |editor, error| {
            editor.child(
                div()
                    .debug_selector(move || format!("{id_prefix}-custom-shortcut-error"))
                    .text_caption()
                    .text_color(pal.text_danger)
                    .child(match error {
                        ShortcutError::Unrecognized => {
                            tr!("action_ring.custom_action_invalid_input")
                        }
                        ShortcutError::ShiftedSymbol => tr!("actions.shifted_symbol_unrecordable"),
                    }),
            )
        })
}

fn gesture_action(
    gesture_map: &BTreeMap<GestureDirection, Action>,
    button: ButtonId,
    direction: GestureDirection,
) -> Action {
    gesture_map.get(&direction).cloned().unwrap_or_else(|| {
        if direction == GestureDirection::Click {
            default_binding(button)
        } else {
            Action::None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_and_hold_card_names_the_tap_then_the_hold_in_milliseconds() {
        let binding = LongPressBinding::new(Action::BrowserBack, Action::Copy).with_hold(
            openlogi_core::binding::HoldThreshold::try_from(500).expect("500 ms is in range"),
        );

        let label = tap_hold_label("Back", &binding);

        assert_eq!(
            label.as_ref(),
            format!(
                "tap: Back / hold 500 ms: {}",
                localized_action_label(&Action::Copy)
            )
        );
    }

    #[test]
    fn default_profile_offers_gestures_for_every_supported_button() {
        let supported: Vec<_> = ButtonId::ALL
            .into_iter()
            .filter(|button| can_enable_gestures(*button, None))
            .collect();

        assert_eq!(
            supported,
            vec![
                ButtonId::MiddleClick,
                ButtonId::Back,
                ButtonId::Forward,
                ButtonId::DpiToggle,
                ButtonId::GestureButton,
                ButtonId::HapticPanel,
            ]
        );
    }

    #[test]
    fn per_app_profile_does_not_offer_forward_gesture_mode() {
        assert!(!can_enable_gestures(
            ButtonId::Forward,
            Some("com.apple.Safari")
        ));
    }

    static NO_LONG_PRESSES: BTreeMap<ButtonId, LongPressBinding> = BTreeMap::new();

    fn global_profile<'a>(
        bindings: &'a BTreeMap<ButtonId, Action>,
        gesture_maps: &'a BTreeMap<ButtonId, BTreeMap<GestureDirection, Action>>,
        wheel_hold: &'a BTreeSet<ButtonId>,
    ) -> BindingInspectorData<'a> {
        BindingInspectorData {
            selected: Some(MouseControlId::Button(ButtonId::Back)),
            gesture_direction: None,
            bindings,
            layout: None,
            gesture_maps,
            dpi_gestures: false,
            extra_actions: &[],
            hidden_actions: &[],
            virtual_thumbwheel: true,
            wheel_hold,
            editing_app: None,
            overridden: None,
            app_wheel_speed: None,
            app_dpi: None,
            press_force: None,
            long_presses: &NO_LONG_PRESSES,
            hold_editable: false,
            hold_slider: None,
            app_switcher: None,
        }
    }

    #[test]
    fn a_g_button_on_its_stock_function_reads_as_the_device_default() {
        let _locale = crate::services::i18n::LOCALE_LOCK.lock().unwrap();
        let (bindings, gesture_maps, held) = (BTreeMap::new(), BTreeMap::new(), BTreeSet::new());
        let mut data = global_profile(&bindings, &gesture_maps, &held);
        data.layout = Some(&openlogi_core::binding::G502_LIGHTSPEED);
        assert_eq!(
            button_status(ButtonId::G6, &Action::DpiShift, &data, false),
            tr!("pointer.device_default")
        );
        data.layout = None;
        assert_eq!(
            button_status(ButtonId::G6, &Action::DpiShift, &data, false),
            tr!("profiles.customized")
        );
    }

    #[test]
    fn a_button_holding_the_wheel_reads_customized_even_with_its_default_click() {
        let _locale = crate::services::i18n::LOCALE_LOCK.lock().unwrap();
        let (bindings, gesture_maps) = (BTreeMap::new(), BTreeMap::new());
        let action = default_binding(ButtonId::Back);
        let plain = BTreeSet::new();
        let data = global_profile(&bindings, &gesture_maps, &plain);
        assert_eq!(
            button_status(ButtonId::Back, &action, &data, false),
            tr!("pointer.device_default")
        );
        let held = BTreeSet::from([ButtonId::Back]);
        let data = global_profile(&bindings, &gesture_maps, &held);
        assert_eq!(
            button_status(ButtonId::Back, &action, &data, false),
            tr!("profiles.customized")
        );
    }
}
