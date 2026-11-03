//! The desktop switcher's map of a display's desktops: which apps are on each
//! one, and where their windows sit.
//!
//! The agent reads the windows (macOS only) and the overlay draws them; this
//! module is the data between them and the arithmetic both ends share. It is a
//! schematic by design: window frames, never window titles, so no Screen
//! Recording permission is involved.
//!
//! Frames travel as [`UnitRect`]s, fractions of the display in thousandths, so
//! the payload is small and the overlay scales them to whatever tile it draws.

use serde::{Deserialize, Serialize};

/// The most desktops a schema describes. macOS offers sixteen at most.
pub const MAX_SPACES: usize = 16;
/// The most windows a desktop's schema keeps, front-most first.
pub const MAX_WINDOWS: usize = 12;
/// The most distinct apps a schema names.
pub const MAX_APPS: usize = 48;
/// The most apps whose icons a desktop's tile shows.
pub const MAX_ICONS: usize = 3;
/// The resolution of a [`UnitRect`]: a full display side.
pub const UNIT: u16 = 1000;
/// A window narrower or shorter than this, in points, is not worth a line on
/// the map (Stage Manager's thumbnails, helper panels).
pub const MIN_WINDOW_POINTS: f64 = 100.0;

/// An axis-aligned rectangle in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// A rectangle as thousandths of a display's width and height.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitRect {
    /// Left edge, `0..=UNIT`.
    pub x: u16,
    /// Top edge, `0..=UNIT`.
    pub y: u16,
    /// Width, at most `UNIT - x`.
    pub width: u16,
    /// Height, at most `UNIT - y`.
    pub height: u16,
}

impl UnitRect {
    /// Where `window` lies on `display`, clipped to it. `None` when the window
    /// is smaller than [`MIN_WINDOW_POINTS`] on a side, lies outside the
    /// display, or the display has no area.
    #[must_use]
    pub fn within(display: Frame, window: Frame) -> Option<Self> {
        let finite = |frame: Frame| {
            [frame.x, frame.y, frame.width, frame.height]
                .iter()
                .all(|value| value.is_finite())
        };
        if !(finite(display) && finite(window) && display.width > 0.0 && display.height > 0.0) {
            return None;
        }
        if !(window.width >= MIN_WINDOW_POINTS && window.height >= MIN_WINDOW_POINTS) {
            return None;
        }
        let left = window.x.max(display.x);
        let top = window.y.max(display.y);
        let right = (window.x + window.width).min(display.x + display.width);
        let bottom = (window.y + window.height).min(display.y + display.height);
        if !(right > left && bottom > top) {
            return None;
        }
        let unit = f64::from(UNIT);
        let along = |value: f64, origin: f64, extent: f64| {
            // Clamped to 0..=UNIT, so the cast cannot truncate or wrap.
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "clamped to 0..=UNIT first"
            )]
            let scaled = ((value - origin) / extent * unit).round().clamp(0.0, unit) as u16;
            scaled
        };
        let x = along(left, display.x, display.width);
        let y = along(top, display.y, display.height);
        let right = along(right, display.x, display.width);
        let bottom = along(bottom, display.y, display.height);
        let (width, height) = (right.saturating_sub(x), bottom.saturating_sub(y));
        (width > 0 && height > 0).then_some(Self {
            x,
            y,
            width,
            height,
        })
    }

    /// The area as a fraction of the display, `0.0..=1.0`.
    #[must_use]
    pub fn area(self) -> f32 {
        let unit = f32::from(UNIT);
        f32::from(self.width) / unit * (f32::from(self.height) / unit)
    }

    /// The rectangle in a tile of `width` by `height`, as `[x, y, width,
    /// height]`. Out-of-range values from a skewed sender are clamped to the
    /// tile, and the rectangle is never thinner than `min_side`.
    #[must_use]
    pub fn scaled(self, width: f32, height: f32, min_side: f32) -> [f32; 4] {
        let unit = f32::from(UNIT);
        let fraction = |value: u16| f32::from(value.min(UNIT)) / unit;
        let (left, top) = (fraction(self.x), fraction(self.y));
        let right = (left + fraction(self.width)).min(1.0);
        let bottom = (top + fraction(self.height)).min(1.0);
        let (w, h) = (
            ((right - left) * width).max(min_side).min(width),
            ((bottom - top) * height).max(min_side).min(height),
        );
        [
            (left * width).min(width - w),
            (top * height).min(height - h),
            w,
            h,
        ]
    }
}

