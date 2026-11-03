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
    APP_SWITCHER_HOLD_LIMIT, APP_SWITCHER_SETTLE, Action, AppSwitcherParams, HeldKeys,
    SwitchDirection, SwitchStepper, switcher_chord,
};
use tracing::{debug, warn};

use super::PressToken;

/// The keys a hold drives. Dropping the value releases the modifier it holds,
/// so its owner can never forget to.
pub(super) trait SwitcherKeys: Send + 'static {
    /// Type one switcher step with the modifier still down.
    fn step(&mut self, direction: SwitchDirection);
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
}

/// One open switcher: the held keys, and the stepper that turns the mouse's
/// travel into taps on them.
struct SwitcherHold<K: SwitcherKeys> {
    keys: Option<K>,
    stepper: SwitchStepper,
    /// When the hold last typed a key, and how long the modifier must outlast it.
    last_key: Instant,
    settle: Duration,
}

impl<K: SwitcherKeys> SwitcherHold<K> {
    fn motion(&mut self, dx: i16) {
        let Some(keys) = self.keys.as_mut() else {
            return;
        };
        if let Some(run) = self.stepper.motion(dx) {
            debug!(?run, "app switcher step");
            for _ in 0..run.count {
                keys.step(run.direction);
            }
            self.last_key = Instant::now();
        }
    }

    /// Let go of the modifier. Idempotent.
    ///
    /// The modifier outlasts the last typed key by the settle time, so the
    /// native switcher has handled that key before it sees the release.
    fn release(&mut self) -> bool {
        if self.keys.is_none() {
            return false;
        }
        let wait = self.settle.saturating_sub(self.last_key.elapsed());
        if !wait.is_zero() {
            thread::sleep(wait);
        }
        debug!(?wait, "app switcher modifier released");
        self.keys.take().is_some()
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
    pub(super) fn start(keys: K, stepper: SwitchStepper, limit: Duration) -> Self {
        Self::start_settling(keys, stepper, limit, APP_SWITCHER_SETTLE)
    }

    fn start_settling(keys: K, stepper: SwitchStepper, limit: Duration, settle: Duration) -> Self {
        let hold = Arc::new(Mutex::new(SwitcherHold {
            keys: Some(keys),
            stepper,
            last_key: Instant::now(),
            settle,
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

    fn motion(&self, dx: i16) {
        lock(&self.hold).motion(dx);
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
}

impl<K: SwitcherKeys> Default for HeldAppSwitchers<K> {
    fn default() -> Self {
        Self {
            by_press: HashMap::new(),
        }
    }
}

impl HeldAppSwitchers<SystemKeys> {
    /// Open the switcher for `press` and follow its mouse travel from here.
    pub(super) fn start(&mut self, press: &PressToken, params: AppSwitcherParams) {
        self.start_with(press, params.stepper(), SystemKeys::open());
    }
}

impl<K: SwitcherKeys> HeldAppSwitchers<K> {
    fn start_with(&mut self, press: &PressToken, stepper: SwitchStepper, keys: K) {
        // A second start for one press replaces the first, which releases its
        // modifier as it drops; the new hold presses its own.
        self.by_press.insert(
            press.clone(),
            ActiveHold::start(keys, stepper, APP_SWITCHER_HOLD_LIMIT),
        );
    }

    /// Travel before the hold opened, or after it ended, steps nothing.
    pub(super) fn motion(&self, press: &PressToken, dx: i16) {
        if let Some(hold) = self.by_press.get(press) {
            hold.motion(dx);
        } else {
            debug!(dx, "app switcher travel outside a hold — ignored");
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
