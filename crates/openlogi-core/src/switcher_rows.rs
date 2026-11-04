//! The rows of the app switcher: running windows, pinned apps, recent apps and
//! the most used apps.
//!
//! This module is the single owner of the row model. It decides what each row
//! holds, how many entries it keeps, how a usage history is bounded, and how
//! the mouse moves a cursor across the rows. Sans-I/O: the agent feeds it
//! foreground-app changes and mouse travel and persists the history; the
//! overlay only draws the [`SwitcherRows`] it is handed.
//!
//! Row 0 is the running windows (built elsewhere, so only its length enters
//! here); moving the mouse sideways walks the active row and moving it
//! vertically changes the row.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::app::ForegroundApp;
use crate::binding::{StepPx, SwitchDirection, SwitchStepper};

/// The most entries any row shows.
pub const MAX_ROW_LEN: usize = 10;
/// The most recent apps the history remembers.
pub const MAX_RECENT: usize = 16;
/// The most distinct apps whose activations are counted. The least used are
/// dropped past it, so the persisted file stays small however long it runs.
pub const MAX_TRACKED: usize = 128;
/// The most pinned apps the settings keep.
pub const MAX_PINNED: usize = 16;

/// One row of the switcher. The discriminant is the row's position, top first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SwitcherRow {
    /// The running windows.
    Windows = 0,
    /// The apps the user pinned.
    Pinned = 1,
    /// The apps used last.
    Recent = 2,
    /// The apps activated most often.
    MostUsed = 3,
}

impl SwitcherRow {
    /// Every row, top first.
    pub const ALL: [Self; 4] = [Self::Windows, Self::Pinned, Self::Recent, Self::MostUsed];

    const fn index(self) -> usize {
        self as usize
    }
}

/// What picking an app of a pinned, recent or most used row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activation {
    /// The app is open: bring it to the front.
    Focus,
    /// The app is closed: start it.
    Launch,
}

/// One app of an app row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowApp {
    /// The app, as a per-app profile names it.
    pub app: ForegroundApp,
    /// Whether the app is running now.
    pub open: bool,
}

impl RowApp {
    /// What picking this app does.
    #[must_use]
    pub const fn activation(&self) -> Activation {
        if self.open {
            Activation::Focus
        } else {
            Activation::Launch
        }
    }
}

/// The three app rows. The windows row is not here: its entries are windows,
/// not apps, and come from the window list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitcherRows {
    /// Row 1.
    pub pinned: Vec<RowApp>,
    /// Row 2.
    pub recent: Vec<RowApp>,
    /// Row 3.
    pub most_used: Vec<RowApp>,
}

impl SwitcherRows {
    /// The apps of an app row; the windows row has none.
    #[must_use]
    pub fn row(&self, row: SwitcherRow) -> &[RowApp] {
        match row {
            SwitcherRow::Windows => &[],
            SwitcherRow::Pinned => &self.pinned,
            SwitcherRow::Recent => &self.recent,
            SwitcherRow::MostUsed => &self.most_used,
        }
    }

    /// The entry counts of all four rows, given the windows row's.
    #[must_use]
    pub fn lens(&self, windows: usize) -> [usize; 4] {
        [
            windows,
            self.pinned.len(),
            self.recent.len(),
            self.most_used.len(),
        ]
    }
}

/// The remembered activations: which apps were frontmost last, and how often
/// each was brought to the front. Both are bounded ([`MAX_RECENT`],
/// [`MAX_TRACKED`]), so the persisted form cannot grow without limit.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppUsage {
    /// Most recently frontmost first, each app once.
    recent: Vec<ForegroundApp>,
    /// Activation count per app id.
    counts: BTreeMap<String, UsageCount>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct UsageCount {
    name: String,
    count: u32,
    /// Ordinal of the latest activation, to break ties toward the newer app.
    last: u64,
}

impl AppUsage {
    /// Note that `app` became the frontmost app.
    pub fn record(&mut self, app: &ForegroundApp) {
        self.recent.retain(|seen| seen.id != app.id);
        self.recent.insert(0, app.clone());
        self.recent.truncate(MAX_RECENT);

        let clock = self.counts.values().map(|c| c.last).max().unwrap_or(0) + 1;
        let entry = self
            .counts
            .entry(app.id.clone())
            .or_insert_with(|| UsageCount {
                name: app.display_name.clone(),
                count: 0,
                last: clock,
            });
        entry.count = entry.count.saturating_add(1);
        entry.last = clock;
        entry.name.clone_from(&app.display_name);

        while self.counts.len() > MAX_TRACKED {
            let weakest = self
                .counts
                .iter()
                .min_by_key(|(_, c)| (c.count, c.last))
                .map(|(id, _)| id.clone());
            match weakest {
                Some(id) => self.counts.remove(&id),
                None => break,
            };
        }
    }

    /// How many times `id` was brought to the front.
    #[must_use]
    pub fn count(&self, id: &str) -> u32 {
        self.counts.get(id).map_or(0, |c| c.count)
    }

    /// The remembered display name of `id`, if the app was ever seen.
    fn name_of(&self, id: &str) -> Option<&str> {
        self.counts.get(id).map(|c| c.name.as_str())
    }

