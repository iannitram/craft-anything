use craft_anything_formats::sqlite::{CellValue, SqliteDb};
use craft_anything_testkit::sqlite::{Val, table_db, without_rowid_db};

/// Verify a rowid-alias table reads the B-tree rowid into its INTEGER primary key column.
fn assert_rowid_alias(sql: &str) {
    let bytes = table_db(sql, &[(42, vec![Val::Null, Val::Text("ok".into())])]);
    let rows = SqliteDb::parse(&bytes).unwrap().read_rows(2, 0, 10).rows;
    assert_eq!(rows.len(), 1, "{sql}");
    assert_eq!(rows[0].values, [CellValue::Int(42), CellValue::Text { text: "ok".into(), truncated: false }], "{sql}");
}

/// Verify a WITHOUT ROWID table maps key-first physical records back to declaration order.
fn assert_without_rowid(sql: &str) {
    let bytes = without_rowid_db(sql, &[vec![Val::Text("key".into()), Val::Text("value".into())]]);
    let rows = SqliteDb::parse(&bytes).unwrap().read_rows(2, 0, 10).rows;
    assert_eq!(rows.len(), 1, "{sql}");
    assert_eq!(rows[0].values, [CellValue::Text { text: "value".into(), truncated: false }, CellValue::Text { text: "key".into(), truncated: false }], "{sql}");
}

/// A table constraint preceded by UNIQUE without a comma still identifies a rowid alias.
#[test]
fn rowid_alias_after_unique_without_comma() {
    assert_rowid_alias("CREATE TABLE t (id INTEGER, v TEXT, UNIQUE(v) PRIMARY KEY(id))");
}

/// A PRIMARY KEY followed by UNIQUE without a comma still identifies a rowid alias.
#[test]
fn rowid_alias_before_unique_without_comma() {
    assert_rowid_alias("CREATE TABLE t (id INTEGER, v TEXT, PRIMARY KEY(id) UNIQUE(v))");
}

/// A named CONSTRAINT PRIMARY KEY after CHECK without a comma still identifies a rowid alias.
#[test]
fn rowid_alias_named_constraint_after_check_without_comma() {
    assert_rowid_alias("CREATE TABLE t (id INTEGER, v TEXT, CHECK(v != '') CONSTRAINT pk PRIMARY KEY(id))");
}

/// Parenthesized column names in a table-level PRIMARY KEY still identify a rowid alias.
#[test]
fn rowid_alias_with_parenthesized_column_name() {
    assert_rowid_alias("CREATE TABLE t (id INTEGER, v TEXT, PRIMARY KEY((id)))");
}

/// A WITHOUT ROWID PRIMARY KEY preceded by UNIQUE without a comma projects columns in order.
#[test]
fn without_rowid_after_unique_without_comma() {
    assert_without_rowid("CREATE TABLE t (v TEXT, k TEXT, UNIQUE(v) PRIMARY KEY(k)) WITHOUT ROWID");
}

/// A WITHOUT ROWID PRIMARY KEY followed by UNIQUE without a comma avoids spanning both clauses.
#[test]
fn without_rowid_before_unique_without_comma() {
    assert_without_rowid("CREATE TABLE t (v TEXT, k TEXT, PRIMARY KEY(k) UNIQUE(v)) WITHOUT ROWID");
}

/// A WITHOUT ROWID named CONSTRAINT PRIMARY KEY after CHECK without a comma projects columns in order.
#[test]
fn without_rowid_named_constraint_after_check_without_comma() {
    assert_without_rowid("CREATE TABLE t (v TEXT, k TEXT, CHECK(v != '') CONSTRAINT pk PRIMARY KEY(k)) WITHOUT ROWID");
}

/// Standard comma-separated PRIMARY KEY control works for both rowid and WITHOUT ROWID tables.
#[test]
fn comma_separated_primary_key_control() {
    assert_rowid_alias("CREATE TABLE t (id INTEGER, v TEXT, UNIQUE(v), PRIMARY KEY(id))");
    assert_without_rowid("CREATE TABLE t (v TEXT, k TEXT, UNIQUE(v), PRIMARY KEY(k)) WITHOUT ROWID");
}

/// Table-level PRIMARY KEY(id AUTOINCREMENT) preserves the INTEGER rowid alias.
#[test]
fn rowid_alias_with_table_autoincrement() {
    assert_rowid_alias("CREATE TABLE t (id INTEGER, v TEXT, PRIMARY KEY(id AUTOINCREMENT))");
}

/// Table-level AUTOINCREMENT keeps working when preceded by UNIQUE without a comma.
#[test]
fn rowid_alias_after_unique_with_table_autoincrement() {
    assert_rowid_alias("CREATE TABLE t (id INTEGER, v TEXT, UNIQUE(v) PRIMARY KEY(id AUTOINCREMENT))");
}

/// Table-level ASC, DESC, COLLATE, and parenthesized terms combined with AUTOINCREMENT stay rowid aliases.
#[test]
fn rowid_alias_with_indexed_column_modifiers_and_autoincrement() {
    assert_rowid_alias("CREATE TABLE t (id INTEGER, v TEXT, PRIMARY KEY((id) AUTOINCREMENT))");
    assert_rowid_alias("CREATE TABLE t (id INTEGER, v TEXT, PRIMARY KEY(id ASC AUTOINCREMENT))");
    assert_rowid_alias("CREATE TABLE t (id INTEGER, v TEXT, PRIMARY KEY(id DESC AUTOINCREMENT))");
    assert_rowid_alias("CREATE TABLE t (id INTEGER, v TEXT, PRIMARY KEY(id COLLATE NOCASE AUTOINCREMENT))");
    assert_rowid_alias("CREATE TABLE t (id INTEGER, v TEXT, PRIMARY KEY((id) COLLATE NOCASE DESC))");
    assert_rowid_alias("CREATE TABLE t (id INTEGER, v TEXT, UNIQUE(v) PRIMARY KEY((id) COLLATE NOCASE DESC AUTOINCREMENT))");
}
