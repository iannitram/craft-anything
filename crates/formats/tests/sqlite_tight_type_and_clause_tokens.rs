use craft_anything_formats::sqlite::{CellValue, SqliteDb, parse_create_columns};
use craft_anything_testkit::sqlite::{Val, table_db, without_rowid_db};

fn text(value: &str) -> CellValue {
    CellValue::Text { text: value.into(), truncated: false }
}

fn names_and_types(sql: &str) -> Vec<(String, String)> {
    parse_create_columns(sql).into_iter().map(|c| (c.name, c.type_name)).collect()
}

/// Control: ordinary whitespace-separated column types, constraints, and non-ASCII Unicode
/// identifier characters (`\u{00a0}`, `\u{3000}`) keep their existing column metadata, affinities,
/// and rowid-alias behavior.
#[test]
fn whitespace_separated_declarations_control() {
    let sql = "CREATE TABLE t (id INTEGER PRIMARY KEY, r REAL(8) CHECK (CAST(r AS INT) > -1) DEFAULT 5, b BLOB(16) CHECK (typeof(b) <> 'char') DEFAULT 5)";
    assert_eq!(names_and_types(sql), vec![("id".into(), "INTEGER".into()), ("r".into(), "REAL(8)".into()), ("b".into(), "BLOB(16)".into())]);
    let bytes = table_db(sql, &[(7, vec![Val::Null])]);
    let page = SqliteDb::parse(&bytes).expect("valid sqlite fixture").read_rows(2, 0, 10);
    assert_eq!(page.rows[0].values, vec![CellValue::Int(7), CellValue::Real(5.0), CellValue::Int(5)]);
    assert!(page.notes.is_empty(), "{:?}", page.notes);

    let unicode_id_sql =
        "CREATE TABLE u (a\u{00a0}b INTEGER PRIMARY KEY, c\u{00a0}int REAL DEFAULT 5, col\u{00a0}check REAL DEFAULT 5, d\u{3000}char BLOB DEFAULT '5')";
    assert_eq!(
        names_and_types(unicode_id_sql),
        vec![
            ("a\u{00a0}b".into(), "INTEGER".into()),
            ("c\u{00a0}int".into(), "REAL".into()),
            ("col\u{00a0}check".into(), "REAL".into()),
            ("d\u{3000}char".into(), "BLOB".into()),
        ]
    );
    let unicode_bytes = table_db(unicode_id_sql, &[(42, vec![Val::Null])]);
    let unicode_page = SqliteDb::parse(&unicode_bytes).expect("valid sqlite fixture").read_rows(2, 0, 10);
    assert_eq!(unicode_page.rows[0].values, vec![CellValue::Int(42), CellValue::Real(5.0), CellValue::Real(5.0), text("5")]);
}

/// Column constraints adjacent to `)`, `(`, or quoted constraint names must not bleed into the
/// declared type or corrupt column affinities (`REAL` vs `INT`, `BLOB` vs `CHAR`, untyped vs `NUMERIC`).
#[test]
fn tight_constraints_do_not_bleed_into_declared_types_or_affinities() {
    let sql = concat!(
        "CREATE TABLE t (",
        "n INT, ",
        "r REAL(8)CHECK(CAST(r AS INT) > -1) DEFAULT 5, ",
        "b BLOB(16)CHECK(typeof(b) <> 'char') DEFAULT 5, ",
        "u CHECK(u IS NOT NULL) DEFAULT '5', ",
        "d REAL DEFAULT(5), ",
        "c INT CONSTRAINT\"c1\"DEFAULT 9, ",
        "t TEXT CONSTRAINT[c2]NOT NULL DEFAULT 'k', ",
        "p VARCHAR(10)NOT NULL DEFAULT 'v'",
        ")"
    );
    assert_eq!(
        names_and_types(sql),
        vec![
            ("n".into(), "INT".into()),
            ("r".into(), "REAL(8)".into()),
            ("b".into(), "BLOB(16)".into()),
            ("u".into(), "".into()),
            ("d".into(), "REAL".into()),
            ("c".into(), "INT".into()),
            ("t".into(), "TEXT".into()),
            ("p".into(), "VARCHAR(10)".into()),
        ]
    );

    let bytes = table_db(sql, &[(1, vec![Val::Int(1)])]);
    let page = SqliteDb::parse(&bytes).expect("valid sqlite fixture").read_rows(2, 0, 10);
    assert_eq!(
        page.rows[0].values,
        vec![CellValue::Int(1), CellValue::Real(5.0), CellValue::Int(5), text("5"), CellValue::Real(5.0), CellValue::Int(9), text("k"), text("v")]
    );
    assert!(page.notes.is_empty(), "{:?}", page.notes);

    // Multi-word unquoted spans, trailing non-ASCII identifier characters, embedded comments, and standalone GENERATED match SQLite.
    let span_sql = "CREATE TABLE spans (a UNSIGNED   BIG\n  INT NOT NULL, b INT/*mid*/EXTRA NOT NULL, c INT GENERATED PRIMARY KEY, tail\u{00a0})";
    assert_eq!(
        names_and_types(span_sql),
        vec![
            ("a".into(), "UNSIGNED   BIG\n  INT".into()),
            ("b".into(), "INT/*mid*/EXTRA".into()),
            ("c".into(), "INT GENERATED".into()),
            ("tail\u{00a0}".into(), "".into()),
        ]
    );
}

