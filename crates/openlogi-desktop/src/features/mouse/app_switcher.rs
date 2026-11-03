//! The tunables of a button bound to the app switcher: how far the mouse
//! travels sideways per app (step) and how long the button is held before the
//! switcher opens. The hold time rides the inspector's shared hold slider; the
//! step has its own. Both take a typed number beside the slider.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{Context, IntoElement, ParentElement as _, Styled as _, Window};
use gpui_component::v_flex;
use openlogi_core::binding::{AppSwitcherParams, ButtonId, HoldThreshold, StepPx};

use super::inspector::{hold_range, hold_seconds};
use super::view::MouseModelView;
use crate::state::AppState;
use crate::ui::commit_slider::{CommitSlider, SliderRange};
use crate::ui::number_entry::NumberEntry;
use crate::ui::slider_row::{SliderRow, slider_with_entry};

/// The step slider's range, in raw mouse counts. The ends are the config's
/// own bounds ([`StepPx`]).
fn step_range() -> SliderRange<u16> {
    SliderRange::new(StepPx::MIN, StepPx::MAX).step(10.)
}

/// `params`' hold time in milliseconds.
pub(super) fn hold_ms(params: AppSwitcherParams) -> u16 {
    u16::from(params.hold_threshold())
}

/// The controls the app switcher panel owns, kept by the view so a drag or an
/// edit survives a repaint.
pub(super) struct AppSwitcherControls {
    step_slider: CommitSlider<u16>,
    step_entry: NumberEntry<u16>,
    hold_entry: NumberEntry<u16>,
    /// The button the entries commit to, kept current by [`Self::sync`].
    button: Rc<Cell<Option<ButtonId>>>,
}

impl AppSwitcherControls {
    pub(super) fn new(window: &mut Window, cx: &mut Context<MouseModelView>) -> Self {
        let button = Rc::new(Cell::new(None));
        let step = StepPx::DEFAULT.get();
        let hold = hold_ms(AppSwitcherParams::default());
        Self {
            step_slider: CommitSlider::new(step_range(), step, cx, |view, step, cx| {
                if let Some(button) = view.selected_button() {
                    AppState::apply(cx, |state| state.commit_app_switcher_step(button, step));
                }
            }),
            step_entry: NumberEntry::new(step_range(), step, window, cx, {
                let button = button.clone();
                move |step, cx| {
                    if let Some(button) = button.get() {
                        AppState::apply(cx, |state| state.commit_app_switcher_step(button, step));
                    }
                }
            }),
            hold_entry: NumberEntry::new(hold_range(), hold, window, cx, {
                let button = button.clone();
                move |ms, cx| {
                    if let Some(button) = button.get() {
                        AppState::apply(cx, |state| state.commit_app_switcher_hold(button, ms));
                    }
                }
            }),
            button,
        }
    }

    /// Point the controls at `button` and seat them on `params`.
    pub(super) fn sync(
        &self,
        button: Option<ButtonId>,
        params: Option<AppSwitcherParams>,
        window: &mut Window,
        cx: &mut Context<MouseModelView>,
    ) {
        self.button.set(button);
        let params = params.unwrap_or_default();
        self.step_slider.sync(params.step().get(), window, cx);
        self.step_entry
            .sync(self.step_slider.shown(params.step().get()), window, cx);
        self.hold_entry
            .sync(hold_ms(params), window, cx);
    }

    /// The editable step and hold time of the selected app switcher button.
    pub(super) fn panel(
        &self,
        params: AppSwitcherParams,
        hold_slider: &CommitSlider<u16>,
    ) -> impl IntoElement {
        let step = self.step_slider.shown(params.step().get());
        let hold = hold_slider.shown(hold_ms(params));
        v_flex()
            .gap_3()
            .child(
                SliderRow::new(
                    tr!("actions.step_distance"),
                    slider_with_entry(self.step_slider.slider(), self.step_entry.input()),
                )
                .value(format!("{step}"), [format!("{}", StepPx::MAX)])
                .description(tr!("actions.step_distance_description")),
            )
            .child(
                SliderRow::new(
                    tr!("actions.hold_time"),
                    slider_with_entry(hold_slider.slider(), self.hold_entry.input()),
                )
                .value(
                    format!("{} s", hold_seconds(hold)),
                    [format!("{} s", hold_seconds(HoldThreshold::MAX_MS))],
                ),
            )
    }
}
