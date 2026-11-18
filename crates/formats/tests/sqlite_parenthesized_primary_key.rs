use craft_anything_formats::sqlite::{CellValue, SqliteDb};
use craft_anything_testkit::sqlite::{Val, without_rowid_db};

/// Read a row with the key slots followed by its remaining field.
fn assert_layout(terms: &str, repeated_key: bool) {
    let sql = format!("CREATE TABLE t (v TEXT, a TEXT, b INT, PRIMARY KEY({terms})) WITHOUT ROWID");
    let mut record = vec![Val::Text("key".into())];
    if repeated_key {
        record.push(Val::Text("key".into()));
    }
    record.extend([Val::Int(42), Val::Text("value".into())]);
    let bytes = without_rowid_db(&sql, &[record]);
    let rows = SqliteDb::parse(&bytes).unwrap().read_rows(2, 0, 10).rows;
    assert_eq!(rows.len(), 1, "{terms}");
    assert_eq!(
        rows[0].values,
        [CellValue::Text { text: "value".into(), truncated: false }, CellValue::Text { text: "key".into(), truncated: false }, CellValue::Int(42)],
        "{terms}"
    );
}

/// The inner closing parenthesis does not end the key list.
#[test]
fn reads_columns_after_a_parenthesized_key_column() {
    assert_layout(r#"("a"), b"#, false);
}

/// A collation inside redundant parentheses affects the physical key width.
#[test]
fn keeps_distinct_collations_inside_and_outside_parentheses() {
    assert_layout("((a COLLATE NOCASE)) DESC, (a) COLLATE RTRIM, b", true);
}

/// The outer collation wins over nested collations.
#[test]
fn collapses_key_terms_with_the_same_outermost_collation() {
    assert_layout("((a COLLATE RTRIM)) COLLATE NOCASE, a COLLATE NOCASE DESC, b", false);
}

/// Plain key columns keep the ordinary declaration order projection.
#[test]
fn reads_unwrapped_key_columns() {
    assert_layout("a, b", false);
}