/// An app that has a window on one of the desktops.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaApp {
    /// The bundle identifier; stable per app, so it also picks the app's tint.
    pub id: String,
    /// The name the user sees, for a full-screen desktop.
    pub name: String,
    /// A PNG of the app's icon the agent wrote, when it could.
    pub icon: Option<String>,
}

/// One window on a desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaWindow {
    /// The owning app: an index into [`DesktopSchema::apps`].
    pub app: u16,
    /// Where it sits on the display.
    pub frame: UnitRect,
}

/// What one desktop holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceSchema {
    /// Whether it is an app's full-screen space.
    pub fullscreen: bool,
    /// Its windows, front-most first, at most [`MAX_WINDOWS`].
    pub windows: Vec<SchemaWindow>,
}

/// The map of a display's desktops, in Mission Control order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopSchema {
    /// The apps the windows belong to.
    pub apps: Vec<SchemaApp>,
    /// One entry per desktop, in order.
    pub spaces: Vec<SpaceSchema>,
}

impl DesktopSchema {
    /// The apps of `space`, largest total window area first, at most `limit`.
    /// Ties keep the front-most window's app first.
    #[must_use]
    pub fn top_apps(&self, space: usize, limit: usize) -> Vec<&SchemaApp> {
        self.top_app_indices(space, limit)
            .into_iter()
            .filter_map(|app| self.app(app))
            .collect()
    }

    /// Like [`Self::top_apps`], as indices into [`Self::apps`]. Indices the
    /// table does not hold are skipped.
    #[must_use]
    pub fn top_app_indices(&self, space: usize, limit: usize) -> Vec<u16> {
        let Some(space) = self.spaces.get(space) else {
            return Vec::new();
        };
        let mut totals: Vec<(u16, f32)> = Vec::new();
        for window in &space.windows {
            match totals.iter_mut().find(|(app, _)| *app == window.app) {
                Some((_, area)) => *area += window.frame.area(),
                None => totals.push((window.app, window.frame.area())),
            }
        }
        // Stable: equal areas stay in front-to-back order.
        totals.sort_by(|a, b| b.1.total_cmp(&a.1));
        totals
            .into_iter()
            .map(|(app, _)| app)
            .filter(|&app| self.app(app).is_some())
            .take(limit)
            .collect()
    }

    /// The app that owns `app`, if the index is one of this schema's.
    #[must_use]
    pub fn app(&self, app: u16) -> Option<&SchemaApp> {
        self.apps.get(usize::from(app))
    }
}

