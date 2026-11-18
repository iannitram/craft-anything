use craft_anything_formats::sqlite::{CellValue, SqliteDb};
use craft_anything_testkit::sqlite::{Val, without_rowid_db};

/// Read the declaration-order row from a supplied WITHOUT ROWID record.
fn row(sql: &str, record: Vec<Val>) -> Vec<CellValue> {
    let data = without_rowid_db(sql, &[record]);
    SqliteDb::parse(&data).unwrap().read_rows(2, 0, 10).rows.remove(0).values
}

/// Create a text value without truncation.
fn text(value: &str) -> CellValue {
    CellValue::Text { text: value.into(), truncated: false }
}

/// A column appearing under different collations takes a slot for each occurrence.
#[test]
fn counts_each_distinct_primary_key_collation() {
    let sql = "CREATE TABLE t (v TEXT, a TEXT, b INT, PRIMARY KEY(a COLLATE NOCASE, a COLLATE RTRIM, b)) WITHOUT ROWID";
    let record = vec![Val::Text("key".into()), Val::Text("key".into()), Val::Int(42), Val::Text("value".into())];
    assert_eq!(row(sql, record), [text("value"), text("key"), CellValue::Int(42)]);
}

/// The declared collation counts for an unqualified key term. Case and sort direction do not make another slot.
#[test]
fn collapses_key_terms_using_the_same_effective_collation() {
    let sql = r#"CREATE TABLE t (v TEXT, a TEXT COLLATE NOCASE, b INT, PRIMARY KEY(a, a COLLATE "nocase" DESC, b)) WITHOUT ROWID"#;
    let record = vec![Val::Text("key".into()), Val::Int(42), Val::Text("value".into())];
    assert_eq!(row(sql, record), [text("value"), text("key"), CellValue::Int(42)]);
}

/// A collation in a column constraint must not become the column's default collation.
#[test]
fn distinguishes_a_constraint_collation_from_the_column_collation() {
    let sql = "CREATE TABLE t (v TEXT, a TEXT CHECK (a COLLATE NOCASE <> ''), b INT, PRIMARY KEY(a, a COLLATE NOCASE, b)) WITHOUT ROWID";
    let record = vec![Val::Text("key".into()), Val::Text("key".into()), Val::Int(42), Val::Text("value".into())];
    assert_eq!(row(sql, record), [text("value"), text("key"), CellValue::Int(42)]);
}
