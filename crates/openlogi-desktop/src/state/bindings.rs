//! Mouse, gesture, and keyboard binding commits.

use std::collections::{BTreeMap, BTreeSet};

use openlogi_core::binding::{
    Action, AppSwitcherBinding, AppSwitcherParams, Binding, ButtonId, GamingLayout, GestureDirection, HoldThreshold, KeypadPage,
    LONG_PRESS_THRESHOLD, LongPressBinding, PerAppBinding, StepPx, WheelHoldBinding, default_binding_on,
};
use openlogi_core::bindings::{
    bindings_for, button_bindings_for, hidpp_gesture_maps_for, oshook_gestures_for,
};
use openlogi_core::config::{Config, KeyTrigger};
use std::time::Duration;

use tracing::debug;

use crate::features::mouse::thumbwheel::{ThumbwheelPair, ThumbwheelPreset};
use crate::state::devices::DeviceRecord;

use super::events::StateEvents;
use super::{AppState, DeviceKey, StateEvent};

/// The open profile's overrides as the panel reads them: which buttons are
/// overridden and the plain-click action each carries. A per-app long press
/// reads as its short action; its hold is edited in the config file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppOverrides<'a> {
    /// The G-Shift layer.
    Actions(&'a BTreeMap<ButtonId, Action>),
    /// A per-application profile.
    PerApp(&'a BTreeMap<ButtonId, PerAppBinding>),
}

impl AppOverrides<'_> {
    /// Whether `button` is overridden.
    pub(crate) fn contains_key(self, button: ButtonId) -> bool {
        match self {
            Self::Actions(map) => map.contains_key(&button),
            Self::PerApp(map) => map.contains_key(&button),
        }
    }

    /// How many buttons are overridden.
    pub(crate) fn len(self) -> usize {
        match self {
            Self::Actions(map) => map.len(),
            Self::PerApp(map) => map.len(),
        }
    }

    /// The plain-click action `button` is overridden to.
    pub(crate) fn get(self, button: ButtonId) -> Option<Action> {
        match self {
            Self::Actions(map) => map.get(&button).cloned(),
            Self::PerApp(map) => map.get(&button).map(PerAppBinding::click_action),
        }
    }
}

/// The per-app profile or G-Shift layer being edited, tied to the device it
/// was opened for so it never carries over to another mouse.
struct EditingScope {
    persistent_key: String,
    layer: Layer,
}

#[derive(PartialEq, Eq)]
enum Layer {
    App(String),
    GShift,
}

/// Binding-editor state projected from the persisted configuration.
pub(super) struct BindingState {
    editing_scope: Option<EditingScope>,
    /// The hotspot the user most recently armed by clicking.
    active_button: Option<ButtonId>,
    /// Effective bindings for the selected device and open profile.
    button_bindings: BTreeMap<ButtonId, Action>,
    /// Device-global per-direction gesture bindings.
    gesture_bindings: BTreeMap<ButtonId, BTreeMap<GestureDirection, Action>>,
    /// Buttons whose hold turns the main wheel into a virtual thumb wheel.
    /// Device-global like the gesture maps: a per-app entry holds one
    /// `Action`, so wheel-hold lives only in the global profile.
    wheel_hold_buttons: BTreeSet<ButtonId>,
    /// Buttons with a tap and a hold action in the open profile: the global
    /// ones, overlaid by the app profile's own overrides.
    long_presses: BTreeMap<ButtonId, LongPressBinding>,
    /// The tunables of the buttons bound to the app switcher in the global
    /// profile. A per-app entry holds one action, so it never carries any.
    app_switchers: BTreeMap<ButtonId, AppSwitcherParams>,
    /// Global keyboard F-key bindings (Esc + F1-F19).
    keyboard_bindings: BTreeMap<KeyTrigger, Action>,
}

impl BindingState {
    pub(super) fn new(
        config: &Config,
        persistent_key: Option<&str>,
        layout: Option<&GamingLayout>,
    ) -> Self {
        let mut state = Self {
            editing_scope: None,
            active_button: None,
            button_bindings: BTreeMap::new(),
            gesture_bindings: BTreeMap::new(),
            wheel_hold_buttons: BTreeSet::new(),
            long_presses: BTreeMap::new(),
            app_switchers: BTreeMap::new(),
            keyboard_bindings: config.keyboard.bindings.clone(),
        };
        state.refresh_device(config, persistent_key, layout);
        state
    }

    fn layer(&self, persistent_key: Option<&str>) -> Option<&Layer> {
        let key = persistent_key?;
        self.editing_scope
            .as_ref()
            .filter(|scope| scope.persistent_key == key)
            .map(|scope| &scope.layer)
    }

