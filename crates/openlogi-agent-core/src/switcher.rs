//! What the app switcher shows, and where it shows it.
//!
//! The button runtime decides when a switcher opens and how its cursor moves;
//! this module answers the two questions that need the rest of the agent: which
//! rows a switcher opened now would hold, and how to show or take down the one
//! on screen. The rows are built from the usage history and settings the
//! orchestrator keeps current in [`SwitcherInputs`], and the windows the OS
//! lists at the moment the switcher opens.

use std::sync::Arc;

use openlogi_core::switcher_rows::{Activation, AppUsage, Selection, SwitcherView};

use crate::observable::ObservableState;

/// The settings and history the app switcher's rows are built from, as the
/// orchestrator last set them.
#[derive(Clone, Debug, Default)]
pub struct SwitcherInputs {
    /// The pinned apps' bundle ids, in order.
    pub pinned: Vec<String>,
    /// The activation history behind the Recent and Most used rows.
    pub usage: AppUsage,
    /// The app in front, which the Recent row leaves out.
    pub frontmost: Option<String>,
    /// The UI locale the switcher's text is shown in.
    pub language: Option<String>,
}

/// The host of the app switcher: builds the rows a switcher opens with, and
/// shows or hides the one on screen.
pub trait SwitcherPort: Send + Sync {
    /// The rows and windows a switcher opened now would show, with the cursor
    /// not yet placed.
    fn snapshot(&self) -> SwitcherView;

    /// Show `view` as the open switcher, replacing the one on show.
    fn show(&self, view: &SwitcherView);

    /// The switcher closed: take it down.
    fn hide(&self);

    /// Bring the entry the cursor was on to the front, or start its app.
    fn activate(&self, target: &SwitcherTarget);
}

/// What releasing the switcher on an entry does, owned so it outlives the
/// rows it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitcherTarget {
    /// Bring this window to the front, with its app.
    Window {
        /// The process that owns the window.
        pid: i32,
        /// The window server's id of the window.
        id: u32,
    },
    /// Bring this app to the front when it runs, or start it.
    App {
        /// The app's bundle identifier.
        id: String,
        /// Whether the app runs now, which decides between the two.
        activation: Activation,
    },
}

impl SwitcherTarget {
    /// The target of the entry `selection` names.
    #[must_use]
    pub fn of(selection: Selection<'_>) -> Self {
        match selection {
            Selection::Window(window) => Self::Window {
                pid: window.pid,
                id: window.id,
            },
            Selection::App(app) => Self::App {
                id: app.app.id.clone(),
                activation: app.activation(),
            },
        }
    }
}

/// A port that lists nothing and draws nothing. A runtime without an overlay
/// host uses it, and the hold then behaves as it did before the overlay.
#[derive(Debug, Default)]
pub struct NoSwitcherOverlay;

impl SwitcherPort for NoSwitcherOverlay {
    fn snapshot(&self) -> SwitcherView {
        SwitcherView::default()
    }

    fn show(&self, _view: &SwitcherView) {}

    fn hide(&self) {}

    fn activate(&self, _target: &SwitcherTarget) {}
}

/// The agent's own port: the windows the OS lists, the rows from
/// [`SwitcherInputs`], and the notice board the overlay reads.
pub struct ObservableSwitcher {
    observable: Arc<ObservableState>,
}

impl ObservableSwitcher {
    /// A port over the agent's observable state.
    #[must_use]
    pub const fn new(observable: Arc<ObservableState>) -> Self {
        Self { observable }
    }
}

impl SwitcherPort for ObservableSwitcher {
    fn snapshot(&self) -> SwitcherView {
        let (windows, icons) = openlogi_inject::open_windows();
        // An app with a window is running; the rows only ever ask that.
        let running: Vec<String> = windows.iter().map(|w| w.app.id.clone()).collect();
        let rows = self.observable.read_switcher(|inputs| {
            inputs
                .usage
                .rows(&inputs.pinned, &running, inputs.frontmost.as_deref())
        });
        SwitcherView::new(windows, rows, icons)
    }

    fn show(&self, view: &SwitcherView) {
        let language = self
            .observable
            .read_switcher(|inputs| inputs.language.clone());
        self.observable
            .notices()
            .post_app_switcher(view.clone(), language);
    }

    fn hide(&self) {
        self.observable.notices().end_app_switcher();
    }

    fn activate(&self, target: &SwitcherTarget) {
        match target {
            SwitcherTarget::Window { pid, id } => openlogi_inject::focus_window(*pid, *id),
            SwitcherTarget::App { id, activation } => {
                // A running app is brought forward; one that stopped since the
                // rows were read is started instead.
                let running = *activation == Activation::Focus && openlogi_inject::activate_app(id);
                if !running {
                    openlogi_inject::launch_app(id);
                }
            }
        }
    }
}