/// Unquoted/quoted column identifiers immediately adjacent to quoted/unquoted types (`id"INTEGER"`,
/// `id[INTEGER]`, ``id`INTEGER` ``, `id'INTEGER'`, `[id]INTEGER`, `"v"REAL`) and table-level quoted
/// keys (`PRIMARY KEY([id])`, ``PRIMARY KEY(`id`)``) split cleanly and preserve rowid aliases.
#[test]
fn unquoted_column_names_split_before_quoted_types_and_preserve_rowid_aliases() {
    for (sql, expected_second_type, expected_second_val) in [
        ("CREATE TABLE t (id\"INTEGER\" PRIMARY KEY, v\"TEXT\" DEFAULT 'hi')", "TEXT", text("hi")),
        ("CREATE TABLE t (id[INTEGER] PRIMARY KEY, v[REAL] DEFAULT 5)", "REAL", CellValue::Real(5.0)),
        ("CREATE TABLE t (id`INTEGER` PRIMARY KEY, v'BLOB' DEFAULT '5')", "BLOB", text("5")),
        ("CREATE TABLE t (id'INTEGER'PRIMARY KEY, v\"NUMERIC\"DEFAULT '7')", "NUMERIC", CellValue::Int(7)),
        ("CREATE TABLE t ([id]INTEGER PRIMARY KEY, \"v\"REAL DEFAULT-.5)", "REAL", CellValue::Real(-0.5)),
        ("CREATE TABLE t (id INTEGER, v TEXT DEFAULT 'ok', PRIMARY KEY([id]))", "TEXT", text("ok")),
        ("CREATE TABLE t (id INTEGER, v TEXT DEFAULT 'ok', PRIMARY KEY(`id`))", "TEXT", text("ok")),
        ("CREATE TABLE t (id INTEGER, v TEXT DEFAULT 'ok', PRIMARY KEY('id'))", "TEXT", text("ok")),
    ] {
        assert_eq!(names_and_types(sql), vec![("id".into(), "INTEGER".into()), ("v".into(), expected_second_type.into())], "{sql}");
        let bytes = table_db(sql, &[(42, vec![Val::Null])]);
        let page = SqliteDb::parse(&bytes).expect("valid sqlite fixture").read_rows(2, 0, 10);
        assert_eq!(page.rows[0].values, vec![CellValue::Int(42), expected_second_val], "{sql}");
        assert!(page.notes.is_empty(), "{sql}: {:?}", page.notes);
    }

    // Non-alias tight forms (`INT`, `DESC`, type suffixes, and unary `+` expression keys) must keep stored NULLs.
    for non_alias_sql in [
        "CREATE TABLE t (id\"INT\"PRIMARY KEY, v TEXT)",
        "CREATE TABLE t (id\"INTEGER\"(10)PRIMARY KEY, v TEXT)",
        "CREATE TABLE t (id\"INTEGER\"EXTRA PRIMARY KEY, v TEXT)",
        "CREATE TABLE t (id\"INTEGER\"PRIMARY KEY DESC, v TEXT)",
        "CREATE TABLE t (id\"INTEGER\", v TEXT, PRIMARY KEY(+id))",
        "CREATE TABLE t (\"\" INTEGER, v TEXT, PRIMARY KEY(+ASC))",
    ] {
        let bytes = table_db(non_alias_sql, &[(42, vec![Val::Null, Val::Text("x".into())])]);
        let page = SqliteDb::parse(&bytes).expect("valid sqlite fixture").read_rows(2, 0, 10);
        assert_eq!(page.rows[0].values, vec![CellValue::Null, text("x")], "{non_alias_sql}");
    }
}