    fn editing_app<'a>(&'a self, persistent_key: Option<&str>) -> Option<&'a str> {
        match self.layer(persistent_key)? {
            Layer::App(app) => Some(app),
            Layer::GShift => None,
        }
    }

    fn editing_gshift(&self, persistent_key: Option<&str>) -> bool {
        self.layer(persistent_key) == Some(&Layer::GShift)
    }

    fn set_layer(
        &mut self,
        config: &Config,
        persistent_key: Option<&str>,
        layout: Option<&GamingLayout>,
        layer: Option<Layer>,
    ) {
        self.editing_scope =
            layer
                .zip(persistent_key.map(str::to_string))
                .map(|(layer, persistent_key)| EditingScope {
                    persistent_key,
                    layer,
                });
        self.refresh_device(config, persistent_key, layout);
    }

    /// Rebuild the projections for the device `persistent_key`, whose
    /// G-series `layout` names the stock functions of its unset G buttons.
    fn refresh_device(
        &mut self,
        config: &Config,
        persistent_key: Option<&str>,
        layout: Option<&GamingLayout>,
    ) {
        let mut button_bindings = bindings_for(
            config,
            persistent_key,
            self.editing_app(persistent_key),
            layout,
        );
        if self.editing_gshift(persistent_key)
            && let Some(layer) = persistent_key.and_then(|key| config.gshift_overrides(key))
        {
            button_bindings.extend(layer.clone());
        }
        let gesture_bindings = gesture_maps_for(config, persistent_key);
        self.long_presses = if self.editing_gshift(persistent_key) {
            BTreeMap::new()
        } else {
            button_bindings_for(
                config,
                persistent_key,
                self.editing_app(persistent_key),
                layout,
            )
            .into_iter()
            .filter_map(|(button, binding)| match binding {
                Binding::LongPress(binding) => Some((button, binding)),
                _ => None,
            })
            .collect()
        };
        self.app_switchers = if self.editing_gshift(persistent_key)
            || self.editing_app(persistent_key).is_some()
        {
            BTreeMap::new()
        } else {
            button_bindings_for(config, persistent_key, None, layout)
                .into_iter()
                .filter_map(|(button, binding)| Some((button, binding.app_switcher()?)))
                .collect()
        };
        self.button_bindings = button_bindings;
        self.gesture_bindings = gesture_bindings;
        self.wheel_hold_buttons = persistent_key.map_or_else(BTreeSet::new, |key| {
            ButtonId::ALL
                .into_iter()
                .filter(|button| config.is_wheel_hold(key, *button))
                .collect()
        });
    }

    fn restore(
        &mut self,
        config: &Config,
        persistent_key: Option<&str>,
        layout: Option<&GamingLayout>,
    ) {
        self.refresh_device(config, persistent_key, layout);
        self.keyboard_bindings = config.keyboard.bindings.clone();
    }
}

fn gesture_maps_for(
    config: &Config,
    persistent_key: Option<&str>,
) -> BTreeMap<ButtonId, BTreeMap<GestureDirection, Action>> {
    let Some(key) = persistent_key else {
        return BTreeMap::new();
    };
    let mut maps = hidpp_gesture_maps_for(config, Some(key), None);
    maps.extend(oshook_gestures_for(config, Some(key), None));
    maps
}

/// `short` and `long` held for `threshold`. The one place a long press is
/// rebuilt from its parts, so every edit keeps the hold time the others set.
fn long_press(short: Action, long: Action, threshold: Duration) -> LongPressBinding {
    let binding = LongPressBinding::new(short, long);
    match u16::try_from(threshold.as_millis())
        .ok()
        .filter(|_| threshold != LONG_PRESS_THRESHOLD)
        .and_then(|ms| HoldThreshold::try_from(ms).ok())
    {
        Some(hold) => binding.with_hold(hold),
        None => binding,
    }
}

/// `binding` with its tap action replaced by `short`.
fn long_press_with_short(short: Action, binding: &LongPressBinding) -> LongPressBinding {
    long_press(short, binding.long().clone(), binding.threshold())
}

/// Record `binding` as `button`'s override in `app`'s profile.
fn set_per_app_long_press(
    config: &mut Config,
    device_key: &str,
    app: &str,
    button: ButtonId,
    binding: LongPressBinding,
) {
    config
        .devices
        .entry(device_key.to_string())
        .or_default()
        .per_app_bindings
        .entry(app.to_string())
        .or_default()
        .insert(button, PerAppBinding::LongPress(binding));
}

/// The binding of an app switcher with `params`: the plain action while they
/// are the defaults, the table form otherwise.
fn app_switcher_binding(params: AppSwitcherParams) -> Binding {
    if params.step() == StepPx::DEFAULT && params.hold() == AppSwitcherParams::default().hold() {
        Binding::Single(Action::AppSwitcher)
    } else {
        Binding::AppSwitcher(AppSwitcherBinding::new(params))
    }
}

