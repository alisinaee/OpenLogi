//! Typed integer entry for a setting that also has a slider.
//!
//! A slider picks a value by feel; a [`NumberEntry`] lets the user type the
//! exact one. It shows the value the slider shows, accepts a whole number,
//! clamps it into the setting's [`SliderRange`], and commits on Enter or when
//! focus leaves the field. Text that is not a number is dropped and the field
//! returns to the current value. The owner calls [`NumberEntry::sync`] every
//! render so the field follows the slider (and a rolled-back write) while it
//! is not being edited.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, AppContext as _, Context, Entity, Focusable as _, Subscription, Window};
use gpui_component::input::{InputEvent, InputState};

use super::commit_slider::{SliderRange, SliderUnit};

/// A text field bound to one slider-backed value.
pub(crate) struct NumberEntry<T> {
    input: Entity<InputState>,
    range: SliderRange<T>,
    /// The value the field last showed: what a revert returns to and what an
    /// unchanged commit is compared with.
    shown: Rc<Cell<T>>,
    _subscription: Subscription,
}

/// The text a field shows for `value`: a whole number.
fn text_of<T: SliderUnit>(value: T) -> String {
    format!("{:.0}", value.to_slider())
}

/// The value a typed `text` means, clamped into `range`, or `None` when it is
/// not a whole number (an optional sign, then digits).
fn parse<T: SliderUnit>(range: SliderRange<T>, text: &str) -> Option<T> {
    let text = text.trim();
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // A run of digits too long for `f32` parses to infinity, which clamps to
    // the maximum like any other number above the range.
    let raw: f32 = text.parse().ok()?;
    Some(range.value_at(raw))
}

impl<T: SliderUnit> NumberEntry<T> {
    /// A field over `range` showing `initial`. `on_commit` runs with the
    /// clamped value whenever a commit changes it.
    pub(crate) fn new<V: 'static>(
        range: SliderRange<T>,
        initial: T,
        window: &mut Window,
        cx: &mut Context<V>,
        on_commit: impl Fn(T, &mut App) + 'static,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(text_of(initial)));
        let shown = Rc::new(Cell::new(initial));
        let subscription = cx.subscribe_in(&input, window, {
            let shown = shown.clone();
            move |_, input, event: &InputEvent, window, cx| {
                if !matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    return;
                }
                let typed = input.read(cx).value();
                let value = parse(range, &typed).unwrap_or_else(|| shown.get());
                let text = text_of(value);
                if typed.as_ref() != text.as_str() {
                    input.update(cx, |input, cx| input.set_value(text, window, cx));
                }
                if value != shown.get() {
                    shown.set(value);
                    on_commit(value, cx);
                }
            }
        });
        Self {
            input,
            range,
            shown,
            _subscription: subscription,
        }
    }

    /// The state to render with `control_input`.
    pub(crate) fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    /// Show `value`, unless the field is being edited.
    pub(crate) fn sync(&self, value: T, window: &mut Window, cx: &mut App) {
        let value = self.range.clamp(value);
        self.shown.set(value);
        let text = text_of(value);
        let editing = self.input.read(cx).focus_handle(cx).is_focused(window);
        if !editing && self.input.read(cx).value().as_ref() != text.as_str() {
            self.input
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui::{Context, Empty, IntoElement, Render, TestAppContext, Window};
    use gpui_component::input::InputEvent;

    use super::{NumberEntry, SliderRange, parse};

    #[test]
    fn typed_text_is_a_whole_number_clamped_into_the_range() {
        let range = SliderRange::new(40_u16, 400);
        for (text, value) in [
            ("100", Some(100)),
            (" 250 ", Some(250)),
            ("+60", Some(60)),
            ("5", Some(40)),
            ("-5", Some(40)),
            ("9999", Some(400)),
            ("9999999999999999999999999999999999999999999", Some(400)),
            ("", None),
            ("-", None),
            ("1.5", None),
            ("1e3", None),
            ("abc", None),
        ] {
            assert_eq!(parse(range, text), value, "{text:?}");
        }
    }

    struct Owner {
        entry: NumberEntry<u16>,
        commits: std::rc::Rc<RefCell<Vec<u16>>>,
    }

    impl Owner {
        fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
            let commits = std::rc::Rc::new(RefCell::new(Vec::new()));
            let entry = NumberEntry::new(SliderRange::new(40, 400), 100, window, cx, {
                let commits = commits.clone();
                move |value, _| commits.borrow_mut().push(value)
            });
            Self { entry, commits }
        }
    }

    impl Render for Owner {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            Empty
        }
    }

    #[gpui::test]
    fn enter_and_blur_commit_a_clamped_value_once(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (owner, cx) = cx.add_window_view(Owner::new);
        let input = owner.read_with(cx, |owner, _| owner.entry.input().clone());
        let type_text = |text: &str, event: InputEvent, cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                input.update(cx, |input, cx| input.set_value(text.to_owned(), window, cx));
            });
            input.update(cx, |_, cx| cx.emit(event));
            cx.run_until_parked();
        };
        let enter = || InputEvent::PressEnter {
            secondary: false,
            shift: false,
        };

        type_text("900", enter(), cx);
        type_text("400", InputEvent::Blur, cx);
        type_text("junk", InputEvent::Blur, cx);
        type_text("40", enter(), cx);

        owner.read_with(cx, |owner, cx| {
            assert_eq!(*owner.commits.borrow(), [400, 40]);
            assert_eq!(*owner.entry.input().read(cx).value(), *"40");
        });

        // A value that follows the slider shows up while not editing.
        cx.update(|window, cx| {
            owner.update(cx, |owner, cx| owner.entry.sync(250, window, cx));
        });
        owner.read_with(cx, |owner, cx| {
            assert_eq!(*owner.entry.input().read(cx).value(), *"250");
        });
    }
}
