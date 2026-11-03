//! What each desktop holds, for the desktop switcher: which apps have windows
//! on it and where those sit, read after a switch is confirmed.
//!
//! The window-to-Space mapping is SkyLight's `SLSCopyWindowsWithOptionsAndTags`
//! (read-only, like the Space list beside it); the owner, frame and layer of
//! each window come from the public `CGWindowListCopyWindowInfo`. Window
//! *titles* are never read, which is what keeps this clear of the Screen
//! Recording permission. Each app's icon is rendered once into a PNG in the
//! data directory, which the overlay then loads as a plain file.
//!
//! Everything here is bounded: a read past its budget, or any failure, gives
//! `None`, and the switcher falls back to plain numbered boxes.

use std::collections::{HashMap, HashSet};
use std::ffi::c_int;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant, UNIX_EPOCH};

use core_graphics::display::CGDisplay;
use objc2::AnyThread as _;
use objc2::rc::autoreleasepool;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSBitmapImageFileType, NSBitmapImageRep,
    NSBitmapImageRepPropertyKey, NSRunningApplication, NSWorkspace,
};
use objc2_core_foundation::{CFArray, CFNumber, CFRetained, CFType};
use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};
use openlogi_core::desktop_schema::{
    DesktopSchema, Frame, MAX_APPS, MAX_ICONS, MAX_SPACES, MAX_WINDOWS, SchemaApp, SchemaWindow,
    SpaceSchema, UnitRect,
};

use super::super::app_services;
use super::{API, Dictionary, SpaceState, dictionary, get, number};

/// `SLSCopyWindowsWithOptionsAndTags`' option set that lists every window on a
/// Space, minimised and off-screen ones included (the set yabai and Windows
/// Manager use).
const WINDOW_OPTIONS: u32 = 0x7;
/// The side, in points, the icon is rendered for; the overlay draws it
/// smaller, at up to 3x.
const ICON_POINTS: f64 = 64.0;

type CopyWindows =
    unsafe extern "C" fn(c_int, u32, *const CFArray, u32, *mut u64, *mut u64) -> *mut CFArray;

static COPY_WINDOWS: LazyLock<Option<CopyWindows>> = LazyLock::new(|| {
    let symbol = app_services::sky_light_symbol(c"SLSCopyWindowsWithOptionsAndTags")?;
    // SAFETY: the resolved SPI has the CFArrayRef(int, uint32_t, CFArrayRef,
    // uint32_t, uint64_t*, uint64_t*) C ABI and Copy ownership.
    Some(unsafe { std::mem::transmute::<*mut std::ffi::c_void, CopyWindows>(symbol) })
});

/// One window of the catalog.
#[derive(Clone, Copy, Debug)]
struct Window {
    id: u32,
    pid: c_int,
    frame: Frame,
}

/// An app as the switcher names it.
#[derive(Clone, Debug)]
struct AppInfo {
    id: String,
    name: String,
    bundle: String,
}

/// Read the desktops of `display` from `state`, within `budget`.
pub(super) fn gather(display: u32, state: &SpaceState, budget: Duration) -> Option<DesktopSchema> {
    let deadline = Instant::now() + budget;
    let started = Instant::now();
    let copy_windows = (*COPY_WINDOWS)?;
    let api = API.as_ref()?;
    // SAFETY: resolved read-only SPI with no arguments.
    let connection = unsafe { (api.connection)() };
    if connection == 0 {
        return None;
    }
    let bounds = CGDisplay::new(display).bounds();
    let display_frame = Frame {
        x: bounds.origin.x,
        y: bounds.origin.y,
        width: bounds.size.width,
        height: bounds.size.height,
    };
    let catalog = catalog()?;

    let mut apps: Vec<(SchemaApp, String)> = Vec::new();
    let mut app_index: HashMap<String, u16> = HashMap::new();
    let mut by_pid: HashMap<c_int, Option<u16>> = HashMap::new();
    let mut spaces = Vec::new();
    for &space in state.ordered.iter().take(MAX_SPACES) {
        if Instant::now() >= deadline {
            tracing::debug!("desktop schema: out of time reading windows");
            return None;
        }
        let members = space_windows(copy_windows, connection, space)?;
        let mut windows = Vec::new();
        // The catalog is front-to-back, which the map keeps.
        for window in catalog.iter().filter(|w| members.contains(&w.id)) {
            if windows.len() >= MAX_WINDOWS {
                break;
            }
            let Some(frame) = UnitRect::within(display_frame, window.frame) else {
                continue;
            };
            let app = *by_pid.entry(window.pid).or_insert_with(|| {
                let info = app_for_pid(window.pid)?;
                if let Some(&known) = app_index.get(&info.id) {
                    return Some(known);
                }
                let index = u16::try_from(apps.len())
                    .ok()
                    .filter(|_| apps.len() < MAX_APPS)?;
                app_index.insert(info.id.clone(), index);
                apps.push((
                    SchemaApp {
                        id: info.id,
                        name: info.name,
                        icon: None,
                    },
                    info.bundle,
                ));
                Some(index)
            });
            // Apps keep invisible twins of a window (shadows, tab strips) at
            // the very same frame; one outline is enough.
            if let Some(app) = app.filter(|&app| {
                !windows
                    .iter()
                    .any(|w: &SchemaWindow| w.app == app && w.frame == frame)
            }) {
                windows.push(SchemaWindow { app, frame });
            }
        }
        spaces.push(SpaceSchema {
            fullscreen: state.fullscreen.contains(&space),
            windows,
        });
    }

    let mut schema = DesktopSchema {
        apps: apps.iter().map(|(app, _)| app.clone()).collect(),
        spaces,
    };
    attach_icons(&mut schema, &apps, deadline);
    tracing::debug!(
        elapsed = ?started.elapsed(),
        spaces = schema.spaces.len(),
        apps = schema.apps.len(),
        "desktop schema read"
    );
    Some(schema)
}

