use super::*;

fn app(id: &str) -> ForegroundApp {
    ForegroundApp {
        id: id.into(),
        display_name: id.to_uppercase(),
    }
}

fn ids(row: &[RowApp]) -> Vec<&str> {
    row.iter().map(|a| a.app.id.as_str()).collect()
}

fn step() -> StepPx {
    StepPx::try_from(10).unwrap()
}

#[test]
fn recent_is_most_recent_first_without_duplicates_or_frontmost() {
    let mut usage = AppUsage::default();
    for id in ["a", "b", "c", "a"] {
        usage.record(&app(id));
    }
    let rows = usage.rows(&SwitcherRowRules::default(), &[], &[], Some("a"));
    assert_eq!(ids(&rows.recent), ["c", "b"]);
}

#[test]
fn most_used_ranks_by_count_then_recency() {
    let mut usage = AppUsage::default();
    for id in ["a", "b", "b", "c", "c", "a", "d"] {
        usage.record(&app(id));
    }
    let rows = usage.rows(&SwitcherRowRules::default(), &[], &[], None);
    assert_eq!(ids(&rows.most_used), ["a", "c", "b", "d"]);
    assert_eq!(usage.count("c"), 2);
}

#[test]
fn history_is_bounded() {
    let mut usage = AppUsage::default();
    for n in 0..(MAX_TRACKED + 40) {
        usage.record(&app(&format!("app{n}")));
    }
    usage.record(&app("app0"));
    assert!(usage.counts.len() <= MAX_TRACKED);
    assert!(usage.recent.len() <= MAX_RECENT);
    let rows = usage.rows(&SwitcherRowRules::default(), &[], &[], None);
    assert!(rows.most_used.len() <= MAX_ROW_LEN);
}

#[test]
fn pinned_keeps_order_dedupes_and_marks_open() {
    let mut usage = AppUsage::default();
    usage.record(&app("mail"));
    let pinned = ["mail".to_owned(), "notes".into(), "mail".into()];
    let rows = usage.rows(
        &SwitcherRowRules::default(),
        &pinned,
        &["notes".into()],
        None,
    );
    assert_eq!(ids(&rows.pinned), ["mail", "notes"]);
    assert_eq!(rows.pinned[0].app.display_name, "MAIL");
    assert_eq!(rows.pinned[0].activation(), Activation::Launch);
    assert_eq!(rows.pinned[1].activation(), Activation::Focus);
    assert_eq!(rows.pinned[1].app.display_name, "notes");
}

#[test]
fn usage_round_trips_through_toml() {
    let mut usage = AppUsage::default();
    usage.record(&app("a"));
    usage.record(&app("b"));
    let text = toml::to_string(&usage).unwrap();
    assert_eq!(toml::from_str::<AppUsage>(&text).unwrap(), usage);
    assert_eq!(toml::from_str::<AppUsage>("").unwrap(), AppUsage::default());
}

#[test]
fn sideways_walks_the_row_and_clamps() {
    let mut cursor = RowsCursor::new([3, 0, 0, 0], 1, step());
    assert_eq!(cursor.column(), Some(1));
    assert!(cursor.motion(10, 0));
    assert_eq!(cursor.column(), Some(2));
    assert!(!cursor.motion(50, 0));
    assert!(cursor.motion(-30, 0), "one report moves one entry");
    assert_eq!(cursor.column(), Some(1));
    assert!(cursor.motion(-30, 0));
    assert_eq!(cursor.column(), Some(0));
}

#[test]
fn vertical_skips_empty_rows_and_remembers_columns() {
    let mut cursor = RowsCursor::new([3, 0, 4, 2], 0, step());
    assert!(cursor.motion(0, 10));
    assert_eq!(cursor.row(), SwitcherRow::Recent);
    assert!(cursor.motion(20, 0));
    assert!(cursor.motion(20, 0));
    assert_eq!(cursor.column(), Some(2));
    assert!(cursor.motion(0, 10));
    assert_eq!(cursor.row(), SwitcherRow::MostUsed);
    assert!(!cursor.motion(0, 10));
    assert!(cursor.motion(0, -10));
    assert_eq!(cursor.row(), SwitcherRow::Recent);
    assert_eq!(cursor.column(), Some(2));
    assert!(cursor.motion(0, -10));
    assert_eq!(cursor.row(), SwitcherRow::Windows);
}

#[test]
fn dominant_axis_wins_a_diagonal_report() {
    let mut cursor = RowsCursor::new([3, 2, 0, 0], 0, step());
    assert!(cursor.motion(20, 10));
    assert_eq!(
        (cursor.row(), cursor.column()),
        (SwitcherRow::Windows, Some(1))
    );
}

#[test]
fn emptied_row_returns_to_windows() {
    let mut cursor = RowsCursor::new([1, 2, 0, 0], 0, step());
    cursor.motion(0, 10);
    assert_eq!(cursor.row(), SwitcherRow::Pinned);
    cursor.set_lens([1, 0, 0, 0]);
    assert_eq!(cursor.row(), SwitcherRow::Windows);
}

fn window(id: u32, title: &str) -> SwitcherWindow {
    SwitcherWindow {
        id,
        pid: 1,
        app: app("finder"),
        title: title.into(),
    }
}

