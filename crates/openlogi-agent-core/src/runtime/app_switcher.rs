//! The hold half of the app switcher button.
//!
//! A tap is an ordinary short action. A hold past the binding's threshold
//! presses and keeps the switcher modifier down so the native switcher stays
//! open, and steps it as the mouse moves sideways. The modifier is owned by
//! [`SwitcherHold`] and released exactly once, by whichever comes first: the
//! press ending (release, cancellation, shutdown), or the safety limit,
//! so a release that never arrives cannot leave it stuck.

use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use openlogi_core::binding::{
    APP_SWITCHER_HOLD_LIMIT, APP_SWITCHER_SETTLE, Action, AppSwitcherParams, HeldKeys, StepPx,
    SwitchDirection, SwitchStepper, switcher_cancel_chord, switcher_chord,
};
use openlogi_core::switcher_rows::SwitcherSelection;
use tracing::{debug, warn};

use super::PressToken;
use crate::switcher::{NoSwitcherOverlay, SwitcherPort, SwitcherTarget};

/// The rows a hold draws once the mouse has moved: the port that shows them,
/// the step the travel counts in, and the selection, opened on the first
/// travel so a hold that never moves never reads the windows.
pub(super) struct Overlay {
    port: Arc<dyn SwitcherPort>,
    step: StepPx,
    selection: Option<SwitcherSelection>,
}

impl Overlay {
    fn new(port: Arc<dyn SwitcherPort>, step: StepPx) -> Self {
        Self {
            port,
            step,
            selection: None,
        }
    }

    /// Account for one mouse report, and show the rows when they opened or
    /// the selection moved.
    fn motion(&mut self, dx: i16, dy: i16) {
        let Self {
            port,
            step,
            selection,
        } = self;
        let opened = selection.is_none();
        let selection =
            selection.get_or_insert_with(|| SwitcherSelection::open(port.snapshot(), 1, *step));
        let moved = selection.motion(dx, dy);
        if opened || moved {
            port.show(selection.view());
        }
    }

    /// The entry the cursor is on, if the rows are open and it is on one.
    fn target(&self) -> Option<SwitcherTarget> {
        self.selection.as_ref()?.selection().map(SwitcherTarget::of)
    }

    /// The press ended: take the rows down, if any were shown.
    fn end(self) {
        if self.selection.is_some() {
            self.port.hide();
        }
    }
}

/// The keys a hold drives. Dropping the value releases the modifier it holds,
/// so its owner can never forget to.
pub(super) trait SwitcherKeys: Send + 'static {
    /// Type one switcher step with the modifier still down.
    fn step(&mut self, direction: SwitchDirection);

    /// Dismiss the open switcher so letting go of the modifier activates
    /// nothing.
    fn cancel(&mut self);
}

/// The real keyboard: the modifier is a held chord, and each step a chord of
/// its own so the modifier stays down throughout.
pub(super) struct SystemKeys {
    _held: openlogi_inject::HeldChord,
}

impl SystemKeys {
    /// Press the modifier and tap Tab once, which opens the switcher.
    pub(super) fn open() -> Self {
        Self {
            _held: openlogi_inject::press_hold(
                switcher_chord(SwitchDirection::Forward),
                HeldKeys::ModifiersOnly,
            ),
        }
    }
}

impl SwitcherKeys for SystemKeys {
    fn step(&mut self, direction: SwitchDirection) {
        openlogi_inject::execute(&Action::CustomShortcut(switcher_chord(direction).clone()));
    }

    fn cancel(&mut self) {
        openlogi_inject::execute(&Action::CustomShortcut(switcher_cancel_chord().clone()));
    }
}

/// One open switcher: the held keys, and the stepper that turns the mouse's
/// travel into taps on them.
struct SwitcherHold<K: SwitcherKeys> {
    keys: Option<K>,
    stepper: SwitchStepper,
    /// Whether the hold has stepped the switcher at all. A hold that never did
    /// is cancelled on release: only a quick tap switches app without motion.
    stepped: bool,
    /// When the hold last typed a key, and how long the modifier must outlast it.
    last_key: Instant,
    settle: Duration,
    /// The rows drawn beside the native switcher, when the hold has a host for them.
    overlay: Option<Overlay>,
}

impl<K: SwitcherKeys> SwitcherHold<K> {
    fn motion(&mut self, dx: i16, dy: i16) {
        let Some(keys) = self.keys.as_mut() else {
            return;
        };
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.motion(dx, dy);
        }
        if let Some(run) = self.stepper.motion(dx) {
            debug!(?run, "app switcher step");
            for _ in 0..run.count {
                keys.step(run.direction);
            }
            self.stepped = true;
            self.last_key = Instant::now();
        }
    }

    /// Let go of the modifier. Idempotent.
    ///
    /// The modifier outlasts the last typed key by the settle time, so the
    /// native switcher has handled that key before it sees the release.
    fn release(&mut self) -> bool {
        let overlay = self.overlay.take();
        let target = overlay.as_ref().and_then(Overlay::target);
        let port = overlay.as_ref().map(|overlay| Arc::clone(&overlay.port));
        if let Some(overlay) = overlay {
            overlay.end();
        }
        let Some(keys) = self.keys.as_mut() else {
            return false;
        };
        // Choosing an entry dismisses the native switcher too, so its own
        // highlight never activates an app instead of the one chosen here.
        if !self.stepped || target.is_some() {
            debug!(chosen = target.is_some(), "app switcher dismissed");
            keys.cancel();
            self.last_key = Instant::now();
        }
        let wait = self.settle.saturating_sub(self.last_key.elapsed());
        if !wait.is_zero() {
            thread::sleep(wait);
        }
        debug!(?wait, "app switcher modifier released");
        let released = self.keys.take().is_some();
        if let (true, Some(port), Some(target)) = (released, port, target) {
            port.activate(&target);
        }
        released
    }
}