/// `DEFAULT` immediately followed by `+`, `-`, or a leading-dot decimal (`DEFAULT-5`, `DEFAULT+5`,
/// `DEFAULT.5`, `DEFAULT-.5E-2`, `DEFAULT-'5'`, `DEFAULT+'5'`) terminates the type, preserves rowid
/// aliases, and fills missing columns, while hex `e`/`E` digits and `--` line comments after `e`
/// never swallow binary `+`/`-` or comment markers as scientific exponents.
#[test]
fn tight_signed_and_leading_dot_defaults_preserve_types_aliases_and_values() {
    let sql = concat!(
        "CREATE TABLE t (",
        "id INTEGER DEFAULT-1 CHECK (1 > 0x1e-- PRIMARY KEY (a)\n + 0) PRIMARY KEY, ",
        "a REAL DEFAULT-5, ",
        "b REAL DEFAULT+5, ",
        "c REAL DEFAULT.5, ",
        "d TEXT DEFAULT-'5', ",
        "e TEXT DEFAULT+'5', ",
        "f INT DEFAULT (-0x10), ",
        "g INT DEFAULT (+0X10), ",
        "h REAL DEFAULT-.5E-2, ",
        "bad1 REAL DEFAULT (0x1e+2), ",
        "bad2 REAL DEFAULT (0X1E+2)",
        ")"
    );
    assert_eq!(
        names_and_types(sql),
        vec![
            ("id".into(), "INTEGER".into()),
            ("a".into(), "REAL".into()),
            ("b".into(), "REAL".into()),
            ("c".into(), "REAL".into()),
            ("d".into(), "TEXT".into()),
            ("e".into(), "TEXT".into()),
            ("f".into(), "INT".into()),
            ("g".into(), "INT".into()),
            ("h".into(), "REAL".into()),
            ("bad1".into(), "REAL".into()),
            ("bad2".into(), "REAL".into()),
        ]
    );

    let bytes = table_db(sql, &[(19, vec![Val::Null])]);
    let page = SqliteDb::parse(&bytes).expect("valid sqlite fixture").read_rows(2, 0, 10);
    assert_eq!(
        page.rows[0].values,
        vec![
            CellValue::Int(19),
            CellValue::Real(-5.0),
            CellValue::Real(5.0),
            CellValue::Real(0.5),
            text("-5"),
            text("5"),
            CellValue::Int(-16),
            CellValue::Int(16),
            CellValue::Real(-0.005),
            CellValue::Null,
            CellValue::Null,
        ]
    );
    assert_eq!(
        page.notes,
        vec![
            "Column 10 has an unsupported DEFAULT expression; missing values were filled as NULL.",
            "Column 11 has an unsupported DEFAULT expression; missing values were filled as NULL.",
        ]
    );
}

