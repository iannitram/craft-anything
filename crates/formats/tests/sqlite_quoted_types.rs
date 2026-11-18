use craft_anything_formats::sqlite::{CellValue, SqliteDb, parse_create_columns};
use craft_anything_testkit::sqlite::{Val, table_db};

/// SQLite returns the unquoted type and keeps spaces and doubled delimiters inside it.
#[test]
fn reads_sqlite_quoted_type_names() {
    let sql = r#"CREATE TABLE t(a "DOUBLE  PRECISION", b "a""b", c "FLOAT(" NOT NULL, d "decimal"(10, 2), e DECIMAL(10, 2))"#;
    let columns = parse_create_columns(sql);
    let types: Vec<_> = columns.iter().map(|column| column.type_name.as_str()).collect();
    assert_eq!(types, ["DOUBLE  PRECISION", "a\"b", "FLOAT(", "decimal", "DECIMAL(10, 2)"]);
}

/// Each SQLite quoting form can declare an INTEGER PRIMARY KEY alias.
#[test]
fn reads_rowids_through_quoted_integer_primary_keys() {
    for declaration in [r#""INTEGER""#, "[INTEGER]", "`INTEGER`", "'INTEGER'"] {
        let sql = format!("CREATE TABLE t(id {declaration} PRIMARY KEY)");
        let bytes = table_db(&sql, &[(42, vec![Val::Null])]);
        let db = SqliteDb::parse(&bytes).unwrap();
        assert_eq!(db.read_rows(2, 0, 1).rows[0].values, [CellValue::Int(42)], "{declaration}");
    }
}

/// A quoted type that spells INT still stores an ordinary primary key.
#[test]
fn leaves_quoted_int_as_a_stored_primary_key() {
    let bytes = table_db(r#"CREATE TABLE t(id "INT" PRIMARY KEY)"#, &[(42, vec![Val::Int(7)])]);
    let db = SqliteDb::parse(&bytes).unwrap();
    assert_eq!(db.read_rows(2, 0, 1).rows[0].values, [CellValue::Int(7)]);
}

/// A suffix keeps the primary key stored even when SQLite displays the type as INTEGER.
#[test]
fn preserves_null_keys_when_the_quoted_integer_type_has_a_suffix() {
    for declaration in [r#"id "INTEGER"(10) PRIMARY KEY"#, r#"id "INTEGER" EXTRA PRIMARY KEY"#, r#"id "INTEGER"(10), PRIMARY KEY (id)"#] {
        let sql = format!("CREATE TABLE t({declaration})");
        let bytes = table_db(&sql, &[(42, vec![Val::Null])]);
        let db = SqliteDb::parse(&bytes).unwrap();
        assert_eq!(db.find_object("t").unwrap().columns[0].type_name, "INTEGER", "{declaration}");
        assert_eq!(db.read_rows(2, 0, 1).rows[0].values, [CellValue::Null], "{declaration}");
    }
}

/// A comment and column constraint can follow a quoted INTEGER used by a table-level key.
#[test]
fn keeps_quoted_integer_aliases_with_constraints_before_a_table_key() {
    let sql = r#"CREATE TABLE t(id /* before type */ "INTEGER" /* after type */ COLLATE BINARY, PRIMARY KEY (id))"#;
    let bytes = table_db(sql, &[(42, vec![Val::Null])]);
    let db = SqliteDb::parse(&bytes).unwrap();
    assert_eq!(db.find_object("t").unwrap().columns[0].type_name, "INTEGER");
    assert_eq!(db.read_rows(2, 0, 1).rows[0].values, [CellValue::Int(42)]);
}

/// SQLite treats GENERATED as part of the type when it appears as a lone type word.
#[test]
fn keeps_a_stored_null_with_generated_as_a_type_word() {
    let sql = r#"CREATE TABLE t(id "INTEGER" GENERATED PRIMARY KEY)"#;
    let bytes = table_db(sql, &[(42, vec![Val::Null])]);
    let db = SqliteDb::parse(&bytes).unwrap();
    assert_eq!(db.find_object("t").unwrap().columns[0].type_name, "INTEGER");
    assert_eq!(db.read_rows(2, 0, 1).rows[0].values, [CellValue::Null]);
}

/// Deferral clauses leave a quoted INTEGER primary key eligible to alias the rowid.
#[test]
fn keeps_rowid_aliases_after_deferral_clauses() {
    for declaration in [r#"id "INTEGER" DEFERRABLE PRIMARY KEY"#, r#"id "INTEGER" NOT DEFERRABLE, PRIMARY KEY (id)"#] {
        let sql = format!("CREATE TABLE t({declaration})");
        let bytes = table_db(&sql, &[(42, vec![Val::Null])]);
        let db = SqliteDb::parse(&bytes).unwrap();
        assert_eq!(db.read_rows(2, 0, 1).rows[0].values, [CellValue::Int(42)], "{declaration}");
    }
}

/// SQLite trims a redundant ALWAYS suffix from a long enough declared type.
#[test]
fn keeps_the_alias_when_sqlite_trims_an_always_suffix() {
    for declaration in [r#"id "INTEGER" ALWAYS PRIMARY KEY"#, r#"id "INTEGER" GENERATED ALWAYS, PRIMARY KEY (id)"#] {
        let sql = format!("CREATE TABLE t({declaration})");
        let bytes = table_db(&sql, &[(42, vec![Val::Null])]);
        let db = SqliteDb::parse(&bytes).unwrap();
        assert_eq!(db.read_rows(2, 0, 1).rows[0].values, [CellValue::Int(42)], "{declaration}");
    }
}

/// A bracketed type with a suffix is dequoted across the full type span.
#[test]
fn reads_the_full_span_for_bracketed_types_with_suffixes() {
    let columns = parse_create_columns("CREATE TABLE t(id [INTEGER] EXTRA PRIMARY KEY, amount [NUMERIC](10, 2))");
    assert_eq!(columns[0].type_name, "INTEGER] EXTR");
    assert_eq!(columns[1].type_name, "NUMERIC](10, 2");
}

/// A quote inside the bracket-led type span leaves the first token's REAL affinity intact.
#[test]
fn keeps_real_affinity_when_a_bracketed_type_contains_another_quote() {
    for declaration in [r#"[REAL] "INT""#, "[REAL] 'INT'", "[REAL] `INT`", "[REAL] [INT]", r#"[REAL] /* " */ INT"#] {
        let sql = format!("CREATE TABLE t(amount {declaration})");
        let bytes = table_db(&sql, &[(1, vec![Val::Int(1)])]);
        let db = SqliteDb::parse(&bytes).unwrap();
        assert_eq!(db.find_object("t").unwrap().columns[0].type_name, "REAL", "{declaration}");
        assert_eq!(db.read_rows(2, 0, 1).rows[0].values, [CellValue::Real(1.0)], "{declaration}");
    }
}
