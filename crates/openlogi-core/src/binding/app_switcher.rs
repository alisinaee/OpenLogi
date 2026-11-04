//! The app switcher button: tap for the previous app, hold to browse the
//! native switcher by moving the mouse sideways.
//!
//! This module is the single owner of every decision the feature makes: the
//! default hold time and step distance, the accepted range of each, the chord
//! a tap types, the safety limit on a hold, and the rule that turns horizontal
//! travel into switcher steps. The agent applies them; none of its crates
//! restates a number or a chord.

use std::sync::LazyLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::action::Action;
use super::key_combo::KeyCombo;
use super::value::{HoldThreshold, LongPressBinding};

/// How long a hold may last before the agent takes its rows down on its own.
/// The rows stay up for as long as the button is held, so a release that never
/// arrives (the mouse went out of range mid-hold) must not leave them on screen.
pub const APP_SWITCHER_HOLD_LIMIT: Duration = Duration::from_secs(10);

/// The hold time of a binding that sets none, in milliseconds.
const DEFAULT_HOLD_MS: u16 = 250;
const _: () = assert!(
    DEFAULT_HOLD_MS >= HoldThreshold::MIN_MS && DEFAULT_HOLD_MS <= HoldThreshold::MAX_MS,
    "the default app switcher hold must be a hold time a binding could set"
);

/// The step distance of a binding that sets none, in raw mouse counts.
const DEFAULT_STEP_PX: u16 = 120;

/// Horizontal travel, in raw sensor counts, that moves the switcher by one
/// app. The single owner of the accepted range: a value outside
/// [`MIN`](Self::MIN)`..=`[`MAX`](Self::MAX) fails to deserialize, so a step
/// can neither be so small that sensor noise walks the switcher nor so large
/// that a sweep never reaches the next app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct StepPx(u16);

impl StepPx {
    /// Smallest accepted step.
    pub const MIN: u16 = 10;
    /// Largest accepted step.
    pub const MAX: u16 = 1000;
    /// The step of a binding that sets none.
    pub const DEFAULT: Self = Self(DEFAULT_STEP_PX);

    /// The step as a count of raw mouse counts.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

const _: () = assert!(
    DEFAULT_STEP_PX >= StepPx::MIN && DEFAULT_STEP_PX <= StepPx::MAX,
    "the default app switcher step must be a step a binding could set"
);

impl TryFrom<u16> for StepPx {
    type Error = String;

    fn try_from(px: u16) -> Result<Self, Self::Error> {
        if (Self::MIN..=Self::MAX).contains(&px) {
            Ok(Self(px))
        } else {
            Err(format!(
                "step must be between {} and {} counts, got {px}",
                Self::MIN,
                Self::MAX
            ))
        }
    }
}

impl From<StepPx> for u16 {
    fn from(step: StepPx) -> Self {
        step.0
    }
}

/// Which way the native switcher moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SwitchDirection {
    /// To the next app: Tab, toward the right.
    Forward,
    /// To the previous app: Shift+Tab, toward the left.
    Backward,
}

/// The chord that moves the native switcher one app in `direction`, modifiers
/// included. [`Forward`](SwitchDirection::Forward) is the chord of a plain tap,
/// which switches to the previous app. It is Cmd+Tab on macOS, where the
/// feature is supported, and Alt+Tab elsewhere.
#[must_use]
pub fn switcher_chord(direction: SwitchDirection) -> &'static KeyCombo {
    static FORWARD: LazyLock<KeyCombo> = LazyLock::new(|| parse_chord(base_chord()));
    static BACKWARD: LazyLock<KeyCombo> = LazyLock::new(|| parse_chord(shifted_chord()));
    match direction {
        SwitchDirection::Forward => &FORWARD,
        SwitchDirection::Backward => &BACKWARD,
    }
}

const fn base_chord() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd+Tab"
    } else {
        "Alt+Tab"
    }
}

const fn shifted_chord() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd+Shift+Tab"
    } else {
        "Alt+Shift+Tab"
    }
}

fn parse_chord(text: &str) -> KeyCombo {
    text.parse()
        .unwrap_or_else(|error| unreachable!("hardcoded switcher chord {text:?}: {error}"))
}