    /// Build the app rows.
    ///
    /// `pinned` is the configured bundle ids in order; `running` the ids of
    /// the apps open now; `frontmost` the app in front, which the recent row
    /// leaves out because choosing it would change nothing.
    #[must_use]
    pub fn rows(
        &self,
        pinned: &[String],
        running: &[String],
        frontmost: Option<&str>,
    ) -> SwitcherRows {
        let is_open = |id: &str| running.iter().any(|open| open == id);
        let entry = |app: &ForegroundApp| RowApp {
            open: is_open(&app.id),
            app: app.clone(),
        };

        let mut seen_pinned: Vec<&str> = Vec::new();
        let pinned_row = pinned
            .iter()
            .filter(|id| {
                let fresh = !seen_pinned.contains(&id.as_str());
                seen_pinned.push(id);
                fresh
            })
            .take(MAX_ROW_LEN)
            .map(|id| {
                entry(&ForegroundApp {
                    id: id.clone(),
                    display_name: self.name_of(id).map_or_else(|| id.clone(), str::to_owned),
                })
            })
            .collect();

        let recent = self
            .recent
            .iter()
            .filter(|app| Some(app.id.as_str()) != frontmost)
            .take(MAX_ROW_LEN)
            .map(entry)
            .collect();

        let mut ranked: Vec<(&String, &UsageCount)> = self.counts.iter().collect();
        ranked.sort_by_key(|a| std::cmp::Reverse((a.1.count, a.1.last)));
        let most_used = ranked
            .into_iter()
            .take(MAX_ROW_LEN)
            .map(|(id, c)| {
                entry(&ForegroundApp {
                    id: id.clone(),
                    display_name: c.name.clone(),
                })
            })
            .collect();

        SwitcherRows {
            pinned: pinned_row,
            recent,
            most_used,
        }
    }
}

/// The selection cursor of an open switcher, moved by mouse travel.
///
/// Sideways travel walks the active row, clamped at its ends. Vertical travel
/// moves to the next row that has entries, so an empty row is never landed on.
/// Each row remembers its own column, so going down and back up returns to the
/// entry left behind. A report moves along its dominant axis only, so a
/// slightly diagonal sweep cannot also change row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowsCursor {
    lens: [usize; 4],
    row: SwitcherRow,
    columns: [usize; 4],
    step: StepPx,
    horizontal: SwitchStepper,
    vertical: SwitchStepper,
}

impl RowsCursor {
    /// A cursor on the windows row, starting at column `start` (the window the
    /// native order would pick first), moving once per `step` counts.
    #[must_use]
    pub fn new(lens: [usize; 4], start: usize, step: StepPx) -> Self {
        let mut cursor = Self {
            lens,
            row: SwitcherRow::Windows,
            columns: [0; 4],
            step,
            horizontal: SwitchStepper::new(step),
            vertical: SwitchStepper::new(step),
        };
        cursor.columns[0] = start;
        cursor.clamp_columns();
        cursor
    }

    /// The row the cursor is in.
    #[must_use]
    pub const fn row(&self) -> SwitcherRow {
        self.row
    }

    /// The selected entry within [`Self::row`], or `None` when that row is empty.
    #[must_use]
    pub fn column(&self) -> Option<usize> {
        (self.lens[self.row.index()] > 0).then(|| self.columns[self.row.index()])
    }

    /// Replace the row lengths, for entries that appeared or went while open.
    pub fn set_lens(&mut self, lens: [usize; 4]) {
        self.lens = lens;
        self.clamp_columns();
        if self.lens[self.row.index()] == 0 {
            self.row = SwitcherRow::Windows;
        }
    }

    /// Account for one mouse report. Returns whether the selection changed.
    pub fn motion(&mut self, dx: i16, dy: i16) -> bool {
        if dx.unsigned_abs() >= dy.unsigned_abs() {
            self.vertical = SwitchStepper::new(self.step);
            self.walk(dx)
        } else {
            self.horizontal = SwitchStepper::new(self.step);
            self.change_row(dy)
        }
    }

    fn walk(&mut self, dx: i16) -> bool {
        let Some(run) = self.horizontal.motion(dx) else {
            return false;
        };
        let len = self.lens[self.row.index()];
        if len == 0 {
            return false;
        }
        let column = &mut self.columns[self.row.index()];
        let before = *column;
        let by = usize::from(run.count);
        *column = match run.direction {
            SwitchDirection::Forward => (*column + by).min(len - 1),
            SwitchDirection::Backward => column.saturating_sub(by),
        };
        *column != before
    }

    fn change_row(&mut self, dy: i16) -> bool {
        let Some(run) = self.vertical.motion(dy) else {
            return false;
        };
        let mut row = self.row.index();
        let before = row;
        for _ in 0..run.count {
            let next = match run.direction {
                SwitchDirection::Forward => (row + 1..4).find(|r| self.lens[*r] > 0),
                SwitchDirection::Backward => (0..row).rev().find(|r| self.lens[*r] > 0),
            };
            match next {
                Some(next) => row = next,
                None => break,
            }
        }
        self.row = SwitcherRow::ALL[row];
        row != before
    }

    fn clamp_columns(&mut self) {
        for (column, len) in self.columns.iter_mut().zip(self.lens) {
            *column = (*column).min(len.saturating_sub(1));
        }
    }
}

#[cfg(test)]
mod tests;
