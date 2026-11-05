//! Bringing a running app, or one of its windows, to the front: what releasing
//! the app switcher on an entry does.
//!
//! An app is activated through `NSRunningApplication`. A window is raised
//! through its Accessibility element: the app's `AXWindows` are matched to the
//! window server's id with the private `_AXUIElementGetWindow`, resolved at run
//! time as `openlogi-hook` resolves it, and the match gets `AXRaise`. If that
//! symbol is missing the app still comes forward, just not that window.

use std::sync::LazyLock;

use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use objc2_application_services::AXUIElement;
use objc2_core_foundation::CFString;
use objc2_foundation::NSString;

use super::app_services;
use super::ax::elements;

/// `_AXUIElementGetWindow`: the AX element's window server id, `0` on success.
type WindowIdFn = unsafe extern "C" fn(*const AXUIElement, *mut u32) -> i32;

static WINDOW_ID: LazyLock<Option<WindowIdFn>> = LazyLock::new(|| {
    let symbol = app_services::symbol(c"_AXUIElementGetWindow")?;
    // SAFETY: the SPI has the AXError(AXUIElementRef, CGWindowID*) C ABI.
    Some(unsafe { std::mem::transmute::<*mut std::ffi::c_void, WindowIdFn>(symbol) })
});

/// Bring the running app with `bundle_id` to the front. `false` when none runs.
#[expect(
    deprecated,
    reason = "ActivateIgnoringOtherApps is necessary to bring background apps forward"
)]
pub(crate) fn activate_app(bundle_id: &str) -> bool {
    let id = NSString::from_str(bundle_id);
    let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&id);
    let Some(app) = apps.firstObject() else {
        return false;
    };
    app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps)
}

/// Bring window `window_id` of the process `pid` to the front, and its app with it.
#[expect(
    deprecated,
    reason = "ActivateIgnoringOtherApps is necessary to bring background apps forward"
)]
pub(crate) fn focus_window(pid: i32, window_id: u32) {
    let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) else {
        return;
    };
    app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
    let Some(window_number) = *WINDOW_ID else {
        return;
    };
    // SAFETY: `new_application` only creates an element for a process id; an
    // unknown pid yields an element whose attributes fail to read.
    let element = unsafe { AXUIElement::new_application(pid) };
    for window in elements(&element, "AXWindows") {
        let mut number = 0_u32;
        // SAFETY: `window` is a live element and `number` a writable out-pointer
        // for the call; the SPI writes it only on success.
        let error = unsafe { window_number(std::ptr::from_ref(&*window), &raw mut number) };
        if error == 0 && number == window_id {
            // SAFETY: the element is live for the call, and AXRaise takes no argument.
            unsafe { window.perform_action(&CFString::from_static_str("AXRaise")) };
            return;
        }
    }
}