/// The tunables of an app switcher binding. Each is optional in the config
/// and falls back to a default owned by this module.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSwitcherParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    step: Option<StepPx>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hold_ms: Option<HoldThreshold>,
}

impl AppSwitcherParams {
    /// Parameters with the given overrides; `None` keeps the default.
    #[must_use]
    pub const fn new(step: Option<StepPx>, hold: Option<HoldThreshold>) -> Self {
        Self {
            step,
            hold_ms: hold,
        }
    }

    /// Horizontal travel that moves the switcher by one app.
    #[must_use]
    pub fn step(self) -> StepPx {
        self.step.unwrap_or(StepPx::DEFAULT)
    }

    /// How long the button must stay down for the switcher to open and follow
    /// the mouse instead of switching straight to the previous app.
    #[must_use]
    pub fn hold(self) -> Duration {
        self.hold_threshold().duration()
    }

    /// [`hold`](Self::hold) as the config's own validated hold time.
    #[must_use]
    pub fn hold_threshold(self) -> HoldThreshold {
        self.hold_ms.unwrap_or(HoldThreshold(DEFAULT_HOLD_MS))
    }

    /// The press this binding is: a tap switches to the previous app, and a
    /// hold past [`hold`](Self::hold) opens the switcher. The runtime times
    /// it exactly as it times any other long press.
    #[must_use]
    pub fn long_press(self) -> LongPressBinding {
        let tap = Action::CustomShortcut(switcher_chord(SwitchDirection::Forward).clone());
        LongPressBinding::new(tap, Action::AppSwitcher).with_hold(self.hold_threshold())
    }
}

/// An app switcher binding that sets its tunables, serialized as
/// `{ AppSwitcher = { step = 120, hold_ms = 250 } }`. A binding that keeps the
/// defaults is the plain action `"AppSwitcher"` instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSwitcherBinding {
    #[serde(rename = "AppSwitcher")]
    params: AppSwitcherParams,
}

impl AppSwitcherBinding {
    /// A binding with these parameters.
    #[must_use]
    pub const fn new(params: AppSwitcherParams) -> Self {
        Self { params }
    }

    /// The binding's tunables.
    #[must_use]
    pub const fn params(self) -> AppSwitcherParams {
        self.params
    }
}

/// A run of identical switcher steps one motion report produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchRun {
    /// Which way to move.
    pub direction: SwitchDirection,
    /// How many apps to move, at least one.
    pub count: u16,
}

/// Turns horizontal mouse travel into switcher steps. Sans-I/O: the caller
/// feeds each report's `dx` and types the steps it returns.
///
/// Travel accumulates until it reaches one step. One report moves at most one
/// app, however far it travelled, and only the part of the travel short of a
/// further whole step carries over, so a fast flick cannot skip apps. Reversing
/// direction discards the partial step in the old direction: sensor jitter
/// around a boundary then cannot make the switcher flutter back and forth,
/// and walking back takes a full step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchStepper {
    step: i32,
    travel: i32,
}

impl SwitchStepper {
    /// A stepper that moves once per `step` counts, starting at rest.
    #[must_use]
    pub fn new(step: StepPx) -> Self {
        Self {
            step: i32::from(step.get()),
            travel: 0,
        }
    }

    /// Account for `dx` counts of horizontal travel (positive is rightward)
    /// and return the steps they complete, if any. Vertical travel never
    /// reaches this: the caller passes only `dx`.
    pub fn motion(&mut self, dx: i16) -> Option<SwitchRun> {
        let dx = i32::from(dx);
        if dx == 0 {
            return None;
        }
        if self.travel != 0 && self.travel.signum() != dx.signum() {
            self.travel = 0;
        }
        self.travel = self.travel.saturating_add(dx);
        if self.travel.abs() < self.step {
            return None;
        }
        let direction = if self.travel > 0 {
            SwitchDirection::Forward
        } else {
            SwitchDirection::Backward
        };
        // One step per report; whole steps beyond it are dropped.
        self.travel %= self.step;
        Some(SwitchRun {
            direction,
            count: 1,
        })
    }
}

#[cfg(test)]
mod tests;
