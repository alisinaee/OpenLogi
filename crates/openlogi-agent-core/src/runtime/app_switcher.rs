//! The hold half of the app switcher button.
//!
//! A tap is an ordinary short action. A hold past the binding's threshold
//! opens the switcher's rows, which the agent's own overlay draws, and the
//! selection follows the mouse sideways. The native switcher is never touched
//! during a hold: no modifier is held and no chord is typed. Releasing the
//! press takes the rows down and activates the entry the cursor was on, if
//! any. A hold that never moved draws nothing and activates nothing.
//!
//! The rows are owned by [`ActiveHold`] and taken down exactly once, by
//! whichever comes first: the press ending, or the safety limit, so a release
//! that never arrives cannot leave them on screen.

use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use openlogi_core::binding::{APP_SWITCHER_HOLD_LIMIT, AppSwitcherParams, StepPx};
use openlogi_core::switcher_rows::SwitcherSelection;
use tracing::{debug, warn};

use super::PressToken;
use crate::switcher::{SwitcherPort, SwitcherTarget};

/// The rows a hold draws once the mouse has moved: the port that shows them,
/// the step the travel counts in, and the selection, opened on the first
/// travel so a hold that never moves never reads the windows.
struct Overlay {
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

    /// The press ended: take the rows down, then bring the entry the cursor
    /// was on to the front.
    fn end(self) {
        let target = self.target();
        if self.selection.is_some() {
            self.port.hide();
        }
        if let Some(target) = target {
            self.port.activate(&target);
        }
    }
}

/// End the rows in `slot` once. Returns whether there were rows to end.
fn end_rows(slot: &Mutex<Option<Overlay>>) -> bool {
    let overlay = lock(slot).take();
    let Some(overlay) = overlay else {
        return false;
    };
    overlay.end();
    true
}

/// One open switcher: the rows, and the watchdog that ends them when the press
/// never does. Dropping it ends the rows and retires the watchdog.
pub(super) struct ActiveHold {
    rows: Arc<Mutex<Option<Overlay>>>,
    /// The watchdog waits on the other end; dropping this wakes it to exit.
    _retire_watchdog: mpsc::Sender<()>,
}

impl ActiveHold {
    fn start(overlay: Overlay, limit: Duration) -> Self {
        let rows = Arc::new(Mutex::new(Some(overlay)));
        let (retire, retired) = mpsc::channel::<()>();
        let watched = Arc::clone(&rows);
        let spawned = thread::Builder::new()
            .name("openlogi-switcher-watchdog".into())
            .spawn(move || {
                if retired.recv_timeout(limit) == Err(RecvTimeoutError::Timeout)
                    && end_rows(&watched)
                {
                    warn!(?limit, "app switcher hold never ended — rows taken down");
                }
            });
        if let Err(error) = spawned {
            // The hold still ends with its press; only the backstop is gone.
            warn!(%error, "could not start the app switcher watchdog");
        }
        Self {
            rows,
            _retire_watchdog: retire,
        }
    }

    fn motion(&self, dx: i16, dy: i16) {
        if let Some(overlay) = lock(&self.rows).as_mut() {
            overlay.motion(dx, dy);
        }
    }
}

impl Drop for ActiveHold {
    fn drop(&mut self) {
        end_rows(&self.rows);
    }
}

fn lock(rows: &Mutex<Option<Overlay>>) -> std::sync::MutexGuard<'_, Option<Overlay>> {
    // The guarded state is an `Option` of plain data; a panic elsewhere
    // leaves it consistent, and the rows must still come down.
    rows.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The open switchers, one per press that started one. The press token is the
/// only key: ending or invalidating a press removes its entry and so takes its
/// rows down.
pub(super) struct HeldAppSwitchers {
    by_press: HashMap<PressToken, ActiveHold>,
    /// Where the rows of an open switcher are drawn and listed.
    port: Arc<dyn SwitcherPort>,
}

impl HeldAppSwitchers {
    pub(super) fn new(port: Arc<dyn SwitcherPort>) -> Self {
        Self {
            by_press: HashMap::new(),
            port,
        }
    }

    /// Open the switcher for `press` and follow its mouse travel from here.
    pub(super) fn start(&mut self, press: &PressToken, params: AppSwitcherParams) {
        let overlay = Overlay::new(Arc::clone(&self.port), params.step());
        // A second start for one press replaces the first, which takes its
        // rows down as it drops.
        self.by_press.insert(
            press.clone(),
            ActiveHold::start(overlay, APP_SWITCHER_HOLD_LIMIT),
        );
    }

    /// Travel before the hold opened, or after it ended, moves nothing.
    pub(super) fn motion(&self, press: &PressToken, dx: i16, dy: i16) {
        if let Some(hold) = self.by_press.get(press) {
            hold.motion(dx, dy);
        } else {
            debug!(dx, dy, "app switcher travel outside a hold — ignored");
        }
    }

    /// The press ended: take the rows down and activate the entry the cursor
    /// was on, if any.
    pub(super) fn end(&mut self, press: &PressToken) {
        self.by_press.remove(press);
    }
}

#[cfg(test)]
mod tests;