/// Every normal window of every Space, front-most first, from the window list
/// (no titles: the title key is never read).
fn catalog() -> Option<Vec<Window>> {
    let list = CGWindowListCopyWindowInfo(CGWindowListOption::OptionAll, 0)?;
    // SAFETY: the window list is an array of CF dictionaries, each downcast
    // before use.
    let list = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(list) };
    let own = std::process::id();
    let mut windows = Vec::new();
    for entry in list {
        let Some(info) = dictionary(entry) else {
            continue;
        };
        let (Some(id), Some(pid), Some(layer)) = (
            number(&info, "kCGWindowNumber"),
            number(&info, "kCGWindowOwnerPID"),
            number(&info, "kCGWindowLayer"),
        ) else {
            continue;
        };
        // Layer 0 is the ordinary window layer: menus, the Dock, overlays and
        // status items live above it.
        if layer != 0 || pid == u64::from(own) {
            continue;
        }
        if float(&info, "kCGWindowAlpha").is_some_and(|alpha| alpha < 0.05) {
            continue;
        }
        let Some(bounds) = get(&info, "kCGWindowBounds").and_then(dictionary) else {
            continue;
        };
        let (Some(x), Some(y), Some(width), Some(height)) = (
            float(&bounds, "X"),
            float(&bounds, "Y"),
            float(&bounds, "Width"),
            float(&bounds, "Height"),
        ) else {
            continue;
        };
        let (Ok(id), Ok(pid)) = (u32::try_from(id), c_int::try_from(pid)) else {
            continue;
        };
        windows.push(Window {
            id,
            pid,
            frame: Frame {
                x,
                y,
                width,
                height,
            },
        });
    }
    Some(windows)
}

fn float(info: &Dictionary, key: &'static str) -> Option<f64> {
    get(info, key)?.downcast::<CFNumber>().ok()?.as_f64()
}

/// The ids of the windows on `space`.
fn space_windows(copy: CopyWindows, connection: c_int, space: u64) -> Option<HashSet<u32>> {
    let space = CFNumber::new_i64(i64::try_from(space).ok()?);
    let spaces = CFArray::from_retained_objects(&[space]);
    let (mut set_tags, mut clear_tags) = (0_u64, 0_u64);
    // SAFETY: the connection is this process's; `spaces` is a live CFArray of
    // CFNumbers and both tag pointers are valid for writes. The Copy-rule
    // result is owned or null.
    let ids = NonNull::new(unsafe {
        copy(
            connection,
            0,
            (&raw const *spaces).cast(),
            WINDOW_OPTIONS,
            &raw mut set_tags,
            &raw mut clear_tags,
        )
    })?;
    // SAFETY: adopt the Copy-rule result once.
    let ids = unsafe { CFRetained::from_raw(ids) };
    // SAFETY: the result is an array of CFNumber window ids, each downcast.
    let ids = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(ids) };
    Some(
        ids.into_iter()
            .filter_map(|id| {
                let id = id.downcast::<CFNumber>().ok()?.as_i64()?;
                u32::try_from(id).ok()
            })
            .collect(),
    )
}

/// The ordinary app behind `pid`. Background agents and accessory apps do not
/// count: their windows are not what a desktop is "of".
fn app_for_pid(pid: c_int) -> Option<AppInfo> {
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
    if app.activationPolicy() != NSApplicationActivationPolicy::Regular {
        return None;
    }
    let id = app.bundleIdentifier()?.to_string();
    let bundle = app.bundleURL()?.path()?.to_string();
    let name = app
        .localizedName()
        .map_or_else(|| id.clone(), |name| name.to_string());
    Some(AppInfo { id, name, bundle })
}

