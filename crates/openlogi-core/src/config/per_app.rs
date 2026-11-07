//! Per-application profiles: the button overrides a device stores for one
//! application, and how the application in front is matched to them.
//!
//! An override replaces a whole button with a single action. Editing addresses
//! a profile by its exact key; matching goes through [`overlay_for`], which
//! also falls back from a Windows executable path to that executable's
//! `exe:<filename>` key, and then to the `app:<alias>` key of the
//! [`Config::apps`] alias that lists the application.

use std::collections::{BTreeMap, BTreeSet};

use super::Config;
use super::device::AppDeviceSettings;
use crate::app::{overlay_for, profile_key_for};
use crate::binding::{Action, Binding, ButtonId, PerAppBinding};

impl Config {
    /// The mouse-setting overrides `device_key` stores for the application in
    /// front, matched as [`Self::effective_bindings`] matches it. `None` when
    /// no application entry matches, so the device-wide values apply.
    #[must_use]
    pub fn app_device_settings(&self, device_key: &str, app: &str) -> Option<&AppDeviceSettings> {
        let device = self.devices.get(device_key)?;
        overlay_for(&device.per_app_settings, &self.apps, app)
    }

    /// The sensor DPI `app` sets while in front, when its entry stores one.
    #[must_use]
    pub fn app_dpi(&self, device_key: &str, app: &str) -> Option<crate::hid::Dpi> {
        self.app_device_settings(device_key, app)?.dpi
    }

    /// The main-wheel vertical scroll sensitivity while `app` is in front: its
    /// own [`Self::app_wheel_speed`] entry when it has one, else the global
    /// [`AppSettings::vertical_scroll_sensitivity`](super::AppSettings::vertical_scroll_sensitivity).
    #[must_use]
    pub fn app_vertical_scroll_sensitivity(
        &self,
        app: &str,
    ) -> super::settings::VerticalScrollSensitivity {
        overlay_for(&self.app_wheel_speed, &self.apps, app)
            .copied()
            .unwrap_or(self.app_settings.vertical_scroll_sensitivity)
    }

    /// Records (or, with `speed = None`, clears) the main-wheel speed `app`
    /// uses in place of the global one.
    pub fn set_app_wheel_speed(
        &mut self,
        app: &str,
        speed: Option<super::settings::VerticalScrollSensitivity>,
    ) {
        match speed {
            Some(speed) => {
                self.app_wheel_speed.insert(app.to_string(), speed);
            }
            None => {
                self.app_wheel_speed.remove(app);
            }
        }
    }

    /// Resolve the effective binding map for `device_key`, overlaying the
    /// per-app entry for `bundle_id` (if any) on top of the global per-device
    /// `bindings`. A per-app override replaces the whole button with a
    /// [`Binding::Single`] or a [`Binding::LongPress`] (with its own hold
    /// time); everything else falls through.
    ///
    /// Returns an empty map when the device has no recorded bindings yet.
    /// Callers (the GUI / hook) layer their own defaults on top.
    #[must_use]
    pub fn effective_bindings(
        &self,
        device_key: &str,
        bundle_id: Option<&str>,
    ) -> BTreeMap<ButtonId, Binding> {
        let Some(device) = self.devices.get(device_key) else {
            return BTreeMap::new();
        };
        let mut out = device.bindings.clone();
        if let Some(bid) = bundle_id
            && let Some(overlay) = overlay_for(&device.per_app_bindings, &self.apps, bid)
        {
            for (k, v) in overlay {
                out.insert(*k, v.to_binding());
            }
        }
        out
    }

    /// Records a per-app override. Creates the device + app entries as
    /// needed; passing an action of `None` removes the override and prunes
    /// the empty app map.
    pub fn set_per_app_binding(
        &mut self,
        device_key: &str,
        bundle_id: &str,
        button: ButtonId,
        action: Option<Action>,
    ) {
        let entry = self
            .devices
            .entry(device_key.to_string())
            .or_default()
            .per_app_bindings
            .entry(bundle_id.to_string())
            .or_default();
        match action {
            Some(a) => {
                entry.insert(button, PerAppBinding::Single(a));
            }
            None => {
                entry.remove(&button);
            }
        }
        if let Some(d) = self.devices.get_mut(device_key) {
            d.per_app_bindings.retain(|_, m| !m.is_empty());
        }
    }

    /// The overrides `device_key` stores for the application key `app`,
    /// or `None` when it has no profile for it.
    ///
    /// Exact key, deliberately: this answers "what did the user author under
    /// this key", which is what an editor needs to show and to clear. The
    /// question [`Self::has_app_override`] answers — "will the app in front hit
    /// a profile" — is the matcher's, and goes through the same `exe:` and
    /// alias fallbacks the matcher does. The two look interchangeable and are
    /// not.
    #[must_use]
    pub fn per_app_overrides(
        &self,
        device_key: &str,
        app: &str,
    ) -> Option<&BTreeMap<ButtonId, PerAppBinding>> {
        self.devices
            .get(device_key)?
            .per_app_bindings
            .get(app)
            .filter(|overrides| !overrides.is_empty())
    }

    /// Every application key `device_key` has a profile for, in key order: the
    /// apps with button overrides on this device, and the apps with a
    /// main-wheel speed of their own (which apply to every device).
    pub fn app_profiles(&self, device_key: &str) -> impl Iterator<Item = &str> {
        let mut apps: BTreeSet<&str> = self
            .devices
            .get(device_key)
            .into_iter()
            .flat_map(|device| device.per_app_bindings.keys().map(String::as_str))
            .collect();
        apps.extend(self.app_wheel_speed.keys().map(String::as_str));
        apps.into_iter()
    }

    /// Drop `device_key`'s whole profile for `app`. Nothing happens when there
    /// is none.
    pub fn remove_app_profile(&mut self, device_key: &str, app: &str) {
        if let Some(device) = self.devices.get_mut(device_key) {
            device.per_app_bindings.remove(app);
        }
    }

    /// Whether `device_key` has a non-empty per-app binding overlay for the
    /// foreground app `app`. Drives the menu-bar popover's "override
    /// active" badge — when the current app has its own bindings for this
    /// device, the global bindings are (partly) overridden.
    #[must_use]
    pub fn has_app_override(&self, device_key: &str, app: &str) -> bool {
        self.devices.get(device_key).is_some_and(|d| {
            overlay_for(&d.per_app_bindings, &self.apps, app)
                .is_some_and(|overlay| !overlay.is_empty())
        })
    }

    /// The key of the profile `device_key` runs the foreground application
    /// `app` under, matched as [`Self::effective_bindings`] matches it — the
    /// exact key, an `exe:<filename>` key, or an `app:<alias>` key — or `None`
    /// when the global bindings apply.
    #[must_use]
    pub fn app_profile_for(&self, device_key: &str, app: &str) -> Option<&str> {
        profile_key_for(
            &self.devices.get(device_key)?.per_app_bindings,
            &self.apps,
            app,
        )
    }

    /// The first per-app profile, by device and then key, whose `app:` key
    /// names an alias [`Self::apps`] does not define, as `(device, key)`.
    /// Such a profile could never apply, so loading rejects it.
    #[cfg(feature = "fs")]
    pub(super) fn undefined_app_alias(&self) -> Option<(&str, &str)> {
        self.devices.iter().find_map(|(device_key, device)| {
            device
                .per_app_bindings
                .keys()
                .chain(device.action_ring.per_app.keys())
                .find(|key| self.apps.is_undefined_reference(key))
                .map(|key| (device_key.as_str(), key.as_str()))
        })
    }
}
