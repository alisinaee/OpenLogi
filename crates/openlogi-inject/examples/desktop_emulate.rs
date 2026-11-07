//! Emulates the desktop switcher's feed on this Mac: performs real Space
//! switches through the same `execute` path the agent uses, with the animated
//! slide on, and prints every `DesktopPosition` the observer receives with its
//! time from the first switch. Local debugging only; not part of the product.
//!
//! Usage: `cargo run -p openlogi-inject --example desktop_emulate -- [steps] [prev]`

use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

use openlogi_core::binding::Action;
use openlogi_inject::{
    DesktopPosition, DesktopSwitch, execute, set_desktop_observer, set_desktop_schema,
    set_desktop_switch,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let steps: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let backwards = args.next().as_deref() == Some("prev");
    // Presses that move the Space, which the restore undoes. A press at an
    // edge moves nothing, so pass it when the run hits one.
    let restore: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(steps);
    let (forward, back) = if backwards {
        (Action::PreviousDesktop, Action::NextDesktop)
    } else {
        (Action::NextDesktop, Action::PreviousDesktop)
    };

    let origin = Instant::now();
    let log: Arc<Mutex<Vec<(u128, u16, u16, bool)>>> = Arc::default();
    let sink = Arc::clone(&log);
    set_desktop_observer(Some(Arc::new(move |p: DesktopPosition| {
        sink.lock().unwrap().push((
            origin.elapsed().as_millis(),
            p.count,
            p.current,
            p.schema.is_some(),
        ));
    })));
    set_desktop_switch(DesktopSwitch::Animated);
    set_desktop_schema(true);

    for step in 0..steps {
        let at = origin.elapsed().as_millis();
        println!("t={at:>5}ms  press {step}");
        execute(&forward);
        sleep(Duration::from_millis(1800));
    }
    sleep(Duration::from_millis(1500));
    println!("--- observer timeline (ms since start) ---");
    for (t, count, current, schema) in log.lock().unwrap().iter() {
        println!("t={t:>5}ms  count={count} current={current} schema={schema}");
    }

    // Put the Space back where it was.
    for _ in 0..restore {
        execute(&back);
        sleep(Duration::from_millis(1800));
    }
    set_desktop_observer(None);
}