/// Give the apps the tiles will show an icon: from the cache when it is there,
/// rendered now while `deadline` allows, otherwise rendered in the background
/// for the next switch.
fn attach_icons(schema: &mut DesktopSchema, apps: &[(SchemaApp, String)], deadline: Instant) {
    let mut wanted: Vec<u16> = Vec::new();
    for space in 0..schema.spaces.len() {
        for app in schema.top_app_indices(space, MAX_ICONS) {
            if !wanted.contains(&app) {
                wanted.push(app);
            }
        }
    }
    let Ok(dir) = openlogi_core::paths::data_dir().map(|dir| dir.join("desktop-icons")) else {
        return;
    };
    let mut later = Vec::new();
    for index in wanted {
        let Some((app, bundle)) = apps.get(usize::from(index)) else {
            continue;
        };
        let file = dir.join(icon_file_name(&app.id, Path::new(bundle)));
        let ready = file.exists() || (Instant::now() < deadline && render_icon(bundle, &file));
        if !ready {
            later.push((bundle.clone(), file));
            continue;
        }
        if let Some(app) = schema.apps.get_mut(usize::from(index)) {
            app.icon = file.to_str().map(str::to_owned);
        }
    }
    if !later.is_empty() {
        render_in_background(later);
    }
}

/// `<bundle id>-<bundle mtime>.png`: a new version of an app has a new name,
/// so a stale icon is never reused.
fn icon_file_name(id: &str, bundle: &Path) -> String {
    let stamp = std::fs::metadata(bundle)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_secs());
    let id: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{id}-{stamp}.png")
}

/// Icons already being rendered by a background thread.
static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn render_in_background(jobs: Vec<(String, PathBuf)>) {
    let jobs: Vec<_> = {
        let mut pending = PENDING.lock().unwrap_or_else(PoisonError::into_inner);
        jobs.into_iter()
            .filter(|(_, file)| {
                let fresh = !pending.contains(file);
                if fresh {
                    pending.push(file.clone());
                }
                fresh
            })
            .collect()
    };
    if jobs.is_empty() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("openlogi-icons".into())
        .spawn(move || {
            for (bundle, file) in jobs {
                autoreleasepool(|_| render_icon(&bundle, &file));
                PENDING
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .retain(|pending| *pending != file);
            }
        });
    if let Err(error) = spawned {
        tracing::debug!(%error, "desktop icons: no background thread");
        PENDING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

/// Render the icon of the app at `bundle` into `file`, atomically.
fn render_icon(bundle: &str, file: &Path) -> bool {
    let Some(png) = icon_png(bundle) else {
        return false;
    };
    let written = (|| {
        std::fs::create_dir_all(file.parent()?).ok()?;
        let temporary = file.with_extension("png.tmp");
        std::fs::write(&temporary, png).ok()?;
        std::fs::rename(&temporary, file).ok()
    })();
    written.is_some()
}

/// The PNG of the icon `NSWorkspace` has for the app at `bundle`.
fn icon_png(bundle: &str) -> Option<Vec<u8>> {
    let image = NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(bundle));
    let mut rect = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(ICON_POINTS, ICON_POINTS),
    );
    // SAFETY: `rect` is a valid pointer for the call; no context or hints.
    let cg_image =
        unsafe { image.CGImageForProposedRect_context_hints(&raw mut rect, None, None) }?;
    let representation = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);
    let properties = NSDictionary::<NSBitmapImageRepPropertyKey, AnyObject>::new();
    // SAFETY: an empty properties dictionary of the declared types.
    let data = unsafe {
        representation.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
    }?;
    Some(data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_file_name_is_safe_and_versioned() {
        let name = icon_file_name("com.example/App Name", Path::new("/nonexistent"));
        assert_eq!(name, "com.example_App_Name-0.png");
        let here = icon_file_name("a.b", Path::new(env!("CARGO_MANIFEST_DIR")));
        assert!(here.starts_with("a.b-") && !here.ends_with("-0.png"));
    }

    /// Reads this machine's desktops for real: run by hand with `--ignored
    /// --nocapture` to see what the switcher would be sent.
    #[test]
    #[ignore = "reads the live desktop"]
    fn reads_the_live_desktops() {
        let cursor = super::super::cursor_display().expect("a display");
        let api = API.as_ref().expect("SkyLight");
        let uuid = {
            // SAFETY: as in `Native::new`.
            let uuid = NonNull::new(unsafe { (api.display_uuid)(cursor) }).expect("uuid");
            // SAFETY: adopt the Create-rule result once.
            let uuid = unsafe { CFRetained::from_raw(uuid) };
            objc2_core_foundation::CFUUID::new_string(None, Some(&uuid))
                .expect("string")
                .to_string()
        };
        let state = api.state(&uuid).expect("space state");
        let started = Instant::now();
        let schema = gather(cursor, &state, Duration::from_secs(2)).expect("a schema");
        eprintln!("{:?} in {:?}", schema, started.elapsed());
    }
}