/// Trailing semicolons and tight combined table options (`)WITHOUT ROWID,STRICT;`) do not mask
/// `STRICT` or `WITHOUT ROWID`.
#[test]
fn semicolon_terminated_table_options_preserve_strict_and_without_rowid() {
    let strict_sql = "CREATE TABLE t (n INT, a ANY DEFAULT '5') STRICT;";
    let strict_bytes = table_db(strict_sql, &[(1, vec![Val::Int(1)])]);
    let strict_page = SqliteDb::parse(&strict_bytes).expect("valid sqlite fixture").read_rows(2, 0, 10);
    assert_eq!(strict_page.rows[0].values, vec![CellValue::Int(1), text("5")]);

    let wr_sql = "CREATE TABLE t (v TEXT, k INTEGER PRIMARY KEY) WITHOUT ROWID;";
    let wr_bytes = without_rowid_db(wr_sql, &[vec![Val::Int(7), Val::Text("hi".into())]]);
    let wr_page = SqliteDb::parse(&wr_bytes).expect("valid sqlite fixture").read_rows(2, 0, 10);
    assert_eq!(wr_page.rows[0].rowid, None);
    assert_eq!(wr_page.rows[0].values, vec![text("hi"), CellValue::Int(7)]);
    assert!(wr_page.notes.is_empty(), "{:?}", wr_page.notes);

    let both_sql = "CREATE TABLE t(first TEXT, [k]INTEGER PRIMARY KEY, v ANY DEFAULT '5')WITHOUT ROWID,STRICT;";
    let both_bytes = without_rowid_db(both_sql, &[vec![Val::Int(7), Val::Text("hi".into())]]);
    let both_page = SqliteDb::parse(&both_bytes).expect("valid sqlite fixture").read_rows(2, 0, 10);
    assert_eq!(both_page.rows[0].rowid, None);
    assert_eq!(both_page.rows[0].values, vec![text("hi"), CellValue::Int(7), text("5")]);
    assert!(both_page.notes.is_empty(), "{:?}", both_page.notes);
}

/// Defaults SQLite 3.50.4 supplies to rows written before the column was added: a blob after a
/// tight unary plus stays a blob, and a bare word with non-ASCII characters or a later `$` is the
/// word's text. `CURRENT_TIME` is a separate, synthetic case: SQLite refuses to add a column with
/// it to a table that has rows, so no old row gets a value from it. The test kit writes the row
/// anyway, and the reader falls back to NULL with a note.
#[test]
fn plus_blob_and_non_ascii_bare_defaults_read_like_sqlite() {
    let sql = concat!(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, ",
        "a BLOB DEFAULT+X'41', b TEXT DEFAULT+x'4142', c TEXT DEFAULT north\u{a0}east, ",
        "d TEXT DEFAULT \u{e9}t\u{e9}, e NUMERIC DEFAULT \u{4e2d}1, f TEXT DEFAULT a$b, g TEXT DEFAULT CURRENT_TIME)"
    );
    let bytes = table_db(sql, &[(1, vec![Val::Null])]);
    let page = SqliteDb::parse(&bytes).expect("valid sqlite fixture").read_rows(2, 0, 10);
    assert_eq!(
        page.rows[0].values,
        vec![
            CellValue::Int(1),
            CellValue::Blob { len: 1, sha256_prefix: "559aead08264d579".into() },
            CellValue::Blob { len: 2, sha256_prefix: "38164fbd17603d73".into() },
            text("north\u{a0}east"),
            text("\u{e9}t\u{e9}"),
            text("\u{4e2d}1"),
            text("a$b"),
            // `CURRENT_TIME`: the reader's own fallback, not a value SQLite supplies.
            CellValue::Null,
        ]
    );
    assert_eq!(page.notes, vec!["Column 8 has an unsupported DEFAULT expression; missing values were filled as NULL."]);
}

/// SQLite 3.50.4 skips U+FEFF where a token starts, but keeps it inside a word:
/// `\u{feff}a \u{feff}INT DEFAULT \u{feff}5` is column `a`, type `INT`, default 5, while
/// `DEFAULT a\u{feff}b` is the text `a\u{feff}b` and `DEFAULT\u{feff}5` is one word, so no default.
#[test]
fn byte_order_marks_at_token_starts_are_skipped() {
    let sql = "CREATE TABLE t (id INTEGER PRIMARY KEY, \u{feff}a \u{feff}INT DEFAULT \u{feff}5, b TEXT DEFAULT a\u{feff}b, c TEXT DEFAULT \u{feff}\u{feff}7, d INT DEFAULT\u{feff}5)";
    let cols = names_and_types(sql);
    assert_eq!(cols.get(1), Some(&("a".to_string(), "INT".to_string())), "{cols:?}");
    let bytes = table_db(sql, &[(1, vec![Val::Null])]);
    let page = SqliteDb::parse(&bytes).expect("valid sqlite fixture").read_rows(2, 0, 10);
    assert_eq!(page.rows[0].values, vec![CellValue::Int(1), CellValue::Int(5), text("a\u{feff}b"), text("7"), CellValue::Null]);
}