/// A [`SwitcherHold`] plus the watchdog that releases it when the press never
/// ends. Dropping it releases the hold and retires the watchdog.
pub(super) struct ActiveHold<K: SwitcherKeys> {
    hold: Arc<Mutex<SwitcherHold<K>>>,
    /// The watchdog waits on the other end; dropping this wakes it to exit.
    _retire_watchdog: mpsc::Sender<()>,
}

impl<K: SwitcherKeys> ActiveHold<K> {
    pub(super) fn start(
        keys: K,
        stepper: SwitchStepper,
        limit: Duration,
        overlay: Option<Overlay>,
    ) -> Self {
        Self::start_settling(keys, stepper, limit, APP_SWITCHER_SETTLE, overlay)
    }

    fn start_settling(
        keys: K,
        stepper: SwitchStepper,
        limit: Duration,
        settle: Duration,
        overlay: Option<Overlay>,
    ) -> Self {
        let hold = Arc::new(Mutex::new(SwitcherHold {
            keys: Some(keys),
            stepper,
            stepped: false,
            last_key: Instant::now(),
            settle,
            overlay,
        }));
        let (retire, retired) = mpsc::channel::<()>();
        let watched = Arc::clone(&hold);
        let spawned = thread::Builder::new()
            .name("openlogi-switcher-watchdog".into())
            .spawn(move || {
                if retired.recv_timeout(limit) == Err(RecvTimeoutError::Timeout)
                    && lock(&watched).release()
                {
                    warn!(?limit, "app switcher hold never ended — modifier released");
                }
            });
        if let Err(error) = spawned {
            // The hold still ends with its press; only the backstop is gone.
            warn!(%error, "could not start the app switcher watchdog");
        }
        Self {
            hold,
            _retire_watchdog: retire,
        }
    }

    fn motion(&self, dx: i16, dy: i16) {
        lock(&self.hold).motion(dx, dy);
    }
}

impl<K: SwitcherKeys> Drop for ActiveHold<K> {
    fn drop(&mut self) {
        lock(&self.hold).release();
    }
}

fn lock<K: SwitcherKeys>(
    hold: &Mutex<SwitcherHold<K>>,
) -> std::sync::MutexGuard<'_, SwitcherHold<K>> {
    // The guarded state is a stepper and an `Option`; a panic elsewhere
    // leaves both consistent, and the release must still happen.
    hold.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The open switchers, one per press that started one. The press token is the
/// only key: ending, cancelling or invalidating a press removes its entry and
/// so releases its modifier.
pub(super) struct HeldAppSwitchers<K: SwitcherKeys = SystemKeys> {
    by_press: HashMap<PressToken, ActiveHold<K>>,
    /// Where the rows of an open switcher are drawn and listed.
    port: Arc<dyn SwitcherPort>,
}

impl<K: SwitcherKeys> Default for HeldAppSwitchers<K> {
    fn default() -> Self {
        Self::new(Arc::new(NoSwitcherOverlay))
    }
}

impl<K: SwitcherKeys> HeldAppSwitchers<K> {
    pub(super) fn new(port: Arc<dyn SwitcherPort>) -> Self {
        Self {
            by_press: HashMap::new(),
            port,
        }
    }
}

impl HeldAppSwitchers<SystemKeys> {
    /// Open the switcher for `press` and follow its mouse travel from here.
    pub(super) fn start(&mut self, press: &PressToken, params: AppSwitcherParams) {
        let overlay = Overlay::new(Arc::clone(&self.port), params.step());
        self.start_with(press, params.stepper(), SystemKeys::open(), Some(overlay));
    }
}

impl<K: SwitcherKeys> HeldAppSwitchers<K> {
    fn start_with(
        &mut self,
        press: &PressToken,
        stepper: SwitchStepper,
        keys: K,
        overlay: Option<Overlay>,
    ) {
        // A second start for one press replaces the first, which releases its
        // modifier as it drops; the new hold presses its own.
        self.by_press.insert(
            press.clone(),
            ActiveHold::start(keys, stepper, APP_SWITCHER_HOLD_LIMIT, overlay),
        );
    }

    /// Travel before the hold opened, or after it ended, steps nothing.
    pub(super) fn motion(&self, press: &PressToken, dx: i16, dy: i16) {
        if let Some(hold) = self.by_press.get(press) {
            hold.motion(dx, dy);
        } else {
            debug!(dx, dy, "app switcher travel outside a hold — ignored");
        }
    }

    /// The press ended: release the modifier so the native switcher activates
    /// the highlighted app.
    pub(super) fn end(&mut self, press: &PressToken) {
        self.by_press.remove(press);
    }
}

#[cfg(test)]
mod tests;