/// One edit of a button's tap-and-hold pair.
enum HoldEdit {
    /// Add a hold action (starting as do nothing) beside the tap action.
    Enable,
    /// Go back to the tap action alone.
    Disable,
    /// Replace the hold action.
    Action(Action),
    /// Replace the hold time.
    Time(HoldThreshold),
}

/// What a [`HoldEdit`] leaves a button as.
enum HoldChange {
    /// The edit does not apply, so nothing is written.
    Unchanged,
    /// The tap action alone.
    Single,
    /// A tap and a hold action.
    Pair(LongPressBinding),
}

impl HoldEdit {
    /// What `self` makes of a button tapping `short` with `current` as its
    /// hold. Editing the hold of a button that has none changes nothing.
    fn apply(self, short: Action, current: Option<&LongPressBinding>) -> HoldChange {
        match (self, current) {
            (Self::Enable, None) => HoldChange::Pair(LongPressBinding::new(short, Action::None)),
            (Self::Disable, Some(_)) => HoldChange::Single,
            (Self::Action(long), Some(current)) => {
                HoldChange::Pair(long_press(short, long, current.threshold()))
            }
            (Self::Time(hold), Some(current)) => HoldChange::Pair(
                LongPressBinding::new(short, current.long().clone()).with_hold(hold),
            ),
            _ => HoldChange::Unchanged,
        }
    }
}

/// Write both halves of a thumb-wheel preset into `app`'s profile, or the
/// device's global bindings when `app` is `None`.
pub(super) fn apply_thumbwheel_pair(
    button_bindings: &mut BTreeMap<ButtonId, Action>,
    config: &mut openlogi_core::config::Config,
    persistent_key: Option<&str>,
    app: Option<&str>,
    pair: ThumbwheelPair,
) -> bool {
    button_bindings.insert(ButtonId::ThumbwheelScrollDown, pair.backward.clone());
    button_bindings.insert(ButtonId::ThumbwheelScrollUp, pair.forward.clone());

    let Some(key) = persistent_key else {
        return false;
    };
    for (button, action) in [
        (ButtonId::ThumbwheelScrollDown, pair.backward),
        (ButtonId::ThumbwheelScrollUp, pair.forward),
    ] {
        match app {
            Some(app) => config.set_per_app_binding(key, app, button, Some(action)),
            None => config.set_binding(key, button, Binding::Single(action)),
        }
    }
    true
}

/// Which bindings the mouse editor edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingLayer {
    /// The normal bindings, or an app profile's.
    Normal,
    /// The bindings used while G-Shift is held.
    GShift,
}

impl AppState {
    /// The application whose profile the binding panels are editing, or `None`
    /// for the device's global profile.
    #[must_use]
    pub fn editing_app(&self) -> Option<&str> {
        self.bindings.editing_app(
            self.current_record()
                .and_then(DeviceRecord::persistent_config_key),
        )
    }

    /// Edit `app`'s profile for the active device, or its global profile with
    /// `None`. Re-derives the editor projections without persisting this
    /// window-local choice.
    pub fn set_editing_app(&mut self, app: Option<String>) -> StateEvents {
        self.set_editing_layer(app.map(Layer::App))
    }

    /// Which bindings the mouse editor edits.
    #[must_use]
    pub fn binding_layer(&self) -> BindingLayer {
        if self.bindings.editing_gshift(
            self.current_record()
                .and_then(DeviceRecord::persistent_config_key),
        ) {
            BindingLayer::GShift
        } else {
            BindingLayer::Normal
        }
    }

    pub fn set_binding_layer(&mut self, layer: BindingLayer) -> StateEvents {
        self.set_editing_layer((layer == BindingLayer::GShift).then_some(Layer::GShift))
    }