#[test]
fn the_view_selects_the_entry_under_the_cursor() {
    let mut usage = AppUsage::default();
    usage.record(&app("a"));
    usage.record(&app("b"));
    let rows = usage.rows(&SwitcherRowRules::default(), &["p".to_owned()], &[], None);
    let mut view = SwitcherView::new(
        vec![window(1, "one"), window(2, "two")],
        rows,
        BTreeMap::new(),
    );
    let mut cursor = RowsCursor::new(view.lens(), 1, step());
    view.follow(&cursor);
    assert_eq!(
        view.selection(),
        Some(Selection::Window(&window(2, "two"))),
        "two windows of one app are two entries"
    );
    assert!(cursor.motion(0, 10));
    view.follow(&cursor);
    let Some(Selection::App(pinned)) = view.selection() else {
        panic!("the pinned row holds apps");
    };
    assert_eq!(pinned.app.id, "p");
    assert_eq!(pinned.activation(), Activation::Launch);
}

#[test]
fn the_windows_row_is_bounded() {
    let many = (0..40).map(|n| window(n, "w")).collect();
    let view = SwitcherView::new(many, SwitcherRows::default(), BTreeMap::new());
    assert_eq!(view.windows.len(), MAX_WINDOW_TILES);
    assert_eq!(view.selection(), None, "an unplaced cursor selects nothing");
}

#[test]
fn the_selection_moves_with_the_cursor_and_reports_motion() {
    let mut usage = AppUsage::default();
    usage.record(&app("a"));
    let rows = usage.rows(&SwitcherRowRules::default(), &[], &[], None);
    let view = SwitcherView::new(
        vec![window(1, "one"), window(2, "two")],
        rows,
        BTreeMap::new(),
    );
    let mut selection = SwitcherSelection::open(view, 1, step());
    assert_eq!(
        selection.selection(),
        Some(Selection::Window(&window(2, "two"))),
        "the window the native order picks first is selected on open"
    );
    assert!(!selection.motion(0, 0), "no travel, no change");
    assert!(selection.motion(0, 10), "moving down enters the recent row");
    assert_eq!(selection.view().row, Some(SwitcherRow::Recent));
    let Some(Selection::App(app)) = selection.selection() else {
        panic!("the recent row holds apps");
    };
    assert_eq!(app.app.id, "a");
    assert!(selection.motion(0, -10), "moving up returns to the windows");
    assert_eq!(
        selection.selection(),
        Some(Selection::Window(&window(2, "two"))),
        "the windows row remembers its column"
    );
}

fn rules_with(f: impl FnOnce(&mut SwitcherRowRules)) -> SwitcherRowRules {
    let mut rules = SwitcherRowRules::default();
    f(&mut rules);
    rules
}

#[test]
fn disabled_row_is_empty() {
    let mut usage = AppUsage::default();
    usage.record(&app("a"));
    usage.record(&app("b"));
    let rules = rules_with(|r| r.recent.enabled = false);
    let rows = usage.rows(&rules, &["p".to_owned()], &[], None);
    assert!(rows.recent.is_empty());
    assert_eq!(ids(&rows.most_used), vec!["b", "a"]);
}

#[test]
fn block_list_removes_app_from_its_row_only() {
    let mut usage = AppUsage::default();
    usage.record(&app("a"));
    usage.record(&app("b"));
    let rules = rules_with(|r| r.recent.block = vec!["a".to_owned()]);
    let rows = usage.rows(&rules, &[], &[], None);
    assert_eq!(ids(&rows.recent), vec!["b"]);
    assert_eq!(ids(&rows.most_used), vec!["b", "a"]);
}

#[test]
fn allow_list_limits_row_and_block_wins_over_allow() {
    let mut usage = AppUsage::default();
    usage.record(&app("a"));
    usage.record(&app("b"));
    usage.record(&app("c"));
    let rules = rules_with(|r| {
        r.most_used.allow = vec!["a".to_owned(), "b".to_owned()];
        r.most_used.block = vec!["b".to_owned()];
    });
    let rows = usage.rows(&rules, &[], &[], None);
    assert_eq!(ids(&rows.most_used), vec!["a"]);
}

#[test]
fn never_show_applies_to_every_row() {
    let mut usage = AppUsage::default();
    usage.record(&app("a"));
    let rules = rules_with(|r| r.never_show = vec!["a".to_owned()]);
    let rows = usage.rows(&rules, &["a".to_owned()], &[], None);
    assert!(rows.pinned.is_empty());
    assert!(rows.recent.is_empty());
    assert!(rows.most_used.is_empty());
}

#[test]
fn pinned_row_keeps_priority_order_under_filter() {
    let usage = AppUsage::default();
    let rules = rules_with(|r| r.pinned.block = vec!["x".to_owned()]);
    let pinned = ["p".to_owned(), "x".to_owned(), "q".to_owned()];
    let rows = usage.rows(&rules, &pinned, &[], None);
    assert_eq!(ids(&rows.pinned), vec!["p", "q"]);
}

#[test]
fn row_rules_default_is_default_and_round_trips_through_toml() {
    assert!(SwitcherRowRules::default().is_default());
    let rules = rules_with(|r| {
        r.recent.enabled = false;
        r.never_show = vec!["com.example.app".to_owned()];
    });
    assert!(!rules.is_default());
    let text = toml::to_string(&rules).expect("serializes");
    let back: SwitcherRowRules = toml::from_str(&text).expect("parses");
    assert_eq!(back, rules);
}