/// A stable hue in `0.0..1.0` for an app, from its identifier, so an app keeps
/// its tint from one switch to the next and across processes.
#[must_use]
pub fn tint_hue(id: &str) -> f32 {
    // FNV-1a: no dependency, and the same on every platform and run.
    let hash = id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    f32::from(u16::try_from(hash % 360).unwrap_or(0)) / 360.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISPLAY: Frame = Frame {
        x: 0.0,
        y: 0.0,
        width: 1000.0,
        height: 500.0,
    };

    fn frame(x: f64, y: f64, width: f64, height: f64) -> Frame {
        Frame {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_window_scales_to_thousandths_of_the_display() {
        let rect = UnitRect::within(DISPLAY, frame(100.0, 50.0, 500.0, 250.0)).unwrap();
        assert_eq!(
            rect,
            UnitRect {
                x: 100,
                y: 100,
                width: 500,
                height: 500
            }
        );
    }

    #[test]
    fn the_display_origin_need_not_be_zero() {
        let second = frame(-1000.0, 200.0, 1000.0, 500.0);
        let rect = UnitRect::within(second, frame(-1000.0, 200.0, 500.0, 500.0)).unwrap();
        assert_eq!((rect.x, rect.y, rect.width, rect.height), (0, 0, 500, 1000));
    }

    #[test]
    fn a_window_hanging_off_the_display_is_clipped() {
        let rect = UnitRect::within(DISPLAY, frame(800.0, -50.0, 400.0, 200.0)).unwrap();
        assert_eq!((rect.x, rect.y), (800, 0));
        assert_eq!(rect.x + rect.width, UNIT);
        assert_eq!(rect.height, 300);
    }

    #[test]
    fn tiny_offscreen_and_degenerate_windows_are_dropped() {
        assert_eq!(
            UnitRect::within(DISPLAY, frame(0.0, 0.0, 90.0, 400.0)),
            None
        );
        assert_eq!(
            UnitRect::within(DISPLAY, frame(0.0, 0.0, 400.0, 20.0)),
            None
        );
        assert_eq!(
            UnitRect::within(DISPLAY, frame(2000.0, 0.0, 400.0, 400.0)),
            None
        );
        assert_eq!(
            UnitRect::within(frame(0.0, 0.0, 0.0, 0.0), frame(0.0, 0.0, 400.0, 400.0)),
            None
        );
        assert_eq!(
            UnitRect::within(DISPLAY, frame(f64::NAN, 0.0, 400.0, 400.0)),
            None
        );
    }

    #[test]
    fn a_rect_scales_into_a_tile_and_stays_inside_it() {
        let half = UnitRect {
            x: 500,
            y: 0,
            width: 500,
            height: 1000,
        };
        assert_eq!(half.scaled(80.0, 50.0, 3.0), [40.0, 0.0, 40.0, 50.0]);
        let sliver = UnitRect {
            x: 990,
            y: 990,
            width: 10,
            height: 10,
        };
        let [x, y, w, h] = sliver.scaled(80.0, 50.0, 3.0);
        assert!((w - 3.0).abs() < f32::EPSILON && (h - 3.0).abs() < f32::EPSILON);
        assert!(x + w <= 80.0 && y + h <= 50.0, "{x} {y} {w} {h}");
        let hostile = UnitRect {
            x: u16::MAX,
            y: u16::MAX,
            width: u16::MAX,
            height: u16::MAX,
        };
        let [x, y, w, h] = hostile.scaled(80.0, 50.0, 3.0);
        assert!(x >= 0.0 && y >= 0.0 && x + w <= 80.0 && y + h <= 50.0);
    }

    fn app(id: &str) -> SchemaApp {
        SchemaApp {
            id: id.into(),
            name: id.into(),
            icon: None,
        }
    }

    fn window(app: u16, width: u16, height: u16) -> SchemaWindow {
        SchemaWindow {
            app,
            frame: UnitRect {
                x: 0,
                y: 0,
                width,
                height,
            },
        }
    }

    #[test]
    fn the_biggest_apps_come_first_and_ties_keep_the_front_order() {
        let schema = DesktopSchema {
            apps: vec![app("a"), app("b"), app("c"), app("d")],
            spaces: vec![SpaceSchema {
                fullscreen: false,
                windows: vec![
                    window(2, 200, 200),
                    window(0, 400, 400),
                    window(1, 200, 200),
                    window(0, 100, 100),
                    window(3, 50, 50),
                ],
            }],
        };
        let ids: Vec<_> = schema
            .top_apps(0, 3)
            .into_iter()
            .map(|app| app.id.as_str())
            .collect();
        assert_eq!(ids, ["a", "c", "b"]);
        assert!(schema.top_apps(1, 3).is_empty(), "no such desktop");
    }

    #[test]
    fn an_app_index_outside_the_table_is_skipped() {
        let schema = DesktopSchema {
            apps: vec![app("a")],
            spaces: vec![SpaceSchema {
                fullscreen: false,
                windows: vec![window(9, 500, 500), window(0, 100, 100)],
            }],
        };
        assert_eq!(schema.top_apps(0, 3).len(), 1);
        assert!(schema.app(9).is_none());
    }

    #[test]
    fn a_tint_is_stable_and_spreads_apps() {
        assert!((tint_hue("com.apple.Safari") - tint_hue("com.apple.Safari")).abs() < f32::EPSILON);
        assert!(
            (tint_hue("com.apple.Safari") - tint_hue("com.apple.Terminal")).abs() > f32::EPSILON
        );
        assert!((0.0..1.0).contains(&tint_hue("")));
    }
}
