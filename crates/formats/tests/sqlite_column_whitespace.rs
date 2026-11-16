use craft_anything_formats::sqlite::{CellValue, SqliteDb, parse_create_columns};
use craft_anything_testkit::sqlite::{Val, table_db};

/// All five SQLite whitespace characters terminate an unquoted column name.
#[test]
fn sql_whitespace_separates_column_names_from_types() {
    for space in [" ", "\t", "\n", "\r", "\r\n", "\u{c}"] {
        let sql = format!("CREATE TABLE t(id{space}INTEGER PRIMARY KEY, name{space}TEXT)");
        let columns = parse_create_columns(&sql);
        assert_eq!(columns.iter().map(|c| (c.name.as_str(), c.type_name.as_str())).collect::<Vec<_>>(), [("id", "INTEGER"), ("name", "TEXT")], "{space:?}");
    }
}

/// Correct type boundaries preserve rowid aliases in actual record decoding.
#[test]
fn column_line_endings_do_not_lose_the_rowid() {
    for space in ["\r", "\r\n", "\u{c}"] {
        let sql = format!("CREATE TABLE t(id{space}INTEGER PRIMARY KEY, name{space}TEXT)");
        let bytes = table_db(&sql, &[(7, vec![Val::Null, Val::Text("seven".into())])]);
        let page = SqliteDb::parse(&bytes).unwrap().read_rows(2, 0, 10);
        assert_eq!(page.rows[0].values, [CellValue::Int(7), CellValue::Text { text: "seven".into(), truncated: false }]);
    }
}

/// Quoted names retain whitespace that is part of the identifier.
#[test]
fn quoted_names_keep_their_whitespace() {
    let columns = parse_create_columns("CREATE TABLE t(\"a\rb\" TEXT, [c\u{c}d] INTEGER)");
    assert_eq!(columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["a\rb", "c\u{c}d"]);
}
