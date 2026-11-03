use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use openlogi_core::binding::{ButtonId, StepPx};

use super::*;

#[derive(Default)]
struct Log {
    steps: Mutex<Vec<SwitchDirection>>,
    released: AtomicUsize,
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
    switchers.start_with(token, stepper(step), FakeKeys(Arc::clone(&log)));
    log
}

#[test]
fn travel_steps_the_switcher_and_release_lets_go_exactly_once() {
    let mut switchers = HeldAppSwitchers::default();
    let token = press(1);
    let log = open(&mut switchers, &token, 60);

    switchers.motion(&token, 70);
    switchers.motion(&token, -130);
    assert_eq!(
        steps(&log),
        [
            SwitchDirection::Forward,
            SwitchDirection::Backward,
            SwitchDirection::Backward,
        ]
    );
    assert_eq!(log.released.load(Ordering::SeqCst), 0, "held while pressed");

    switchers.end(&token);
    assert_eq!(log.released.load(Ordering::SeqCst), 1);
    // The press is over: late travel and a second end are inert.
    switchers.motion(&token, 500);
    switchers.end(&token);
    assert_eq!(steps(&log).len(), 3);
    assert_eq!(log.released.load(Ordering::SeqCst), 1);
}

#[test]
fn travel_for_another_press_or_before_the_hold_steps_nothing() {
    let mut switchers = HeldAppSwitchers::default();
    let held = press(1);
    let log = open(&mut switchers, &held, 60);
    switchers.motion(&press(2), 500);
    assert!(steps(&log).is_empty());
}

#[test]
fn a_press_that_never_ends_releases_at_the_limit() {
    let log = Arc::new(Log::default());
    let hold = ActiveHold::start(
        FakeKeys(Arc::clone(&log)),
        stepper(60),
        Duration::from_millis(30),
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
    hold.motion(500);
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
