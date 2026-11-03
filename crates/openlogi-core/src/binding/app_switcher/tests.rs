use super::*;
use crate::binding::Binding;

fn stepper(step: u16) -> SwitchStepper {
    SwitchStepper::new(StepPx::try_from(step).expect("in range"))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "compared against the stepper's Option"
)]
fn run(direction: SwitchDirection, count: u16) -> Option<SwitchRun> {
    Some(SwitchRun { direction, count })
}

#[test]
fn a_step_of_travel_moves_one_app_and_keeps_the_remainder() {
    let mut s = stepper(60);
    assert_eq!(s.motion(59), None);
    assert_eq!(s.motion(1), run(SwitchDirection::Forward, 1));
    // 130 counts: two steps and 10 carried over.
    assert_eq!(s.motion(130), run(SwitchDirection::Forward, 2));
    assert_eq!(s.motion(49), None);
    assert_eq!(s.motion(1), run(SwitchDirection::Forward, 1));
}

#[test]
fn leftward_travel_steps_backward() {
    let mut s = stepper(60);
    assert_eq!(s.motion(-59), None);
    assert_eq!(s.motion(-1), run(SwitchDirection::Backward, 1));
    assert_eq!(s.motion(-120), run(SwitchDirection::Backward, 2));
}

#[test]
fn reversing_drops_the_partial_step_so_jitter_does_not_flutter() {
    let mut s = stepper(60);
    // Jitter around the boundary never completes a step in either direction.
    for _ in 0..50 {
        assert_eq!(s.motion(40), None);
        assert_eq!(s.motion(-40), None);
    }
    // Walking back after a step needs a whole step, not the remainder.
    assert_eq!(s.motion(70), run(SwitchDirection::Forward, 1));
    assert_eq!(s.motion(-50), None);
    assert_eq!(s.motion(-10), run(SwitchDirection::Backward, 1));
}

#[test]
fn zero_motion_is_ignored() {
    let mut s = stepper(60);
    assert_eq!(s.motion(30), None);
    assert_eq!(s.motion(0), None);
    assert_eq!(s.motion(30), run(SwitchDirection::Forward, 1));
}

#[test]
fn a_glitch_sized_report_is_capped() {
    let mut s = stepper(10);
    assert_eq!(s.motion(i16::MAX), run(SwitchDirection::Forward, 4));
    assert_eq!(s.motion(i16::MIN), run(SwitchDirection::Backward, 4));
    // The excess was dropped, not queued for the next report.
    assert_eq!(s.motion(5), None);
}

#[test]
fn step_and_hold_ranges_are_enforced() {
    StepPx::try_from(StepPx::MIN - 1).unwrap_err();
    StepPx::try_from(StepPx::MAX + 1).unwrap_err();
    StepPx::try_from(StepPx::MIN).unwrap();
    StepPx::try_from(StepPx::MAX).unwrap();
}

#[test]
fn defaults_come_from_one_place() {
    let params = AppSwitcherParams::default();
    assert_eq!(params.step(), StepPx::DEFAULT);
    assert_eq!(params.hold(), Duration::from_millis(250));
    let press = params.long_press();
    assert_eq!(press.threshold(), params.hold());
    assert_eq!(*press.long(), Action::AppSwitcher);
    assert_eq!(
        *press.short(),
        Action::CustomShortcut(switcher_chord(SwitchDirection::Forward).clone())
    );
}

#[test]
fn explicit_params_override_the_defaults() {
    let params = AppSwitcherParams::new(
        Some(StepPx::try_from(90).expect("in range")),
        Some(HoldThreshold::try_from(400).expect("in range")),
    );
    assert_eq!(params.step().get(), 90);
    assert_eq!(params.hold(), Duration::from_millis(400));
    assert_eq!(params.long_press().threshold(), Duration::from_millis(400));
}

#[test]
fn backward_chord_adds_shift_to_the_forward_chord() {
    let forward = switcher_chord(SwitchDirection::Forward);
    let backward = switcher_chord(SwitchDirection::Backward);
    assert!(!forward.has_shift());
    assert!(backward.has_shift());
    assert_eq!(forward.key(), backward.key());
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Wrapper {
    binding: Binding,
}

fn parse(text: &str) -> Result<Binding, toml::de::Error> {
    toml::from_str::<Wrapper>(text).map(|wrapper| wrapper.binding)
}

#[test]
fn plain_action_round_trips_as_a_bare_string() {
    let wrapper = Wrapper {
        binding: Binding::Single(Action::AppSwitcher),
    };
    let text = toml::to_string(&wrapper).expect("serialize");
    assert_eq!(text.trim(), "binding = \"AppSwitcher\"");
    assert_eq!(toml::from_str::<Wrapper>(&text).expect("parse"), wrapper);
    assert_eq!(
        Binding::Single(Action::AppSwitcher).app_switcher(),
        Some(AppSwitcherParams::default())
    );
}

#[test]
fn params_table_round_trips() {
    let binding = parse("binding = { AppSwitcher = { step = 80, hold_ms = 300 } }").expect("parse");
    let params = binding.app_switcher().expect("an app switcher");
    assert_eq!(params.step().get(), 80);
    assert_eq!(params.hold(), Duration::from_millis(300));
    assert_eq!(binding.click_action(), Action::AppSwitcher);

    let text = toml::to_string(&Wrapper { binding }).expect("serialize");
    let again = parse(&text).expect("reparse");
    assert_eq!(again.app_switcher(), Some(params));
}

#[test]
fn params_are_optional_and_partial() {
    let only_step = parse("binding = { AppSwitcher = { step = 90 } }").expect("parse");
    let params = only_step.app_switcher().expect("an app switcher");
    assert_eq!(params.step().get(), 90);
    assert_eq!(params.hold(), AppSwitcherParams::default().hold());

    let empty = parse("binding = { AppSwitcher = {} }").expect("parse");
    assert_eq!(empty.app_switcher(), Some(AppSwitcherParams::default()));
}

#[test]
fn bad_params_are_rejected() {
    parse("binding = { AppSwitcher = { step = 1 } }").unwrap_err();
    parse("binding = { AppSwitcher = { hold_ms = 50 } }").unwrap_err();
    parse("binding = { AppSwitcher = { speed = 5 } }").unwrap_err();
}

#[test]
fn other_bindings_are_not_app_switchers() {
    assert_eq!(Binding::Single(Action::Copy).app_switcher(), None);
}
