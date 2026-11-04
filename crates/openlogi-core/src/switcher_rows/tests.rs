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
    let rows = usage.rows(&[], &[], Some("a"));
    assert_eq!(ids(&rows.recent), ["c", "b"]);
}

#[test]
fn most_used_ranks_by_count_then_recency() {
    let mut usage = AppUsage::default();
    for id in ["a", "b", "b", "c", "c", "a", "d"] {
        usage.record(&app(id));
    }
    let rows = usage.rows(&[], &[], None);
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
    let rows = usage.rows(&[], &[], None);
    assert!(rows.most_used.len() <= MAX_ROW_LEN);
}

#[test]
fn pinned_keeps_order_dedupes_and_marks_open() {
    let mut usage = AppUsage::default();
    usage.record(&app("mail"));
    let pinned = ["mail".to_owned(), "notes".into(), "mail".into()];
    let rows = usage.rows(&pinned, &["notes".into()], None);
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
    assert!(cursor.motion(-30, 0));
    assert_eq!(cursor.column(), Some(0));
}

#[test]
fn vertical_skips_empty_rows_and_remembers_columns() {
    let mut cursor = RowsCursor::new([3, 0, 4, 2], 0, step());
    assert!(cursor.motion(0, 10));
    assert_eq!(cursor.row(), SwitcherRow::Recent);
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
        (SwitcherRow::Windows, Some(2))
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