    fn set_editing_layer(&mut self, layer: Option<Layer>) -> StateEvents {
        let key = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string);
        let layout = self.current_gaming_layout();
        self.bindings
            .set_layer(&self.config, key.as_deref(), layout, layer);
        self.for_current_device(StateEvent::BindingsChanged)
    }

    /// The hotspot most recently armed in the mouse editor.
    #[must_use]
    pub fn active_button(&self) -> Option<ButtonId> {
        self.bindings.active_button
    }

    /// Effective mouse bindings for the selected device and open profile.
    #[must_use]
    pub fn button_bindings(&self) -> &BTreeMap<ButtonId, Action> {
        &self.bindings.button_bindings
    }

    /// Device-global gesture direction maps for the selected device.
    #[must_use]
    pub fn gesture_bindings(&self) -> &BTreeMap<ButtonId, BTreeMap<GestureDirection, Action>> {
        &self.bindings.gesture_bindings
    }

    /// The selected device's buttons that hold the wheel to scroll sideways.
    /// Device-level like [`Self::gesture_bindings`], so it does not vary with
    /// the profile this window has open.
    #[must_use]
    pub fn wheel_hold_buttons(&self) -> &BTreeSet<ButtonId> {
        &self.bindings.wheel_hold_buttons
    }

    /// Global keyboard F-key bindings.
    #[must_use]
    pub fn keyboard_bindings(&self) -> &BTreeMap<KeyTrigger, Action> {
        &self.bindings.keyboard_bindings
    }

    /// The last of the selected device's keypad binding pages — its page
    /// count; one for any device that never had more.
    #[must_use]
    pub fn current_keypad_pages(&self) -> KeypadPage {
        self.current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map_or(KeypadPage::FIRST, |key| self.config.keypad_pages(key))
    }

    /// What each of the selected keypad's grid keys shows in the open
    /// profile ([`Binding::key_action`]) — the face the agent paints on its
    /// LCD. A key that does nothing is absent.
    ///
    /// Unlike [`Self::button_bindings`], a long press with only its long arm
    /// set shows that arm.
    #[must_use]
    pub fn keypad_key_actions(&self) -> BTreeMap<ButtonId, Action> {
        let key = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key);
        button_bindings_for(&self.config, key, self.editing_app(), None)
            .into_iter()
            .filter(|(button, _)| button.grid_slot().is_some())
            .filter_map(|(button, binding)| Some((button, binding.key_action()?)))
            .collect()
    }

    /// Whether the selected keypad binds any key on `page` in any profile —
    /// what removing that page would drop.
    #[must_use]
    pub fn current_keypad_page_binds(&self, page: KeypadPage) -> bool {
        self.current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .is_some_and(|key| self.config.keypad_page_binds(key, page))
    }

    /// Give the selected keypad pages `1..=last`, persist it, and have the
    /// agent reload. Shrinking drops the removed pages' grid bindings (see
    /// [`Config::set_keypad_pages`]); the binding projection is refreshed so
    /// the editor and the paging buttons' seeded actions follow at once.
    pub fn commit_keypad_pages(&mut self, last: KeypadPage) -> StateEvents {
        let events = self.for_current_device(StateEvent::BindingsChanged);
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            debug!("no persistent device key — keypad pages not stored");
            return StateEvents::default();
        };
        self.config
            .edit(|config| config.set_keypad_pages(&key, last));
        self.refresh_binding_projections();
        self.persist_and_reload("keypad pages");
        events
    }

    pub(super) fn refresh_binding_projections(&mut self) {
        let key = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string);
        let layout = self.current_gaming_layout();
        self.bindings
            .refresh_device(&self.config, key.as_deref(), layout);
    }

    pub(super) fn restore_binding_projections(&mut self) {
        let key = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string);
        let layout = self.current_gaming_layout();
        self.bindings.restore(&self.config, key.as_deref(), layout);
    }

    /// Update a single binding in memory, on disk, and in the shared hook
    /// map for the currently selected device — in whichever profile
    /// [`AppState::editing_app`] has open.
    ///
    /// In the global profile a long-press button keeps its long action and a
    /// wheel-hold button keeps holding the wheel: the picked action replaces
    /// only the short or click action.
    ///
    /// Disk failures restore the persisted projection and surface a config
    /// error instead of crashing the UI thread.
    pub fn commit_binding(&mut self, button: ButtonId, action: Action) -> StateEvents {
        let events = self.for_current_device(StateEvent::BindingsChanged);
        self.bindings.button_bindings.insert(button, action.clone());

        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            debug!(
                ?button,
                "no persistent device key — binding kept in memory only"
            );
            return events;
        };
        let app = self.editing_app().map(str::to_string);
        let gshift = self.binding_layer() == BindingLayer::GShift;
        let long_press = self.bindings.long_presses.get(&button).cloned();
        self.config.edit(|config| {
            // A per-app entry replaces the whole button — which is exactly what
            // picking an action means, and why gesture mode is not offered in
            // this scope. A button with a hold action keeps it, and its time.
            if let Some(app) = app {
                match long_press {
                    Some(held) => set_per_app_long_press(
                        config,
                        &key,
                        &app,
                        button,
                        long_press_with_short(action, &held),
                    ),
                    None => config.set_per_app_binding(&key, &app, button, Some(action)),
                }
            } else if gshift {
                config.set_gshift_binding(&key, button, Some(action));
            } else {
                let binding = match config.stored_bindings(&key).get(&button) {
                    Some(Binding::LongPress(binding)) => {
                        Binding::LongPress(long_press_with_short(action, binding))
                    }
                    Some(Binding::WheelHold(_)) => {
                        Binding::WheelHold(WheelHoldBinding::horizontal_scroll(action))
                    }
                    _ => Binding::Single(action),
                };
                config.set_binding(&key, button, binding);
            }
        });
        self.refresh_binding_projections();
        // The agent owns the hook; have it rebuild its live map from config.
        self.persist_and_reload("binding");
        events
    }

    /// Buttons with a tap and a hold action in the open profile.
    #[must_use]
    pub fn long_presses(&self) -> &BTreeMap<ButtonId, LongPressBinding> {
        &self.bindings.long_presses
    }

    /// The app switcher tunables of the buttons bound to it in the global
    /// profile; empty while an app profile or the G-Shift layer is open.
    #[must_use]
    pub fn app_switchers(&self) -> &BTreeMap<ButtonId, AppSwitcherParams> {
        &self.bindings.app_switchers
    }

    /// Set how far the mouse travels sideways per app on `button`'s app
    /// switcher, in raw counts.
    pub fn commit_app_switcher_step(&mut self, button: ButtonId, step: u16) -> StateEvents {
        match StepPx::try_from(step) {
            Ok(step) => self.commit_app_switcher(button, |params| {
                AppSwitcherParams::new(Some(step), Some(params.hold_threshold()))
            }),
            Err(error) => {
                debug!(%error, "app switcher step out of range — not stored");
                self.for_current_device(StateEvent::BindingsChanged)
            }
        }
    }

    /// Set how long `button`'s app switcher must be held, in milliseconds.
    pub fn commit_app_switcher_hold(&mut self, button: ButtonId, ms: u16) -> StateEvents {
        match HoldThreshold::try_from(ms) {
            Ok(hold) => self.commit_app_switcher(button, |params| {
                AppSwitcherParams::new(Some(params.step()), Some(hold))
            }),
            Err(error) => {
                debug!(%error, "app switcher hold out of range — not stored");
                self.for_current_device(StateEvent::BindingsChanged)
            }
        }
    }

    fn commit_app_switcher(
        &mut self,
        button: ButtonId,
        edit: impl FnOnce(AppSwitcherParams) -> AppSwitcherParams,
    ) -> StateEvents {
        let events = self.for_current_device(StateEvent::BindingsChanged);
        let Some(current) = self.bindings.app_switchers.get(&button).copied() else {
            debug!(?button, "no app switcher on this button — nothing stored");
            return events;
        };
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            return events;
        };
        let params = edit(current);
        self.config.edit(|config| {
            config.set_binding(&key, button, app_switcher_binding(params));
        });
        self.refresh_binding_projections();
        self.persist_and_reload("app switcher");
        events
    }

    /// Whether the open profile can give `button` a hold action: not the
    /// G-Shift layer, and not a button that gestures or holds the wheel in the
    /// global profile.
    #[must_use]
    pub fn can_edit_hold(&self, button: ButtonId) -> bool {
        if self.binding_layer() == BindingLayer::GShift {
            return false;
        }
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
        else {
            return false;
        };
        self.editing_app().is_some()
            || (!self.config.is_gesture_mode(key, button)
                && !self.config.is_wheel_hold(key, button))
    }

    /// Turn the hold action of `button` on or off in the open profile.
    pub fn commit_tap_hold(&mut self, button: ButtonId, enabled: bool) -> StateEvents {
        let edit = if enabled {
            HoldEdit::Enable
        } else {
            HoldEdit::Disable
        };
        self.commit_hold_edit(button, edit)
    }

    /// Set what holding `button` does.
    pub fn commit_hold_action(&mut self, button: ButtonId, action: Action) -> StateEvents {
        self.commit_hold_edit(button, HoldEdit::Action(action))
    }

    /// Set how long `button` must be held, in milliseconds: a tap-and-hold
    /// button's hold, or an app switcher's.
    pub fn commit_hold_time(&mut self, button: ButtonId, ms: u16) -> StateEvents {
        if self.bindings.app_switchers.contains_key(&button) {
            return self.commit_app_switcher_hold(button, ms);
        }
        let events = self.for_current_device(StateEvent::BindingsChanged);
        match HoldThreshold::try_from(ms) {
            Ok(hold) => self.commit_hold_edit(button, HoldEdit::Time(hold)),
            Err(error) => {
                debug!(%error, "hold time out of range — not stored");
                events
            }
        }
    }

    fn commit_hold_edit(&mut self, button: ButtonId, edit: HoldEdit) -> StateEvents {
        let events = self.for_current_device(StateEvent::BindingsChanged);
        if !self.can_edit_hold(button) {
            debug!(?button, "tap and hold is not editable here");
            return events;
        }
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            return events;
        };
        let short = self
            .bindings
            .button_bindings
            .get(&button)
            .cloned()
            .unwrap_or_else(|| default_binding_on(self.current_gaming_layout(), button));
        let next = edit.apply(short.clone(), self.bindings.long_presses.get(&button));
        let app = self.editing_app().map(str::to_string);
        self.config.edit(|config| match (app, next) {
            (_, HoldChange::Unchanged) => {}
            (Some(app), HoldChange::Pair(binding)) => {
                set_per_app_long_press(config, &key, &app, button, binding);
            }
            (Some(app), HoldChange::Single) => {
                config.set_per_app_binding(&key, &app, button, Some(short));
            }
            (None, HoldChange::Pair(binding)) => {
                config.set_binding(&key, button, Binding::LongPress(binding));
            }
            (None, HoldChange::Single) => config.set_binding(&key, button, Binding::Single(short)),
        });
        self.refresh_binding_projections();
        self.persist_and_reload("tap and hold");
        events
    }

    /// Remove the whole assignment in the open scope. The default scope
    /// returns to the device's native/default behavior; an app scope inherits
    /// the device default again. Unlike `Action::None`, this does not suppress
    /// a keyboard control.
    pub fn clear_binding(&mut self, button: ButtonId) -> StateEvents {
        if self.editing_app().is_some() {
            return self.clear_app_binding(button);
        }
        let events = self.for_current_device(StateEvent::BindingsChanged);
        self.bindings.button_bindings.remove(&button);
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            return events;
        };
        self.config.edit(|config| {
            if let Some(device) = config.devices.get_mut(&key) {
                device.bindings.remove(&button);
            }
        });
        self.refresh_binding_projections();
        self.persist_and_reload("binding");
        events
    }

    /// Drop `button`'s override in the open per-app profile, so it inherits the
    /// device's global binding again. A no-op in the global profile, which has
    /// nothing to inherit from.
    pub fn clear_app_binding(&mut self, button: ButtonId) -> StateEvents {
        self.clear_app_bindings([button])
    }

    /// Drop both halves of a thumb-wheel override together.
    pub fn clear_app_thumbwheel(&mut self) -> StateEvents {
        self.clear_app_bindings([ButtonId::ThumbwheelScrollDown, ButtonId::ThumbwheelScrollUp])
    }

    fn clear_app_bindings(&mut self, buttons: impl IntoIterator<Item = ButtonId>) -> StateEvents {
        let events = self.for_current_device(StateEvent::BindingsChanged);
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            return events;
        };
        let app = self.editing_app().map(str::to_string);
        if app.is_none() && self.binding_layer() == BindingLayer::Normal {
            return events;
        }
        self.config.edit(|config| {
            for button in buttons {
                match &app {
                    Some(app) => config.set_per_app_binding(&key, app, button, None),
                    None => config.set_gshift_binding(&key, button, None),
                }
            }
        });
        self.refresh_binding_projections();
        self.persist_and_reload("per-app binding");
        events
    }

    /// Restore live default inheritance without leaving the application's editor.
    pub fn reset_app_profile(&mut self, key: &DeviceKey, app: &str) -> StateEvents {
        self.clear_app_profile(key.as_str(), app);
        StateEvent::BindingsChanged(key.clone()).into()
    }

    /// Delete both per-app configuration sections in one save/reload. A failed
    /// save restores both sections and leaves both editor selections intact.
    pub fn remove_all_app_profiles(&mut self, key: &DeviceKey, app: &str) -> StateEvents {
        let changed = self.config.edit(|config| {
            let Some(device) = config.devices.get_mut(key.as_str()) else {
                return false;
            };
            let buttons = device.per_app_bindings.remove(app).is_some();
            let ring = device.action_ring.per_app.remove(app).is_some();
            buttons || ring
        });
        if changed && !self.persist_and_reload("all application profiles") {
            return StateEvent::BindingsChanged(key.clone()).into();
        }

        // Both saved sections are gone, so the scoped removals only close their
        // matching editors; neither writes or reloads an already absent profile.
        let events = self.remove_app_profile(key, app);
        let _ = self.remove_action_ring_profile(key, app);
        events
    }

    /// Remove the named profile, not whichever profile is selected when a
    /// confirmation completes. Only leave its editor after a successful save.
    pub fn remove_app_profile(&mut self, key: &DeviceKey, app: &str) -> StateEvents {
        if self.clear_app_profile(key.as_str(), app)
            && self.bindings.editing_scope.as_ref().is_some_and(|scope| {
                scope.persistent_key == key.as_str() && scope.layer == Layer::App(app.into())
            })
        {
            self.bindings.editing_scope = None;
            self.refresh_binding_projections();
        }
        StateEvent::BindingsChanged(key.clone()).into()
    }

    fn clear_app_profile(&mut self, key: &str, app: &str) -> bool {
        if !self
            .config
            .app_profiles(key)
            .any(|candidate| candidate == app)
        {
            // An application selected but never edited is window-local only.
            return true;
        }
        self.config
            .edit(|config| config.remove_app_profile(key, app));
        let saved = self.persist_and_reload("per-app profile");
        self.refresh_binding_projections();
        saved
    }

    /// The open per-app profile's overrides, so the panel can tell an override
    /// apart from a binding inherited from the global profile. `None` in the
    /// global profile, where there is nothing to distinguish.
    #[must_use]
    pub(crate) fn editing_app_overrides(&self) -> Option<AppOverrides<'_>> {
        let key = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)?;
        if self.binding_layer() == BindingLayer::GShift {
            return self.config.gshift_overrides(key).map(AppOverrides::Actions);
        }
        self.editing_app()
            .and_then(|app| self.config.per_app_overrides(key, app))
            .map(AppOverrides::PerApp)
    }

    /// Apply one paired thumb-wheel preset atomically. Both directional
    /// bindings are updated before the single config persistence/reload.
    pub fn commit_thumbwheel_preset(&mut self, preset: ThumbwheelPreset) -> StateEvents {
        let events = self.for_current_device(StateEvent::BindingsChanged);
        let pair = preset.pair();
        let key = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string);
        let app = self.editing_app().map(str::to_string);
        let changed = self.config.edit(|config| {
            apply_thumbwheel_pair(
                &mut self.bindings.button_bindings,
                config,
                key.as_deref(),
                app.as_deref(),
                pair,
            )
        });
        if !changed {
            debug!("no persistent device key — thumb-wheel pair kept in memory only");
            return events;
        }
        self.persist_and_reload("thumb-wheel binding");
        events
    }
    /// Records (or, with `action = None`, clears) the F-key `trigger` binding
    /// in the global `[keyboard]` map. Mirrors [`Self::commit_binding`] minus
    /// the device key — keyboard bindings are device-agnostic, so the write
    /// happens with or without a selected device; only the event it reports is
    /// addressed to the selected device, as every binding change's is. The
    /// agent's `rebuild()` republishes its shared keyboard map on
    /// `reload_config`, so this lands live.
    pub fn commit_keyboard_binding(
        &mut self,
        trigger: KeyTrigger,
        action: Option<Action>,
    ) -> StateEvents {
        match action {
            Some(ref a) => {
                self.bindings
                    .keyboard_bindings
                    .insert(trigger.clone(), a.clone());
            }
            None => {
                self.bindings.keyboard_bindings.remove(&trigger);
            }
        }
        self.config
            .edit(|config| config.set_keyboard_binding(trigger, action));
        self.persist_and_reload("keyboard binding");
        self.for_current_device(StateEvent::BindingsChanged)
    }
    /// Per-direction maps for every gesture-mode button of the current device,
    /// keyed by button — what the runtime dispatches for it. HID++ sources come
    /// fully seeded (matching the gesture watcher's projection); OS-hook
    /// buttons show their raw stored map (matching the OS hook's dispatch).
    /// Empty when no device is selected.
    ///
    /// Device-level: direction maps live only in the global profile, so this
    /// does not vary with the profile this window has open.
    #[must_use]
    pub(crate) fn device_gesture_maps(
        &self,
    ) -> BTreeMap<ButtonId, BTreeMap<GestureDirection, Action>> {
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
        else {
            return BTreeMap::new();
        };
        gesture_maps_for(&self.config, Some(key))
    }

    /// How many gesture directions the active device has bound, across every
    /// gesture-mode button. Device-level like [`Self::device_gesture_maps`].
    #[must_use]
    pub fn device_gesture_binding_count(&self) -> usize {
        self.device_gesture_maps().values().map(BTreeMap::len).sum()
    }

    /// The gesture menus the panel offers: [`Self::device_gesture_maps`], or
    /// nothing while a per-app profile is open.
    ///
    /// A per-app entry holds one `Action` and has no per-direction shape, so
    /// there is nothing to edit in that scope: every button falls through to
    /// the single-action picker, and overriding one is what stops it gesturing
    /// in that app. Offering the gesture menu instead would edit the global
    /// profile from a screen labelled with an application.
    #[must_use]
    #[cfg(test)]
    pub fn current_gesture_maps(&self) -> BTreeMap<ButtonId, BTreeMap<GestureDirection, Action>> {
        if self.editing_app().is_some() {
            return BTreeMap::new();
        }
        self.device_gesture_maps()
    }

    /// Turn gesture mode on or off for one button of the current device —
    /// independently of every other button. Persists, tells the agent to
    /// rebuild, and refreshes the projected maps the UI reads.
    pub fn commit_gesture_mode(&mut self, button: ButtonId, enabled: bool) -> StateEvents {
        let events = self.for_current_device(StateEvent::BindingsChanged);
        if enabled && !button.supports_gesture_mode() {
            debug!(?button, "gesture mode is not supported for this control");
            return events;
        }
        if enabled
            && button == ButtonId::DpiToggle
            && !self
                .current_record()
                .and_then(|record| record.capabilities)
                .is_some_and(|caps| caps.dpi_gestures)
        {
            debug!("DPI gestures require measured raw-XY support");
            return events;
        }
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            return events;
        };
        // Gesture mode is a property of the device's global bindings — a
        // per-app entry holds one `Action` and has no per-direction shape to
        // promote into. The picker hides the entry point in a per-app profile;
        // this is the backstop, because writing it here would silently change
        // every app instead of the one on screen.
        if self.editing_app().is_some() {
            debug!(?button, "gesture mode is not editable in a per-app profile");
            return events;
        }
        if self.config.is_gesture_mode(&key, button) == enabled {
            return events;
        }
        self.config
            .edit(|config| config.set_gesture_mode(&key, button, enabled));
        // The mode change shuffles bindings between the single + gesture maps.
        self.refresh_binding_projections();
        self.persist_and_reload("gesture-mode change");
        events
    }

    /// Turn wheel-hold on or off for one button of the current device:
    /// holding it then turns the main wheel into a horizontal scroll, and a
    /// press without scrolling still runs its action. Persists, tells the
    /// agent to rebuild, and refreshes the projections the UI reads.
    pub fn commit_wheel_hold(&mut self, button: ButtonId, enabled: bool) -> StateEvents {
        let events = self.for_current_device(StateEvent::BindingsChanged);
        if !button.supports_wheel_hold() {
            debug!(?button, "wheel-hold is not supported for this control");
            return events;
        }
        // Turning it off stays possible without the capability, so a binding
        // carried over from another device can always be removed.
        if enabled
            && !self
                .current_record()
                .and_then(|record| record.capabilities)
                .is_some_and(|caps| caps.virtual_thumbwheel)
        {
            debug!("wheel-hold requires measured raw-wheel support");
            return events;
        }
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            return events;
        };
        // Same backstop as `commit_gesture_mode`: a per-app entry holds one
        // `Action`, so writing here would change every app instead of the one
        // on screen.
        if self.editing_app().is_some() {
            debug!(?button, "wheel-hold is not editable in a per-app profile");
            return events;
        }
        if self.config.is_wheel_hold(&key, button) == enabled {
            return events;
        }
        self.config
            .edit(|config| config.set_wheel_hold(&key, button, enabled));
        // Leaving gesture mode moves the button between the projected maps.
        self.refresh_binding_projections();
        self.persist_and_reload("wheel-hold change");
        events
    }

    /// Update one direction of `button`'s gesture binding in memory, on disk,
    /// and (via reload) in the maps the agent dispatches from.
    pub fn commit_gesture_binding(
        &mut self,
        button: ButtonId,
        direction: GestureDirection,
        action: Action,
    ) -> StateEvents {
        let events = self.for_current_device(StateEvent::BindingsChanged);
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            debug!(
                ?button,
                ?direction,
                "no persistent device key — gesture binding edit ignored"
            );
            return events;
        };
        let is_gesture_mode = self.config.is_gesture_mode(&key, button);
        let is_stored_os_hook_gesture = button.is_os_hook_button() && is_gesture_mode;
        if !button.supports_gesture_mode() && !is_stored_os_hook_gesture {
            debug!(?button, "gestures are not supported for this control");
            return events;
        }
        // Same backstop as `commit_gesture_mode`: direction maps live only in
        // the global profile, so an edit arriving while a per-app one is open
        // would change every app instead of the one on screen.
        if self.editing_app().is_some() {
            debug!(
                ?button,
                ?direction,
                "gestures are not editable in a per-app profile"
            );
            return events;
        }
        // A stray edit on a button not in gesture mode must NOT silently
        // promote it (the gesture editor shouldn't be reachable in that
        // state): no-op instead.
        if !is_gesture_mode {
            debug!(
                ?button,
                ?direction,
                "button is not in gesture mode — ignoring gesture binding edit"
            );
            return events;
        }
        self.bindings
            .gesture_bindings
            .entry(button)
            .or_default()
            .insert(direction, action.clone());
        self.config
            .edit(|config| config.set_gesture_direction(&key, button, direction, action));
        // The agent owns the gesture watcher; have it rebuild from config.
        self.persist_and_reload("gesture binding");
        events
    }
}
