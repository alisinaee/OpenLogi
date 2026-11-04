use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use openlogi_core::app::ForegroundApp;
use openlogi_core::binding::{AppSwitcherParams, ButtonId, StepPx};
use openlogi_core::switcher_rows::{SwitcherRows, SwitcherView, SwitcherWindow};
use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use super::*;

/// A port that records what the hold shows and takes down, and lists two
/// windows of one app when a switcher opens.
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

fn recorder() -> Arc<RecordingPort> {
    Arc::new(RecordingPort::default())
}

fn press(id: u64) -> PressToken {
    PressToken::hook_for_test(id, ButtonId::DpiToggle)
}

fn params(step: u16) -> AppSwitcherParams {
    AppSwitcherParams::new(Some(StepPx::try_from(step).expect("in range")), None)
}

fn overlay(port: &Arc<RecordingPort>, step: u16) -> Overlay {
    Overlay::new(
        Arc::clone(port) as Arc<dyn SwitcherPort>,
        StepPx::try_from(step).expect("in range"),
    )
}

fn shown(port: &RecordingPort) -> usize {
    port.shown
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .len()
}

fn hidden(port: &RecordingPort) -> usize {
    port.hidden.load(Ordering::SeqCst)
}

fn activated(port: &RecordingPort) -> Vec<SwitcherTarget> {
    port.activated
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

#[test]
fn travel_draws_the_rows_and_release_takes_them_down() {
    let port = recorder();
    let mut switchers = HeldAppSwitchers::new(port.clone());
    let token = press(1);
    switchers.start(&token, params(60));

    switchers.motion(&token, 0, 70);
    assert_eq!(shown(&port), 1, "the first travel opens and shows the rows");
    switchers.motion(&token, -70, 0);
    assert_eq!(shown(&port), 2, "a move of the selection shows it again");
    assert_eq!(hidden(&port), 0, "held while pressed");

    switchers.end(&token);
    assert_eq!(hidden(&port), 1, "release takes the rows down");
    // The press is over: late travel and a second end are inert.
    switchers.motion(&token, -70, 0);
    switchers.end(&token);
    assert_eq!(shown(&port), 2);
    assert_eq!(hidden(&port), 1);
}

#[test]
fn release_on_a_window_activates_it_and_nothing_else() {
    let port = recorder();
    let mut switchers = HeldAppSwitchers::new(port.clone());
    let token = press(1);
    switchers.start(&token, params(60));
    // Down through the rows: the first travel opens on the second window,
    // and moving left selects the first one.
    switchers.motion(&token, -70, 0);
    switchers.end(&token);
    assert_eq!(hidden(&port), 1);
    assert_eq!(
        activated(&port),
        [SwitcherTarget::Window { pid: 1, id: 1 }],
        "the window the cursor was on comes forward"
    );
}

#[test]
fn a_hold_that_never_moved_reads_no_windows_and_activates_nothing() {
    let port = recorder();
    let mut switchers = HeldAppSwitchers::new(port.clone());
    let token = press(1);
    switchers.start(&token, params(60));
    switchers.end(&token);
    assert_eq!(shown(&port), 0, "nothing was drawn");
    assert_eq!(hidden(&port), 0, "nothing was drawn to take down");
    assert!(activated(&port).is_empty());
}

#[test]
fn travel_for_another_press_or_after_the_release_moves_nothing() {
    let port = recorder();
    let mut switchers = HeldAppSwitchers::new(port.clone());
    let held = press(1);
    switchers.start(&held, params(60));
    switchers.motion(&press(2), -70, 0);
    assert_eq!(shown(&port), 0);

    switchers.end(&held);
    switchers.motion(&held, -70, 0);
    assert_eq!(shown(&port), 0);
    assert!(activated(&port).is_empty());
}

#[test]
fn restarting_a_press_takes_down_the_rows_it_replaces() {
    let port = recorder();
    let mut switchers = HeldAppSwitchers::new(port.clone());
    let token = press(1);
    switchers.start(&token, params(60));
    switchers.motion(&token, -70, 0);
    assert_eq!(shown(&port), 1);

    switchers.start(&token, params(60));
    assert_eq!(hidden(&port), 1, "the replaced rows come down");
    assert_eq!(activated(&port).len(), 1, "as a release of them would");
    switchers.end(&token);
    assert_eq!(hidden(&port), 1, "the new hold drew nothing to take down");
    assert_eq!(activated(&port).len(), 1);
}

#[test]
fn a_press_that_never_ends_takes_its_rows_down_at_the_limit() {
    let port = recorder();
    let hold = ActiveHold::start(overlay(&port, 60), Duration::from_millis(30));
    hold.motion(-70, 0);
    assert_eq!(shown(&port), 1);
    for _ in 0..200 {
        if hidden(&port) > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(hidden(&port), 1, "the watchdog took the rows down");
    assert_eq!(activated(&port).len(), 1, "as a release would have");

    // A lost release arriving afterwards finds no rows, and travel draws
    // nothing new.
    hold.motion(-70, 0);
    assert_eq!(shown(&port), 1);
    drop(hold);
    assert_eq!(hidden(&port), 1);
    assert_eq!(activated(&port).len(), 1);
}

#[test]
fn ending_before_the_limit_retires_the_watchdog_without_a_second_end() {
    let port = recorder();
    let hold = ActiveHold::start(overlay(&port, 60), Duration::from_millis(40));
    hold.motion(-70, 0);
    drop(hold);
    assert_eq!(hidden(&port), 1);
    thread::sleep(Duration::from_millis(120));
    assert_eq!(
        hidden(&port),
        1,
        "a retired watchdog must not take the rows down again"
    );
    assert_eq!(activated(&port).len(), 1);
}
