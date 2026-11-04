use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use openlogi_core::app::ForegroundApp;
use openlogi_core::binding::{ButtonId, StepPx};
use openlogi_core::switcher_rows::{SwitcherRows, SwitcherView, SwitcherWindow};
use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use super::*;

#[derive(Default)]
struct Log {
    steps: Mutex<Vec<SwitchDirection>>,
    released: AtomicUsize,
    cancelled: AtomicUsize,
}

/// Records the steps it is asked for and counts its own release.
struct FakeKeys(Arc<Log>);

impl SwitcherKeys for FakeKeys {
    fn step(&mut self, direction: SwitchDirection) {
        self.0
            .steps
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(direction);
    }

    fn cancel(&mut self) {
        self.0.cancelled.fetch_add(1, Ordering::SeqCst);
    }
}

impl Drop for FakeKeys {
    fn drop(&mut self) {
        self.0.released.fetch_add(1, Ordering::SeqCst);
    }
}

fn stepper(step: u16) -> SwitchStepper {
    SwitchStepper::new(StepPx::try_from(step).expect("in range"))
}

fn press(id: u64) -> PressToken {
    PressToken::hook_for_test(id, ButtonId::DpiToggle)
}

fn steps(log: &Log) -> Vec<SwitchDirection> {
    log.steps
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn open(switchers: &mut HeldAppSwitchers<FakeKeys>, token: &PressToken, step: u16) -> Arc<Log> {
    let log = Arc::new(Log::default());
    switchers.start_with(token, stepper(step), FakeKeys(Arc::clone(&log)), None);
    log
}

#[test]
fn travel_steps_the_switcher_and_release_lets_go_exactly_once() {
    let mut switchers = HeldAppSwitchers::default();
    let token = press(1);
    let log = open(&mut switchers, &token, 60);

    switchers.motion(&token, 70, 0);
    switchers.motion(&token, -70, 0);
    assert_eq!(
        steps(&log),
        [SwitchDirection::Forward, SwitchDirection::Backward,]
    );
    assert_eq!(log.released.load(Ordering::SeqCst), 0, "held while pressed");

    switchers.end(&token);
    assert_eq!(log.released.load(Ordering::SeqCst), 1);
    // The press is over: late travel and a second end are inert.
    switchers.motion(&token, 500, 0);
    switchers.end(&token);
    assert_eq!(steps(&log).len(), 2);
    assert_eq!(log.released.load(Ordering::SeqCst), 1);
}

#[test]
fn travel_for_another_press_or_before_the_hold_steps_nothing() {
    let mut switchers = HeldAppSwitchers::default();
    let held = press(1);
    let log = open(&mut switchers, &held, 60);
    switchers.motion(&press(2), 500, 0);
    assert!(steps(&log).is_empty());
}

#[test]
fn a_press_that_never_ends_releases_at_the_limit() {
    let log = Arc::new(Log::default());
    let hold = ActiveHold::start(
        FakeKeys(Arc::clone(&log)),
        stepper(60),
        Duration::from_millis(30),
        None,
    );
    for _ in 0..200 {
        if log.released.load(Ordering::SeqCst) > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        log.released.load(Ordering::SeqCst),
        1,
        "the watchdog let go"
    );

    // A lost release arriving afterwards finds nothing left to release,
    // and travel can no longer type into whatever is now focused.
    hold.motion(500, 0);
    assert!(steps(&log).is_empty());
    drop(hold);
    assert_eq!(log.released.load(Ordering::SeqCst), 1);
}

#[test]
fn ending_before_the_limit_retires_the_watchdog_without_a_second_release() {
    let log = Arc::new(Log::default());
    let hold = ActiveHold::start(
        FakeKeys(Arc::clone(&log)),
        stepper(60),
        Duration::from_millis(40),
        None,
    );
    drop(hold);
    assert_eq!(log.released.load(Ordering::SeqCst), 1);
    thread::sleep(Duration::from_millis(120));
    assert_eq!(
        log.released.load(Ordering::SeqCst),
        1,
        "a retired watchdog must not release again"
    );
}

#[test]
fn restarting_a_press_releases_the_hold_it_replaces() {
    let mut switchers = HeldAppSwitchers::default();
    let token = press(1);
    let first = open(&mut switchers, &token, 60);
    let second = open(&mut switchers, &token, 60);
    assert_eq!(first.released.load(Ordering::SeqCst), 1);
    assert_eq!(second.released.load(Ordering::SeqCst), 0);
    switchers.end(&token);
    assert_eq!(second.released.load(Ordering::SeqCst), 1);
}

#[test]
fn the_modifier_outlasts_the_last_typed_key_by_the_settle_time() {
    let log = Arc::new(Log::default());
    let settle = Duration::from_millis(80);
    let mut hold = SwitcherHold {
        keys: Some(FakeKeys(Arc::clone(&log))),
        stepper: stepper(60),
        stepped: false,
        last_key: Instant::now(),
        settle,
        overlay: None,
    };
    hold.motion(70, 0);
    let stepped = Instant::now();
    assert!(hold.release());
    assert!(stepped.elapsed() >= settle.saturating_sub(Duration::from_millis(5)));
    assert_eq!(log.released.load(Ordering::SeqCst), 1);
    // A hold that was left alone long enough lets go without waiting.
    let idle = Arc::new(Log::default());
    let mut hold = SwitcherHold {
        keys: Some(FakeKeys(Arc::clone(&idle))),
        stepper: stepper(60),
        stepped: true,
        last_key: Instant::now().checked_sub(settle).expect("uptime"),
        settle,
        overlay: None,
    };
    let started = Instant::now();
    assert!(hold.release());
    assert!(started.elapsed() < Duration::from_millis(40));
    assert!(!hold.release());
}

#[test]
fn a_hold_without_travel_is_cancelled_on_release() {
    let mut switchers = HeldAppSwitchers::default();
    let token = press(1);
    let log = open(&mut switchers, &token, 60);
    // Travel short of a step is still no step.
    switchers.motion(&token, 30, 0);
    switchers.end(&token);
    assert!(steps(&log).is_empty());
    assert_eq!(log.cancelled.load(Ordering::SeqCst), 1);
    assert_eq!(log.released.load(Ordering::SeqCst), 1);
}

#[test]
fn a_hold_that_stepped_opens_the_chosen_app_on_release() {
    let mut switchers = HeldAppSwitchers::default();
    let token = press(1);
    let log = open(&mut switchers, &token, 60);
    switchers.motion(&token, 70, 0);
    switchers.end(&token);
    assert_eq!(steps(&log), [SwitchDirection::Forward]);
    assert_eq!(log.cancelled.load(Ordering::SeqCst), 0);
    assert_eq!(log.released.load(Ordering::SeqCst), 1);
}

/// A port that records what the hold shows and takes down.
#[derive(Default)]
struct RecordingPort {
    shown: Mutex<Vec<SwitcherView>>,
    hidden: AtomicUsize,
    activated: Mutex<Vec<SwitcherTarget>>,
}

impl SwitcherPort for RecordingPort {
    fn snapshot(&self) -> SwitcherView {
        let window = |id| SwitcherWindow {
            id,
            pid: 1,
            app: ForegroundApp::unnamed("a".to_owned()),
            title: String::new(),
        };
        SwitcherView::new(
            vec![window(1), window(2)],
            SwitcherRows::default(),
            BTreeMap::new(),
        )
    }

    fn show(&self, view: &SwitcherView) {
        self.shown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(view.clone());
    }

    fn hide(&self) {
        self.hidden.fetch_add(1, Ordering::SeqCst);
    }

    fn activate(&self, target: &SwitcherTarget) {
        self.activated
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(target.clone());
    }
}

#[test]
fn travel_draws_the_rows_and_release_takes_them_down() {
    let port = Arc::new(RecordingPort::default());
    let log = Arc::new(Log::default());
    let overlay = Overlay::new(
        Arc::clone(&port) as Arc<dyn SwitcherPort>,
        StepPx::try_from(60).expect("in range"),
    );
    let mut hold = SwitcherHold {
        keys: Some(FakeKeys(Arc::clone(&log))),
        stepper: stepper(60),
        stepped: false,
        last_key: Instant::now(),
        settle: Duration::ZERO,
        overlay: Some(overlay),
    };
    hold.motion(0, 70);
    let shown = || {
        port.shown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    };
    assert_eq!(shown(), 1, "the first travel opens and shows the rows");
    hold.motion(-70, 0);
    assert_eq!(shown(), 2, "a move of the selection shows it again");
    assert_eq!(port.hidden.load(Ordering::SeqCst), 0);
    assert!(hold.release());
    assert_eq!(
        port.hidden.load(Ordering::SeqCst),
        1,
        "release takes the rows down"
    );
    assert!(!hold.release(), "a second release does nothing");
    assert_eq!(port.hidden.load(Ordering::SeqCst), 1);
}

#[test]
fn a_hold_without_travel_never_reads_the_windows() {
    let port = Arc::new(RecordingPort::default());
    let log = Arc::new(Log::default());
    let overlay = Overlay::new(
        Arc::clone(&port) as Arc<dyn SwitcherPort>,
        StepPx::try_from(60).expect("in range"),
    );
    let mut hold = SwitcherHold {
        keys: Some(FakeKeys(log)),
        stepper: stepper(60),
        stepped: false,
        last_key: Instant::now(),
        settle: Duration::ZERO,
        overlay: Some(overlay),
    };
    assert!(hold.release());
    assert!(
        port.shown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    );
    assert_eq!(
        port.hidden.load(Ordering::SeqCst),
        0,
        "nothing was drawn to take down"
    );
}

#[test]
fn releasing_on_a_window_dismisses_the_native_switcher_and_raises_it() {
    let port = Arc::new(RecordingPort::default());
    let log = Arc::new(Log::default());
    let overlay = Overlay::new(
        Arc::clone(&port) as Arc<dyn SwitcherPort>,
        StepPx::try_from(60).expect("in range"),
    );
    let mut hold = SwitcherHold {
        keys: Some(FakeKeys(Arc::clone(&log))),
        stepper: stepper(60),
        stepped: false,
        last_key: Instant::now(),
        settle: Duration::ZERO,
        overlay: Some(overlay),
    };
    // Down through the rows: the first travel opens on the second window,
    // and moving left selects the first one.
    hold.motion(-70, 0);
    assert!(hold.release());
    assert_eq!(
        log.cancelled.load(Ordering::SeqCst),
        1,
        "the native switcher is dismissed"
    );
    assert_eq!(log.released.load(Ordering::SeqCst), 1);
    assert_eq!(
        *port
            .activated
            .lock()
            .unwrap_or_else(PoisonError::into_inner),
        [SwitcherTarget::Window { pid: 1, id: 1 }],
        "the window the cursor was on comes forward"
    );
}

#[test]
fn a_hold_that_never_moved_activates_nothing() {
    let port = Arc::new(RecordingPort::default());
    let log = Arc::new(Log::default());
    let overlay = Overlay::new(
        Arc::clone(&port) as Arc<dyn SwitcherPort>,
        StepPx::try_from(60).expect("in range"),
    );
    let mut hold = SwitcherHold {
        keys: Some(FakeKeys(Arc::clone(&log))),
        stepper: stepper(60),
        stepped: false,
        last_key: Instant::now(),
        settle: Duration::ZERO,
        overlay: Some(overlay),
    };
    assert!(hold.release());
    assert_eq!(
        log.cancelled.load(Ordering::SeqCst),
        1,
        "a hold without travel cancels"
    );
    assert!(
        port.activated
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    );
}
