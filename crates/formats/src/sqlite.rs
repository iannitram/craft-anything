//! Read-only parser for the SQLite 3 database file format (clean-room, from the public
//! on-disk documentation). It reads the 100-byte database header, b-tree pages (interior and
//! leaf, table and index), the record / serial-type encoding and the `sqlite_schema` table, and
//! can count a table's rows and read a page of them. Nothing is executed and nothing is written.
//!
//! Everything is bounded: inputs are capped, b-tree descent carries a depth limit and a
//! visited-page set to stop cycles, the number of pages walked and rows read is capped, cell
//! payloads (including overflow chains) are capped, and every read is bounds-checked. No input
//! can cause a panic.

use crate::FormatError;
use serde::Serialize;
use std::collections::BTreeSet;

/// Largest database file we read into memory (256 MiB).
pub const MAX_INPUT: usize = 256 << 20;
/// Longest text value decoded (bytes of source); longer values are truncated with a marker.
const MAX_TEXT: usize = 4096;
/// Largest assembled cell payload we follow across overflow pages.
const MAX_CELL_SIZE: usize = 8 << 20;
/// Most columns decoded from a single record.
const MAX_COLUMNS: usize = 4096;
/// Most overflow pages followed for one cell.
const MAX_OVERFLOW_PAGES: usize = 4096;
/// Most pages visited while counting or reading one table's b-tree.
const MAX_PAGES_WALKED: usize = 500_000;
/// Deepest b-tree descent (SQLite trees are shallow; this only guards hostile files).
const MAX_DEPTH: usize = 64;
/// Hard cap on rows returned in one read.
pub const MAX_ROWS: usize = 100_000;
/// Most schema objects listed.
const MAX_SCHEMA_OBJECTS: usize = 100_000;
/// Longest schema text read whole (SQLite's own default limit on a statement is 1,000,000
/// bytes). A CREATE statement is parsed whole, so the columns, rowid alias and `WITHOUT ROWID`
/// after its first [`MAX_TEXT`] bytes still count; the statement shown is cut at [`MAX_TEXT`].
const MAX_SCHEMA_TEXT: usize = 1 << 20;
/// Schema text one walk keeps whole in all. Past it, schema text is cut at [`MAX_TEXT`] like other
/// text, so schema rows that share an overflow chain can't multiply the file's size.
const MAX_SCHEMA_TEXT_TOTAL: usize = 32 << 20;

const MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// The 100-byte database header.
#[derive(Debug, Clone, Serialize)]
pub struct Header {
    /// Page size in bytes (512..=65536, a power of two).
    pub page_size: u32,
    /// File-format write version: 1 = legacy (rollback journal), 2 = WAL.
    pub write_version: u8,
    /// File-format read version: 1 = legacy, 2 = WAL.
    pub read_version: u8,
    /// Bytes reserved at the end of every page (usually 0).
    pub reserved_space: u8,
    /// Incremented on every change; used to spot stale caches.
    pub file_change_counter: u32,
    /// Database size in pages recorded in the header (valid only when it matches the file).
    pub db_size_pages: u32,
    /// Page number of the first freelist trunk page (0 when the freelist is empty).
    pub freelist_trunk: u32,
    /// Total number of freelist pages.
    pub freelist_pages: u32,
    pub schema_cookie: u32,
    /// Schema layer format number (1..=4).
    pub schema_format: u32,
    pub default_cache_size: u32,
    /// Page number of the largest root b-tree page (auto/incremental vacuum), else 0.
    pub largest_root_page: u32,
    /// `utf-8`, `utf-16le`, `utf-16be` or `unknown`.
    pub text_encoding: &'static str,
    pub user_version: u32,
    /// Non-zero when incremental-vacuum mode is on.
    pub incremental_vacuum: u32,
    pub application_id: u32,
    pub version_valid_for: u32,
    /// SQLITE_VERSION_NUMBER of the library that last wrote the file (e.g. 3045001 = 3.45.1).
    pub sqlite_version_number: u32,
    /// Human form of `sqlite_version_number`, e.g. `3.45.1`.
    pub sqlite_version: String,
    /// True when the file declares WAL mode (write or read version == 2).
    pub wal_mode: bool,
}

/// One row of the `sqlite_schema` table: a table, index, view or trigger.
#[derive(Debug, Clone, Serialize)]
pub struct SchemaObject {
    /// `table`, `index`, `view` or `trigger`.
    pub obj_type: String,
    pub name: String,
    /// The table this object belongs to (for an index/trigger it's the parent table).
    pub tbl_name: String,
    /// Root b-tree page (0 for views and triggers, which have no b-tree).
    pub rootpage: u32,
    /// The original CREATE statement, cut at 4 KiB for display; `columns` come from the whole
    /// statement.
    pub sql: String,
    /// Columns parsed from the CREATE statement (tables only, best effort).
    pub columns: Vec<Column>,
}

/// A column declared in a CREATE TABLE statement (parsed from SQL text, best effort).
#[derive(Debug, Clone, Serialize)]
pub struct Column {
    pub name: String,
    /// Declared type, or empty when none was given.
    pub type_name: String,
}

/// A whole-database overview: header fields, integrity notes and the schema.
#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub header: Header,
    /// File length in bytes.
    pub file_size: u64,
    /// Page count derived from the file length.
    pub page_count_from_size: u64,
    /// Non-fatal observations about consistency.
    pub integrity_notes: Vec<String>,
    pub schema: Vec<SchemaObject>,
    /// True when the schema listing hit the object cap.
    pub schema_truncated: bool,
}

/// A decoded cell value, rendered honestly: BLOBs are never dumped whole.
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    Null,
    Int(i64),
    Real(f64),
    Text { text: String, truncated: bool },
    Blob { len: usize, sha256_prefix: String },
    PartialBlob { len: usize },
    NotRead,
}

impl Serialize for CellValue {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            CellValue::Null => s.serialize_none(),
            CellValue::Int(v) => s.serialize_i64(*v),
            CellValue::Real(v) => s.serialize_f64(*v),
            CellValue::Text { text, truncated } => {
                if *truncated {
                    let mut m = s.serialize_map(Some(2))?;
                    m.serialize_entry("text", text)?;
                    m.serialize_entry("truncated", &true)?;
                    m.end()
                } else {
                    s.serialize_str(text)
                }
            }
            CellValue::Blob { len, sha256_prefix } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("blob_len", len)?;
                m.serialize_entry("sha256_prefix", sha256_prefix)?;
                m.end()
            }
            CellValue::PartialBlob { len } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("blob_len", len)?;
                m.serialize_entry("truncated", &true)?;
                m.end()
            }
            CellValue::NotRead => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("not_read", &true)?;
                m.end()
            }
        }
    }
}

/// One table row.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    /// Absent for WITHOUT ROWID tables.
    pub rowid: Option<i64>,
    pub values: Vec<CellValue>,
}

/// A page of rows read from one table.
#[derive(Debug, Clone, Serialize)]
pub struct RowPage {
    pub rows: Vec<Row>,
    /// Rows skipped before the page (the requested offset, clamped to what exists).
    pub offset: usize,
    /// True when a row cap, budget or unreadable part of the tree may have hidden more rows.
    pub truncated: bool,
    /// Non-fatal observations (corruption skipped, overflow capped, …).
    pub notes: Vec<String>,
}

/// A table's row count.
#[derive(Debug, Clone, Serialize)]
pub struct RowCount {
    pub count: u64,
    /// True when a budget or an unreadable page/child stopped the walk; the count is a lower bound.
    pub bounded: bool,
}

/// Accumulator for a bounded b-tree row walk: retained rows, a visited-page set to stop cycles,
/// the remaining page-walk budget, the requested row range, and any notes.
struct RowWalk {
    index_tree: bool,
    visited: BTreeSet<u32>,
    budget: usize,
    out: Vec<Row>,
    notes: Vec<String>,
    incomplete: bool,
    /// Stop once this many readable rows have been consumed, including skipped rows.
    want: usize,
    seen: usize,
    /// Decode but immediately discard rows before this offset.
    skip: usize,
    /// Longest text value decoded whole, and the whole text still allowed above [`MAX_TEXT`].
    text_cap: usize,
    text_budget: usize,
}

impl RowWalk {
    fn new(want: usize) -> Self {
        RowWalk {
            index_tree: false,
            visited: BTreeSet::new(),
            budget: MAX_PAGES_WALKED,
            out: Vec::new(),
            notes: Vec::new(),
            incomplete: false,
            want,
            seen: 0,
            skip: 0,
            text_cap: MAX_TEXT,
            text_budget: 0,
        }
    }

    /// Count a readable row, retaining it only after the requested offset. While text past
    /// [`MAX_TEXT`] is retained, it spends the budget; later rows are cut once the budget is spent.
    fn keep(&mut self, row: Row) {
        self.seen = self.seen.saturating_add(1);
        if self.seen <= self.skip {
            return;
        }
        if self.text_cap > MAX_TEXT {
            let text: usize = row.values.iter().map(|value| if let CellValue::Text { text, .. } = value { text.len() } else { 0 }).sum();
            self.text_budget = self.text_budget.saturating_sub(text);
            if self.text_budget == 0 {
                self.text_cap = MAX_TEXT;
            }
        }
        self.out.push(row);
    }
}

/// A parsed SQLite database, holding a borrow of the file bytes.
pub struct SqliteDb<'a> {
    data: &'a [u8],
    header: Header,
    usable: usize,
}

/// A b-tree page limited to its available, non-reserved bytes.
struct BtreePage<'a> {
    data: &'a [u8],
    header: usize,
    kind: u8,
    cells: usize,
    pointers: usize,
    pointers_end: usize,
}

impl BtreePage<'_> {
    fn cell_offset(&self, index: usize) -> Option<usize> {
        if index >= self.cells {
            return None;
        }
        let pointer = self.pointers.checked_add(index.checked_mul(2)?)?;
        let offset = usize::from(read_u16_be(self.data, pointer)?);
        (offset >= self.pointers_end && offset < self.data.len()).then_some(offset)
    }
}

/// True when `data` starts with the SQLite 3 magic string.
pub fn is_sqlite(data: &[u8]) -> bool {
    data.len() >= 16 && data.get(..16) == Some(MAGIC.as_slice())
}

fn read_u32_be(data: &[u8], off: usize) -> Option<u32> {
    let b: [u8; 4] = data.get(off..off.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_be_bytes(b))
}

fn read_u16_be(data: &[u8], off: usize) -> Option<u16> {
    let b: [u8; 2] = data.get(off..off.checked_add(2)?)?.try_into().ok()?;
    Some(u16::from_be_bytes(b))
}

/// A SQLite big-endian varint (up to 9 bytes): (value, bytes consumed).
fn varint(data: &[u8], off: usize) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut i = 0usize;
    while i < 9 {
        let b = *data.get(off.checked_add(i)?)?;
        if i == 8 {
            result = (result << 8) | u64::from(b);
            return Some((result, 9));
        }
        result = (result << 7) | u64::from(b & 0x7f);
        if b & 0x80 == 0 {
            return Some((result, i + 1));
        }
        i += 1;
    }
    None
}

fn version_string(n: u32) -> String {
    // SQLITE_VERSION_NUMBER = major*1_000_000 + minor*1_000 + patch.
    let major = n / 1_000_000;
    let minor = (n / 1_000) % 1_000;
    let patch = n % 1_000;
    format!("{major}.{minor}.{patch}")
}

impl<'a> SqliteDb<'a> {
    /// Parse the database header. Fails only when the magic or page size is wrong.
    pub fn parse(data: &'a [u8]) -> Result<SqliteDb<'a>, FormatError> {
        if data.len() > MAX_INPUT {
            return Err(FormatError::TooLarge(format!("{} bytes; the limit is {MAX_INPUT}", data.len())));
        }
        if !is_sqlite(data) {
            return Err(FormatError::Unsupported("missing the \"SQLite format 3\" signature".into()));
        }
        if data.len() < 100 {
            return Err(FormatError::Truncated("the 100-byte header does not fit".into()));
        }
        let raw_ps = read_u16_be(data, 16).unwrap_or(0);
        let page_size: u32 = if raw_ps == 1 { 65_536 } else { u32::from(raw_ps) };
        if page_size < 512 || !page_size.is_power_of_two() {
            return Err(FormatError::Malformed(format!("page size {page_size} is not a power of two in 512..=65536")));
        }
        let write_version = *data.get(18).unwrap_or(&0);
        let read_version = *data.get(19).unwrap_or(&0);
        let reserved_space = *data.get(20).unwrap_or(&0);
        let usable = (page_size as usize).saturating_sub(reserved_space as usize);
        if usable < 480 {
            return Err(FormatError::Malformed(format!("usable page size {usable} is too small")));
        }
        let text_encoding = match read_u32_be(data, 56).unwrap_or(0) {
            1 => "utf-8",
            2 => "utf-16le",
            3 => "utf-16be",
            0 => "utf-8", // a fresh database leaves this 0; UTF-8 is the default.
            _ => "unknown",
        };
        let sqlite_version_number = read_u32_be(data, 96).unwrap_or(0);
        let header = Header {
            page_size,
            write_version,
            read_version,
            reserved_space,
            file_change_counter: read_u32_be(data, 24).unwrap_or(0),
            db_size_pages: read_u32_be(data, 28).unwrap_or(0),
            freelist_trunk: read_u32_be(data, 32).unwrap_or(0),
            freelist_pages: read_u32_be(data, 36).unwrap_or(0),
            schema_cookie: read_u32_be(data, 40).unwrap_or(0),
            schema_format: read_u32_be(data, 44).unwrap_or(0),
            default_cache_size: read_u32_be(data, 48).unwrap_or(0),
            largest_root_page: read_u32_be(data, 52).unwrap_or(0),
            text_encoding,
            user_version: read_u32_be(data, 60).unwrap_or(0),
            incremental_vacuum: read_u32_be(data, 64).unwrap_or(0),
            application_id: read_u32_be(data, 68).unwrap_or(0),
            version_valid_for: read_u32_be(data, 92).unwrap_or(0),
            sqlite_version_number,
            sqlite_version: version_string(sqlite_version_number),
            wal_mode: write_version == 2 || read_version == 2,
        };
        Ok(SqliteDb { data, header, usable })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    fn page_size(&self) -> usize {
        self.header.page_size as usize
    }

    /// Total pages implied by the file length.
    fn page_count(&self) -> u64 {
        (self.data.len() as u64) / (self.header.page_size as u64)
    }

    /// Byte offset of page `pageno` (1-based).
    fn page_base(&self, pageno: u32) -> Option<usize> {
        let p = (pageno as usize).checked_sub(1)?;
        p.checked_mul(self.page_size())
    }

    /// Byte offset where the b-tree page header begins (page 1 follows the 100-byte db header).
    fn btree_header_off(&self, pageno: u32) -> Option<usize> {
        let base = self.page_base(pageno)?;
        if pageno == 1 { base.checked_add(100) } else { Some(base) }
    }

    fn btree_page(&self, pageno: u32) -> Option<BtreePage<'_>> {
        let base = self.page_base(pageno)?;
        let end = base.checked_add(self.usable)?.min(self.data.len());
        let data = self.data.get(base..end)?;
        let header: usize = if pageno == 1 { 100 } else { 0 };
        let kind = *data.get(header)?;
        let header_size = match kind {
            0x05 | 0x02 => 12,
            0x0d | 0x0a => 8,
            _ => return None,
        };
        let cells = usize::from(read_u16_be(data, header.checked_add(3)?)?);
        let pointers = header.checked_add(header_size)?;
        let pointers_end = pointers.checked_add(cells.checked_mul(2)?)?;
        data.get(header..pointers_end)?;
        Some(BtreePage { data, header, kind, cells, pointers, pointers_end })
    }

    /// Build the whole-database overview, including the schema.
    pub fn overview(&self) -> Overview {
        let (schema, schema_truncated) = self.schema();
        let file_size = self.data.len() as u64;
        let page_count_from_size = self.page_count();
        let mut notes = Vec::new();
        if !self.data.len().is_multiple_of(self.page_size()) {
            notes.push("The file length is not a whole number of pages; it may be truncated or a header is stale.".into());
        }
        if self.header.db_size_pages != 0 && u64::from(self.header.db_size_pages) != page_count_from_size {
            notes.push(format!(
                "Header records {} pages but the file holds {page_count_from_size}; a -wal file may hold newer pages, or the header is stale.",
                self.header.db_size_pages
            ));
        }
        if self.header.wal_mode {
            notes.push("The database is in WAL mode: the newest committed rows may live in a -wal sidecar, not in this file.".into());
        }
        if self.header.freelist_pages > 0 {
            notes.push(format!("{} page(s) are on the freelist (deleted content may linger there; it is not read).", self.header.freelist_pages));
        }
        Overview { header: self.header.clone(), file_size, page_count_from_size, integrity_notes: notes, schema, schema_truncated }
    }

    /// Walk the `sqlite_schema` b-tree (rooted at page 1) and return its objects. Columns are
    /// parsed from the whole CREATE statement; the text shown is cut at [`MAX_TEXT`] bytes.
    pub fn schema(&self) -> (Vec<SchemaObject>, bool) {
        let (mut objects, truncated) = self.whole_schema();
        for object in &mut objects {
            for text in [&mut object.obj_type, &mut object.name, &mut object.tbl_name, &mut object.sql] {
                cut_at_max_text(text);
            }
        }
        (objects, truncated)
    }

    /// The schema objects with their whole text (up to [`MAX_SCHEMA_TEXT`] while
    /// [`MAX_SCHEMA_TEXT_TOTAL`] lasts), for reading rows.
    fn whole_schema(&self) -> (Vec<SchemaObject>, bool) {
        let mut out = Vec::new();
        let mut w = RowWalk::new(MAX_SCHEMA_OBJECTS);
        w.text_cap = MAX_SCHEMA_TEXT;
        w.text_budget = MAX_SCHEMA_TEXT_TOTAL;
        self.collect_table_rows(&mut w, 1, 0);
        let truncated = w.out.len() >= MAX_SCHEMA_OBJECTS;
        for row in w.out {
            // sqlite_schema columns: type, name, tbl_name, rootpage, sql.
            let get_text = |i: usize| match row.values.get(i) {
                Some(CellValue::Text { text, .. }) => text.clone(),
                _ => String::new(),
            };
            let obj_type = get_text(0);
            let name = get_text(1);
            let tbl_name = get_text(2);
            let rootpage = match row.values.get(3) {
                Some(CellValue::Int(v)) if *v >= 0 => *v as u32,
                _ => 0,
            };
            let sql = get_text(4);
            let columns = if obj_type == "table" { parse_create_columns(&sql) } else { Vec::new() };
            out.push(SchemaObject { obj_type, name, tbl_name, rootpage, sql, columns });
        }
        (out, truncated)
    }

    /// Find a schema object (table/index/view/trigger) by name (case-insensitive).
    pub fn find_object(&self, name: &str) -> Option<SchemaObject> {
        let (schema, _) = self.schema();
        schema.into_iter().find(|o| o.name.eq_ignore_ascii_case(name))
    }

    fn without_rowid_table(&self, rootpage: u32) -> Option<SchemaObject> {
        self.whole_schema().0.into_iter().find(|obj| obj.obj_type == "table" && obj.rootpage == rootpage && is_without_rowid(&obj.sql))
    }

    /// Count the rows of the table rooted at `rootpage`, including interior index records.
    pub fn count_rows(&self, rootpage: u32) -> RowCount {
        let mut count: u64 = 0;
        let mut visited = BTreeSet::new();
        let mut budget = MAX_PAGES_WALKED;
        let index_tree = self.btree_header_off(rootpage).and_then(|hdr| self.data.get(hdr)).is_some_and(|ptype| matches!(*ptype, 0x02 | 0x0a));
        let bounded = !self.count_leaf_cells(rootpage, 0, &mut visited, &mut budget, &mut count, index_tree);
        RowCount { count, bounded }
    }

    /// Read a page of rows from the table rooted at `rootpage`.
    pub fn read_rows(&self, rootpage: u32, offset: usize, limit: usize) -> RowPage {
        let limit = limit.min(MAX_ROWS);
        // Consume offset+limit readable rows, retaining only the requested page.
        let want = offset.saturating_add(limit);
        let mut w = RowWalk::new(want);
        w.skip = offset;
        let table = self.without_rowid_table(rootpage);
        w.index_tree = table.is_some();
        self.collect_table_rows(&mut w, rootpage, 0);
        if let Some(table) = table {
            if let Some(order) = without_rowid_order(&table) {
                // A row stored before ALTER TABLE ADD COLUMN ends early, and added columns come last
                // in both orders. Keep every stored field, and leave off the columns after the last
                // one so the defaults below fill them.
                for row in &mut w.out {
                    let stored = row.values.len();
                    let keep = order.iter().rposition(|slot| slot.is_some_and(|i| i < stored)).map_or(0, |last| last.saturating_add(1));
                    row.values = order
                        .iter()
                        .take(keep)
                        .map(|slot| match slot {
                            Some(i) => row.values.get(*i).cloned().unwrap_or(CellValue::Null),
                            None => CellValue::NotRead,
                        })
                        .collect();
                }
            } else {
                w.notes.push("Could not determine the primary-key column order; values remain in storage order.".into());
            }
        }
        let hit_cap = w.seen >= want && want > 0;
        let skipped = offset.min(w.seen);
        let mut page = w.out;
        let (schema, _) = self.whole_schema();
        if let Some(table) = schema.iter().find(|o| o.obj_type == "table" && o.rootpage == rootpage) {
            let definitions = column_definitions(&table.sql);
            let generated: Vec<bool> = definitions.iter().map(|(_, rest)| is_virtual_column(rest)).collect();
            let strict = is_strict(&table.sql);
            let affinities: Vec<Affinity> = definitions.iter().map(|(column, _)| column_affinity(&column.type_name, strict)).collect();
            // WITHOUT ROWID rows were already placed in declaration order above.
            if !w.index_tree && generated.contains(&true) {
                for row in &mut page {
                    row.values = place_stored_values(std::mem::take(&mut row.values), &generated);
                }
            }
            let defaults: Vec<_> = definitions.into_iter().zip(&affinities).map(|((_, rest), affinity)| column_default(rest, *affinity)).collect();
            let mut unsupported = vec![false; defaults.len()];
            for row in &mut page {
                for (((value, supported), disclosed), is_virtual) in defaults.iter().zip(&mut unsupported).zip(&generated).skip(row.values.len()) {
                    if *is_virtual {
                        row.values.push(CellValue::NotRead);
                        continue;
                    }
                    row.values.push(value.clone());
                    if !supported {
                        *disclosed = true;
                    }
                }
            }
            for (i, disclosed) in unsupported.into_iter().enumerate() {
                if disclosed {
                    w.notes.push(format!("Column {} has an unsupported DEFAULT expression; missing values were filled as NULL.", i + 1));
                }
            }
            for (i, is_virtual) in generated.iter().enumerate() {
                if *is_virtual {
                    w.notes.push(format!(
                        "Column {} is a VIRTUAL generated column; SQLite computes it when a row is read and stores no value, so it is shown as not read.",
                        i + 1
                    ));
                }
            }
            if let Some(index) = rowid_alias_index(&table.sql) {
                for row in &mut page {
                    if let (Some(rowid), Some(value @ CellValue::Null)) = (row.rowid, row.values.get_mut(index)) {
                        *value = CellValue::Int(rowid);
                    }
                }
            }
            // SQLite stores a REAL with no fractional part as an integer and turns it back into a
            // REAL when it reads a REAL column; a filled-in default is read the same way.
            for row in &mut page {
                for (value, _) in row.values.iter_mut().zip(&affinities).filter(|(_, affinity)| **affinity == Affinity::Real) {
                    if let CellValue::Int(n) = *value {
                        *value = CellValue::Real(n as f64);
                    }
                }
            }
        }
        RowPage { rows: page, offset: skipped, truncated: hit_cap || w.incomplete, notes: w.notes }
    }

    /// Recursively count rows. Returns false when a budget or an unreadable page/child stopped the walk.
    fn count_leaf_cells(&self, pageno: u32, depth: usize, visited: &mut BTreeSet<u32>, budget: &mut usize, count: &mut u64, index_tree: bool) -> bool {
        if depth > MAX_DEPTH || *budget == 0 {
            return false;
        }
        if pageno == 0 || !visited.insert(pageno) {
            return false; // A repeated or null child cannot establish a complete count.
        }
        *budget -= 1;
        let Some(page) = self.btree_page(pageno) else { return false };
        let ptype = page.kind;
        let ncells = page.cells;
        match ptype {
            0x0d if !index_tree => {
                *count = count.saturating_add(ncells as u64);
                true
            }
            0x0a if index_tree => {
                *count = count.saturating_add(ncells as u64);
                true
            }
            0x05 | 0x02 if (ptype == 0x02) == index_tree => {
                if index_tree {
                    *count = count.saturating_add(ncells as u64);
                }
                let mut ok = true;
                // Child pointers live in each cell; the right-most pointer is in the header.
                for i in 0..ncells {
                    if *budget == 0 {
                        return false;
                    }
                    if let Some(child) = Self::interior_child(&page, i) {
                        ok &= self.count_leaf_cells(child, depth + 1, visited, budget, count, index_tree);
                    } else {
                        ok = false;
                    }
                }
                if let Some(right) = read_u32_be(page.data, page.header.saturating_add(8)) {
                    ok &= self.count_leaf_cells(right, depth + 1, visited, budget, count, index_tree);
                }
                ok
            }
            _ => false, // A child must belong to the same kind of b-tree as its root.
        }
    }

    /// Recursively read rows in key order, stopping once `want` readable rows are consumed.
    fn collect_table_rows(&self, w: &mut RowWalk, pageno: u32, depth: usize) {
        if w.seen >= w.want {
            return;
        }
        if depth > MAX_DEPTH || w.budget == 0 {
            if !w.incomplete {
                w.notes.push(format!("Stopped before b-tree page {pageno}: page or depth budget exhausted."));
            }
            w.incomplete = true;
            return;
        }
        if pageno == 0 || !w.visited.insert(pageno) {
            if !w.incomplete {
                w.notes.push(format!("Skipped a null or repeated b-tree child page {pageno}."));
            }
            w.incomplete = true;
            return;
        }
        w.budget -= 1;
        let Some(page) = self.btree_page(pageno) else {
            w.incomplete = true;
            w.notes.push(format!("Skipped an unreadable b-tree page {pageno}."));
            return;
        };
        let ptype = page.kind;
        let ncells = page.cells;
        match ptype {
            0x0d | 0x0a if (ptype == 0x0a) == w.index_tree => {
                for i in 0..ncells {
                    if w.seen >= w.want {
                        return;
                    }
                    let row = page.cell_offset(i).and_then(|cell_off| {
                        if w.index_tree {
                            self.read_index_cell(page.data, cell_off, w.text_cap, &mut w.notes)
                        } else {
                            self.read_table_leaf_cell(page.data, cell_off, w.text_cap, &mut w.notes)
                        }
                    });
                    match row {
                        Some(row) => w.keep(row),
                        None => {
                            w.incomplete = true;
                            w.notes.push(format!("Skipped an unreadable row on page {pageno}."));
                        }
                    }
                }
            }
            0x05 | 0x02 if (ptype == 0x02) == w.index_tree => {
                for i in 0..ncells {
                    if w.seen >= w.want {
                        return;
                    }
                    if w.budget == 0 {
                        if !w.incomplete {
                            w.notes.push(format!("Stopped within b-tree page {pageno}: page budget exhausted."));
                        }
                        w.incomplete = true;
                        return;
                    }
                    if let Some(child) = Self::interior_child(&page, i) {
                        self.collect_table_rows(w, child, depth + 1);
                    } else {
                        w.incomplete = true;
                        w.notes.push(format!("Skipped an unreadable child pointer on page {pageno}."));
                    }
                    if w.index_tree {
                        if w.seen >= w.want {
                            return;
                        }
                        let row = page
                            .cell_offset(i)
                            .and_then(|off| off.checked_add(4))
                            .and_then(|off| self.read_index_cell(page.data, off, w.text_cap, &mut w.notes));
                        match row {
                            Some(row) => w.keep(row),
                            None => {
                                w.incomplete = true;
                                w.notes.push(format!("Skipped an unreadable row on page {pageno}."));
                            }
                        }
                    }
                }
                if let Some(right) = read_u32_be(page.data, page.header.saturating_add(8)) {
                    self.collect_table_rows(w, right, depth + 1);
                }
            }
            _ => {
                if !w.incomplete {
                    w.notes.push(format!("Skipped b-tree page {pageno}: its page kind does not match the root."));
                }
                w.incomplete = true;
            }
        }
    }

    /// The left-child page number of cell `i` on an interior page.
    fn interior_child(page: &BtreePage<'_>, i: usize) -> Option<u32> {
        read_u32_be(page.data, page.cell_offset(i)?)
    }

    /// Decode one table-leaf cell into a row, following overflow pages if needed.
    fn read_table_leaf_cell(&self, page: &[u8], cell_off: usize, text_cap: usize, notes: &mut Vec<String>) -> Option<Row> {
        let (payload_len, n1) = varint(page, cell_off)?;
        let (rowid, n2) = varint(page, cell_off.checked_add(n1)?)?;
        let body_off = cell_off.checked_add(n1)?.checked_add(n2)?;
        let payload_len = usize::try_from(payload_len).ok()?;
        let payload = self.assemble_payload(page, body_off, payload_len, true, notes)?;
        let values = parse_record(&payload, self.header.text_encoding, text_cap);
        let rowid = i64::from_le_bytes(rowid.to_le_bytes());
        Some(Row { rowid: Some(rowid), values })
    }

    /// Index cells hold a whole row, without a rowid, using the index local-payload formula.
    fn read_index_cell(&self, page: &[u8], cell_off: usize, text_cap: usize, notes: &mut Vec<String>) -> Option<Row> {
        let (payload_len, n) = varint(page, cell_off)?;
        let payload_len = usize::try_from(payload_len).ok()?;
        let payload = self.assemble_payload(page, cell_off.checked_add(n)?, payload_len, false, notes)?;
        Some(Row { rowid: None, values: parse_record(&payload, self.header.text_encoding, text_cap) })
    }

    /// Assemble a cell payload: the local part plus any overflow-page chain, capped.
    fn assemble_payload(&self, page: &[u8], local_off: usize, payload_len: usize, is_table_leaf: bool, notes: &mut Vec<String>) -> Option<Vec<u8>> {
        let usable = self.usable;
        // Maximum bytes stored on the page before spilling to overflow.
        let x = if is_table_leaf { usable.saturating_sub(35) } else { ((usable.saturating_sub(12)) * 64 / 255).saturating_sub(23) };
        let local = if payload_len <= x {
            payload_len
        } else {
            let m = ((usable.saturating_sub(12)) * 32 / 255).saturating_sub(23);
            let surplus = payload_len.saturating_sub(m);
            let denom = usable.saturating_sub(4).max(1);
            let k = m.saturating_add(surplus % denom);
            if k <= x { k } else { m }
        };
        let mut out = Vec::new();
        let first = page.get(local_off..local_off.checked_add(local)?)?;
        out.extend_from_slice(first);
        if local < payload_len {
            // The 4 bytes after the local part point at the first overflow page.
            let next_off = local_off.checked_add(local)?;
            let mut next = read_u32_be(page, next_off).unwrap_or(0);
            let per_page = usable.saturating_sub(4);
            let mut pages = 0usize;
            let mut visited = std::collections::BTreeSet::new();
            while next != 0 && out.len() < payload_len.min(MAX_CELL_SIZE) {
                if pages >= MAX_OVERFLOW_PAGES {
                    notes.push("An overflow chain was cut at the page cap; a value is partial.".into());
                    break;
                }
                if !visited.insert(next) {
                    notes.push("An overflow chain repeats a page; a value is partial.".into());
                    break;
                }
                pages += 1;
                let Some(pbase) = self.page_base(next) else { break };
                let Some(chunk) = self.data.get(pbase.checked_add(4)?..pbase.checked_add(4)?.checked_add(per_page).unwrap_or(pbase)) else {
                    break;
                };
                let remaining = payload_len.min(MAX_CELL_SIZE).saturating_sub(out.len());
                let take = remaining.min(chunk.len());
                out.extend_from_slice(chunk.get(..take)?);
                next = read_u32_be(self.data, pbase).unwrap_or(0);
            }
        }
        if out.len() < payload_len {
            notes.push("A cell payload was only partially read; some values are truncated or not read.".into());
        }
        Some(out)
    }
}

/// Decode a SQLite record (the "payload") into column values, cutting text at `text_cap` bytes.
fn parse_record(payload: &[u8], text_encoding: &str, text_cap: usize) -> Vec<CellValue> {
    let mut out = Vec::new();
    let Some((hdr_len, n)) = varint(payload, 0) else { return out };
    let hdr_len = hdr_len as usize;
    if hdr_len > payload.len() || hdr_len < n {
        return out;
    }
    // First read the serial types from the header region.
    let mut types = Vec::new();
    let mut p = n;
    while p < hdr_len {
        let Some((st, used)) = varint(payload, p) else { break };
        types.push(st);
        p = match p.checked_add(used) {
            Some(v) => v,
            None => break,
        };
        if types.len() >= MAX_COLUMNS {
            break;
        }
    }
    // Then read each value from the body region.
    let mut body = hdr_len;
    for st in types {
        let (val, size) = decode_serial(st, payload, body, text_encoding, text_cap);
        out.push(val);
        body = body.saturating_add(size);
    }
    out
}

/// Decode one serial-typed value at `off`; returns the value and its byte size.
fn decode_serial(serial: u64, data: &[u8], off: usize, text_encoding: &str, text_cap: usize) -> (CellValue, usize) {
    let size = match serial {
        1..=4 => serial as usize,
        5 => 6,
        6 | 7 => 8,
        n if n >= 12 => usize::try_from((n - 12) / 2).unwrap_or(usize::MAX),
        _ => 0,
    };
    if off > data.len() || (off == data.len() && size != 0) {
        return (CellValue::NotRead, size);
    }
    if serial < 12 && size > data.len().saturating_sub(off) {
        return (CellValue::NotRead, size);
    }
    match serial {
        0 => (CellValue::Null, 0),
        1 => (int_be(data, off, 1), 1),
        2 => (int_be(data, off, 2), 2),
        3 => (int_be(data, off, 3), 3),
        4 => (int_be(data, off, 4), 4),
        5 => (int_be(data, off, 6), 6),
        6 => (int_be(data, off, 8), 8),
        7 => {
            let bytes: Option<[u8; 8]> = data.get(off..off.saturating_add(8)).and_then(|s| s.try_into().ok());
            (bytes.map(|b| CellValue::Real(f64::from_be_bytes(b))).unwrap_or(CellValue::Null), 8)
        }
        8 => (CellValue::Int(0), 0),
        9 => (CellValue::Int(1), 0),
        10 | 11 => (CellValue::Null, 0), // reserved for internal use.
        n if n >= 12 && n % 2 == 0 => {
            let len = size;
            let slice = data.get(off..off.saturating_add(len).min(data.len())).unwrap_or(&[]);
            let value = if slice.len() < len { CellValue::PartialBlob { len } } else { blob_value(slice, len) };
            (value, len)
        }
        _ => {
            // Odd and >= 13: text of (n-13)/2 bytes, using the database's declared encoding.
            let len = size;
            let slice = data.get(off..off.saturating_add(len).min(data.len())).unwrap_or(&[]);
            let mut value = capped_text_value(slice, text_encoding, text_cap);
            if slice.len() < len
                && let CellValue::Text { truncated, .. } = &mut value
            {
                *truncated = true;
            }
            (value, len)
        }
    }
}

fn int_be(data: &[u8], off: usize, len: usize) -> CellValue {
    let Some(slice) = data.get(off..off.saturating_add(len)) else { return CellValue::Null };
    let mut v: i64 = if slice.first().is_some_and(|b| b & 0x80 != 0) { -1 } else { 0 };
    for &b in slice {
        v = (v << 8) | i64::from(b);
    }
    CellValue::Int(v)
}

fn text_value(slice: &[u8]) -> CellValue {
    encoded_text_value(slice, "utf-8")
}

/// Decode a TEXT value, cutting at [`MAX_TEXT`] bytes.
fn encoded_text_value(slice: &[u8], encoding: &str) -> CellValue {
    capped_text_value(slice, encoding, MAX_TEXT)
}

/// Decode a TEXT value, cutting at `cap` bytes. A truncated value ends at a character boundary,
/// so its shown prefix never gains a replacement character it didn't have; the flag still says
/// the value was cut.
fn capped_text_value(slice: &[u8], encoding: &str, cap: usize) -> CellValue {
    let truncated = slice.len() > cap;
    let cut = cap.min(slice.len());
    let text = match encoding {
        "utf-16le" | "utf-16be" => {
            let mut bytes = cut;
            if truncated && !bytes.is_multiple_of(2) {
                bytes -= 1; // a cut inside a code unit drops the half unit
            }
            let take = slice.get(..bytes).unwrap_or_default();
            let (pairs, _) = take.as_chunks::<2>();
            let mut units: Vec<u16> =
                pairs.iter().map(|&pair| if encoding == "utf-16le" { u16::from_le_bytes(pair) } else { u16::from_be_bytes(pair) }).collect();
            // A cut inside a surrogate pair leaves the high half alone: drop it rather than
            // render a replacement character that wasn't in the value.
            if truncated && units.last().is_some_and(|u| (0xD800..=0xDBFF).contains(u)) {
                units.pop();
            }
            let mut text = String::from_utf16_lossy(&units);
            if !truncated && !bytes.is_multiple_of(2) {
                text.push(char::REPLACEMENT_CHARACTER);
            }
            text
        }
        _ => {
            let take = slice.get(..cut).unwrap_or_default();
            // A cut can land inside a character: `from_utf8` reports an incomplete trailing
            // sequence (not an invalid byte), and its `valid_up_to` is the boundary to keep.
            // Genuinely invalid bytes keep the lossy prefix they had before.
            let end = if truncated {
                match std::str::from_utf8(take) {
                    Err(e) if e.error_len().is_none() => e.valid_up_to(),
                    _ => cut,
                }
            } else {
                cut
            };
            String::from_utf8_lossy(slice.get(..end).unwrap_or_default()).into_owned()
        }
    };
    CellValue::Text { text, truncated }
}

fn blob_value(slice: &[u8], len: usize) -> CellValue {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(slice);
    let prefix: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    CellValue::Blob { len, sha256_prefix: prefix }
}

/// Cut text shown to the reader at [`MAX_TEXT`] bytes, on a character boundary.
fn cut_at_max_text(text: &mut String) {
    if text.len() > MAX_TEXT {
        let end = (0..=MAX_TEXT).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
        text.truncate(end);
    }
}

/// The words after a CREATE TABLE's column list, such as `WITHOUT ROWID` and `STRICT`.
fn table_options(sql: &str) -> Vec<&str> {
    let Some(close) = column_list_close(sql) else { return Vec::new() };
    sql_tokens(close.checked_add(1).and_then(|start| sql.get(start..)).unwrap_or(""))
}

/// The byte offset of the `)` that closes a CREATE TABLE's column list. It is found by tokens,
/// so a parenthesis in a comment, string or quoted name after the list (SQLite keeps a trailing
/// comment in the stored statement) doesn't count.
fn column_list_close(sql: &str) -> Option<usize> {
    let mut depth = 0usize;
    for token in sql_tokens(sql) {
        match token {
            "(" => depth = depth.saturating_add(1),
            ")" if depth == 1 => return (token.as_ptr() as usize).checked_sub(sql.as_ptr() as usize),
            ")" => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

fn is_without_rowid(sql: &str) -> bool {
    table_options(sql)
        .windows(2)
        .any(|pair| pair.first().is_some_and(|s| s.eq_ignore_ascii_case("WITHOUT")) && pair.get(1).is_some_and(|s| s.eq_ignore_ascii_case("ROWID")))
}

fn is_strict(sql: &str) -> bool {
    table_options(sql).iter().any(|word| word.eq_ignore_ascii_case("STRICT"))
}

/// Output-column positions in the stored record: PK columns first, then the remaining stored
/// columns. A VIRTUAL generated column has no field in the record, so its position is `None`.
fn without_rowid_order(table: &SchemaObject) -> Option<Vec<Option<usize>>> {
    let generated: Vec<bool> = column_definitions(&table.sql).iter().map(|(_, rest)| is_virtual_column(rest)).collect();
    let is_virtual = |i: usize| generated.get(i).copied().unwrap_or(false);
    let open = column_list_open(&table.sql)?;
    let close = column_list_close(&table.sql)?;
    let body = table.sql.get(open.checked_add(1)?..close)?;
    let mut stored = Vec::new();
    for segment in split_top_level(body) {
        let tokens = sql_tokens(segment);
        let Some(pk) = tokens
            .windows(2)
            .position(|pair| pair.first().is_some_and(|s| s.eq_ignore_ascii_case("PRIMARY")) && pair.get(1).is_some_and(|s| s.eq_ignore_ascii_case("KEY")))
        else {
            continue;
        };
        let table_constraint = tokens.first().is_some_and(|s| s.eq_ignore_ascii_case("PRIMARY") || s.eq_ignore_ascii_case("CONSTRAINT"));
        let names: Vec<&str> = if table_constraint {
            let tail = tokens.get(pk.checked_add(3)?..)?;
            let end = tail.iter().position(|s| *s == ")")?;
            tail.get(..end)?.split(|s| *s == ",").filter_map(|part| part.first().copied()).collect()
        } else {
            vec![*tokens.first()?]
        };
        for token in names {
            let (name, _) = take_identifier(token);
            let position = table.columns.iter().position(|column| column.name.eq_ignore_ascii_case(&name))?;
            if !stored.contains(&position) {
                stored.push(position);
            }
        }
    }
    if stored.is_empty() {
        return None;
    }
    for i in 0..table.columns.len() {
        if !stored.contains(&i) && !is_virtual(i) {
            stored.push(i);
        }
    }
    (0..table.columns.len()).map(|i| if is_virtual(i) { Some(None) } else { stored.iter().position(|&column| column == i).map(Some) }).collect()
}

/// A generated column (`AS (expr)`, with or without `GENERATED ALWAYS`) is VIRTUAL unless
/// `STORED` follows its expression. SQLite computes a VIRTUAL column when a row is read, so it
/// has no field in the stored record.
fn is_virtual_column(rest: &str) -> bool {
    let tokens = sql_tokens(rest);
    let mut depth = 0usize;
    let mut generated = false;
    let mut iter = tokens.iter();
    while let Some(token) = iter.next() {
        match *token {
            "(" => depth = depth.saturating_add(1),
            ")" => {
                depth = depth.saturating_sub(1);
                if generated && depth == 0 {
                    return !iter.next().is_some_and(|next| next.eq_ignore_ascii_case("STORED"));
                }
            }
            _ if depth == 0 && token.eq_ignore_ascii_case("AS") => generated = true,
            _ => {}
        }
    }
    generated
}

/// A column's affinity: how SQLite converts a value it reads into the column. INTEGER affinity
/// converts values the same way NUMERIC does, so it is NUMERIC here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Affinity {
    Text,
    Numeric,
    Real,
    Blob,
}

/// The affinity SQLite gives a column of this declared type. SQLite looks for these parts of
/// the type in order, ignoring case: INT means INTEGER; CHAR, CLOB or TEXT mean TEXT; BLOB (or no
/// type) means BLOB; REAL, FLOA or DOUB mean REAL; anything else is NUMERIC. So `FLOATING POINT`
/// is an INTEGER column. An ANY column in a STRICT table keeps values as they are, like BLOB.
fn column_affinity(type_name: &str, strict: bool) -> Affinity {
    let upper = type_name.to_ascii_uppercase();
    let has = |parts: &[&str]| parts.iter().any(|part| upper.contains(part));
    if has(&["INT"]) {
        Affinity::Numeric
    } else if has(&["CHAR", "CLOB", "TEXT"]) {
        Affinity::Text
    } else if upper.is_empty() || has(&["BLOB"]) || (strict && upper == "ANY") {
        Affinity::Blob
    } else if has(&["REAL", "FLOA", "DOUB"]) {
        Affinity::Real
    } else {
        Affinity::Numeric
    }
}

/// Spread a rowid table's record fields over its declared columns. A VIRTUAL generated column
/// takes no field, so it is marked not read and the next field belongs to the next stored
/// column. Placement stops where the record ends, so columns added later can take their
/// defaults.
fn place_stored_values(values: Vec<CellValue>, generated: &[bool]) -> Vec<CellValue> {
    let mut stored = values.into_iter().peekable();
    let mut placed = Vec::with_capacity(generated.len());
    for &is_virtual in generated {
        if stored.peek().is_none() {
            break;
        }
        if is_virtual {
            placed.push(CellValue::NotRead);
        } else if let Some(value) = stored.next() {
            placed.push(value);
        }
    }
    placed.extend(stored);
    placed
}

/// Parse column names and types from a CREATE TABLE statement (best effort, bounded).
pub fn parse_create_columns(sql: &str) -> Vec<Column> {
    column_definitions(sql).into_iter().map(|(column, _)| column).collect()
}

/// Each column of a CREATE TABLE, with the text that follows its name (type, constraints,
/// default).
fn column_definitions(sql: &str) -> Vec<(Column, &str)> {
    let mut cols = Vec::new();
    // Take the text inside the outermost parentheses.
    let Some(inner) = column_list_open(sql).and_then(|open| sql.get(open.checked_add(1)?..)) else { return cols };
    let mut depth = 1usize;
    let mut end = inner.len();
    for token in sql_tokens(inner) {
        match token {
            "(" => depth = depth.saturating_add(1),
            ")" => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 {
            end = (token.as_ptr() as usize).saturating_sub(inner.as_ptr() as usize);
            break;
        }
    }
    let body = inner.get(..end).unwrap_or("");
    for seg in split_top_level(body) {
        if cols.len() >= MAX_COLUMNS {
            break;
        }
        // A comment in front of a definition (often the previous line's note) isn't part of it.
        let Some(first) = sql_tokens(seg).into_iter().next() else { continue };
        let seg = seg.get((first.as_ptr() as usize).saturating_sub(seg.as_ptr() as usize)..).unwrap_or("").trim_end();
        // Skip table-level constraints.
        let first_word = first.to_ascii_uppercase();
        if matches!(first_word.as_str(), "CONSTRAINT" | "PRIMARY" | "UNIQUE" | "CHECK" | "FOREIGN") {
            continue;
        }
        let (name, rest) = take_identifier(seg);
        if name.is_empty() && !seg.starts_with(['"', '`', '\'', '[']) {
            continue;
        }
        let type_name = extract_type(strip_comments(rest).trim());
        cols.push((Column { name, type_name }, rest));
    }
    cols
}

/// Where a CREATE TABLE's column list opens: the first parenthesis outside quotes, so a table
/// named `"export (1)"` doesn't start it.
fn column_list_open(sql: &str) -> Option<usize> {
    let open = sql_tokens(sql).into_iter().find(|token| *token == "(")?;
    (open.as_ptr() as usize).checked_sub(sql.as_ptr() as usize)
}

/// Identify the single INTEGER primary key, excluding the column-level DESC exception.
fn rowid_alias_index(sql: &str) -> Option<usize> {
    let tokens = sql_tokens(sql);
    let is = |token: &&str, word: &str| token.eq_ignore_ascii_case(word);
    let open = tokens.iter().position(|t| *t == "(")?;
    let mut depth = 0usize;
    let close = tokens.iter().enumerate().skip(open).find_map(|(i, t)| {
        match *t {
            "(" => depth = depth.saturating_add(1),
            ")" => depth = depth.saturating_sub(1),
            _ => {}
        }
        (depth == 0).then_some(i)
    })?;
    let tail = tokens.get(close.saturating_add(1)..)?;
    if tail.windows(2).any(|w| w.first().is_some_and(|t| is(t, "WITHOUT")) && w.get(1).is_some_and(|t| is(t, "ROWID"))) {
        return None;
    }
    let columns = parse_create_columns(sql);
    let mut key = None;
    let mut column = 0usize;
    let body_start = open.saturating_add(1);
    let mut start = body_start;
    depth = 0;
    for i in body_start..=close {
        let token = *tokens.get(i)?;
        if i == close || (token == "," && depth == 0) {
            let mut segment = tokens.get(start..i)?;
            start = i.saturating_add(1);
            if segment.first().is_some_and(|t| is(t, "CONSTRAINT")) {
                segment = segment.get(2..)?;
            }
            if segment.first().is_some_and(|t| is(t, "PRIMARY")) {
                if key.is_some() || !segment.get(1).is_some_and(|t| is(t, "KEY")) || segment.get(2) != Some(&"(") {
                    return None;
                }
                let end = segment.iter().position(|t| *t == ")")?;
                let members = segment.get(3..end)?;
                if members.contains(&",") {
                    return None;
                }
                let (name, _) = take_identifier(members.first()?);
                key = Some(columns.iter().position(|c| c.name.eq_ignore_ascii_case(&name))?);
            } else if !segment.first().is_some_and(|t| ["UNIQUE", "CHECK", "FOREIGN"].iter().any(|word| is(t, word))) {
                let mut nesting = 0usize;
                for (j, t) in segment.iter().enumerate().skip(1) {
                    match *t {
                        "(" => nesting = nesting.saturating_add(1),
                        ")" => nesting = nesting.saturating_sub(1),
                        _ => {}
                    }
                    if nesting == 0 && is(t, "PRIMARY") && segment.get(j.saturating_add(1)).is_some_and(|t| is(t, "KEY")) {
                        if key.is_some() || segment.get(j.saturating_add(2)).is_some_and(|t| is(t, "DESC")) {
                            return None;
                        }
                        key = Some(column);
                    }
                }
                column = column.saturating_add(1);
            }
        } else {
            match token {
                "(" => depth = depth.saturating_add(1),
                ")" => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    let index = key?;
    columns.get(index)?.type_name.eq_ignore_ascii_case("INTEGER").then_some(index)
}

/// Keep quoted text intact so defaults and constraint names cannot become SQL keywords.
fn sql_tokens(mut sql: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    while !sql.is_empty() {
        sql = sql.trim_start();
        if sql.is_empty() {
            break;
        }
        if sql.starts_with("--") {
            sql = sql.get(sql.find('\n').unwrap_or(sql.len())..).unwrap_or("");
            continue;
        }
        if sql.starts_with("/*") {
            sql = sql.get(2..).and_then(|s| s.find("*/").and_then(|end| s.get(end.saturating_add(2)..))).unwrap_or("");
            continue;
        }
        let mut chars = sql.char_indices().peekable();
        let Some((_, first)) = chars.next() else { break };
        let mut end = first.len_utf8();
        if matches!(first, '\'' | '"' | '`' | '[') {
            let quote = if first == '[' { ']' } else { first };
            while let Some((i, c)) = chars.next() {
                end = i.saturating_add(c.len_utf8());
                if c == quote {
                    if first != '[' && chars.peek().is_some_and(|(_, c)| *c == quote) {
                        if let Some((i, c)) = chars.next() {
                            end = i.saturating_add(c.len_utf8());
                        }
                    } else {
                        break;
                    }
                }
            }
        } else if !matches!(first, '(' | ')' | ',' | '/') {
            end = sql.find(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | '\'' | '"' | '`' | '[' | '/')).unwrap_or(sql.len());
            // A line comment may follow a word with no space between: `INTEGER--note`.
            if let Some(dash) = sql.get(..end).and_then(|word| word.find("--")).filter(|&dash| dash > 0) {
                end = dash;
            }
        }
        tokens.push(sql.get(..end).unwrap_or(""));
        sql = sql.get(end..).unwrap_or("");
    }
    tokens
}

/// `text` with each SQL comment replaced by a space. Quoted text is kept as it is.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut end = 0usize;
    for token in sql_tokens(text) {
        let start = (token.as_ptr() as usize).saturating_sub(text.as_ptr() as usize);
        // Only whitespace and comments lie between two tokens.
        let gap = text.get(end..start).unwrap_or("");
        out.push_str(if gap.contains("--") || gap.contains("/*") { " " } else { gap });
        out.push_str(token);
        end = start.saturating_add(token.len());
    }
    out
}

/// Missing trailing fields use literal defaults, read with the column's affinity; expressions
/// remain explicitly unknown.
fn column_default(rest: &str, affinity: Affinity) -> (CellValue, bool) {
    let tokens = sql_tokens(rest);
    let mut depth = 0usize;
    for (i, token) in tokens.iter().enumerate() {
        if depth == 0 && token.eq_ignore_ascii_case("DEFAULT") {
            return tokens
                .get(i.saturating_add(1)..)
                .and_then(|literal| literal_default(literal, affinity))
                .map(|v| (v, true))
                .unwrap_or((CellValue::Null, false));
        }
        match *token {
            "(" => depth = depth.saturating_add(1),
            ")" => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    (CellValue::Null, true)
}

/// A literal DEFAULT as SQLite reads it into a column of this affinity: `NULL`, booleans, quoted
/// or bare words, `X'…'`, numbers (with optional digit separators), hex integers and one
/// parenthesized literal; anything else (an expression, a keyword) stays unknown. A TEXT column
/// reads a number as text; a NUMERIC or REAL column reads
/// text that is a number as that number (a REAL column then turns a whole number into a REAL, as
/// it does for stored values); a BLOB column, or one with no type, keeps the literal's own type.
fn literal_default(tokens: &[&str], affinity: Affinity) -> Option<CellValue> {
    literal_value(tokens, affinity, true)
}

/// A top-level `DEFAULT` value, or one inside its parentheses. Bare and double-quoted words are
/// string constants only at the top level: SQLite rejects `(abc)` and `("abc")` as not constant.
fn literal_value(tokens: &[&str], affinity: Affinity, allow_words: bool) -> Option<CellValue> {
    let token = *tokens.first()?;
    if token == "(" {
        return parenthesized_literal(tokens, affinity);
    }
    if token.eq_ignore_ascii_case("NULL") {
        return Some(CellValue::Null);
    }
    if token.eq_ignore_ascii_case("TRUE") || token.eq_ignore_ascii_case("FALSE") {
        return Some(integer_default(i64::from(token.eq_ignore_ascii_case("TRUE")), affinity));
    }
    if let Some(text) = single_quoted(token) {
        return text_default(&text, affinity);
    }
    // A bare `x` is a word, but `X'…'` is a blob; a quoted next token decides which.
    let blob = token.eq_ignore_ascii_case("X") && tokens.get(1).is_some_and(|quoted| quoted.starts_with('\''));
    if allow_words && !blob && is_bare_word(token) {
        return text_default(token, affinity);
    }
    if allow_words && token.starts_with('"') && token.ends_with('"') && token.len() >= 2 {
        let text = token.get(1..token.len().checked_sub(1)?)?.replace("\"\"", "\"");
        return text_default(&text, affinity);
    }
    if token.eq_ignore_ascii_case("X") {
        // `X'…'` is a blob; a bare `x` is just a word, so only a quoted next token starts one.
        if let Some(quoted) = tokens.get(1).filter(|quoted| quoted.starts_with('\'')) {
            if !quoted.ends_with('\'') || quoted.len() < 2 {
                return None;
            }
            let hex = quoted.get(1..quoted.len().checked_sub(1)?)?;
            if !hex.len().is_multiple_of(2) {
                return None;
            }
            let bytes: Option<Vec<u8>> = hex
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| {
                    let high = char::from(*pair.first()?).to_digit(16)?;
                    let low = char::from(*pair.get(1)?).to_digit(16)?;
                    Some((high * 16 + low) as u8)
                })
                .collect();
            let bytes = bytes?;
            return Some(blob_value(&bytes, bytes.len()));
        }
    }
    if let Some(n) = hex_integer(token) {
        return Some(integer_default(n, affinity));
    }
    let number = if matches!(token, "+" | "-") {
        let value = *tokens.get(1)?;
        if let Some(text) = single_quoted(value) {
            if token == "+" {
                return text_default(&text, affinity);
            }
            return signed_quoted_default(&text, affinity);
        }
        if let Some(n) = hex_integer(value) {
            return Some(integer_default(if token == "-" { n.checked_neg()? } else { n }, affinity));
        }
        format!("{token}{value}")
    } else {
        token.to_owned()
    };
    let clean = unseparated_number(&number)?;
    let integer = number_kind(&clean)?;
    match affinity {
        // Reading an old row supplies the DEFAULT expression, not the stored value of a
        // later insert. SQLite formats a representable integer magnitude, but otherwise
        // keeps a separated literal's spelling (without underscores or a leading plus).
        Affinity::Text if clean != number => {
            let spelling = clean.strip_prefix('+').unwrap_or(&clean);
            let magnitude = spelling.strip_prefix('-').unwrap_or(spelling);
            Some(match magnitude.parse::<i64>() {
                Ok(n) if integer => integer_default(if spelling.starts_with('-') { -n } else { n }, affinity),
                _ => text_value(spelling.as_bytes()),
            })
        }
        // An integer literal that fits in 64 bits is written plainly (`007` reads '7', and
        // `-9223372036854775808` keeps its sign); any other number is written in SQLite's REAL
        // text form, since the schema's text is not the REAL it reads.
        Affinity::Text => Some(match clean.parse::<i64>() {
            Ok(n) if integer => integer_default(n, affinity),
            _ => {
                let real = clean.parse::<f64>().ok()?;
                text_value(real_text(real).as_bytes())
            }
        }),
        Affinity::Numeric | Affinity::Real => numeric_value(&clean).and_then(finite),
        Affinity::Blob => match clean.parse::<i64>() {
            Ok(n) if integer => Some(CellValue::Int(n)),
            _ => clean.parse::<f64>().ok().map(CellValue::Real).and_then(finite),
        },
    }
}

/// A quoted SQL string's content, or `None` when the token is not one.
fn single_quoted(token: &str) -> Option<String> {
    if token.starts_with('\'') && token.ends_with('\'') && token.len() >= 2 {
        Some(token.get(1..token.len().checked_sub(1)?)?.replace("''", "'"))
    } else {
        None
    }
}

/// A word SQLite reads as a string constant in a DEFAULT, but not a dynamic date/time keyword.
fn is_bare_word(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else { return false };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !["CURRENT_TIME", "CURRENT_DATE", "CURRENT_TIMESTAMP"].iter().any(|keyword| token.eq_ignore_ascii_case(keyword))
}

/// A string default read into a column of this affinity: a number becomes one for NUMERIC and
/// REAL, and otherwise the text stays text.
fn text_default(text: &str, affinity: Affinity) -> Option<CellValue> {
    if matches!(affinity, Affinity::Numeric | Affinity::Real)
        && let Some(number) = numeric_value(text)
    {
        return finite(number);
    }
    Some(text_value(text.as_bytes()))
}

/// A `-'…'` default: SQLite coerces the quoted text to an INTEGER when it is written as one,
/// otherwise to a REAL, negates it, and reads it with the column's affinity.
fn signed_quoted_default(text: &str, affinity: Affinity) -> Option<CellValue> {
    let text = text.trim_matches([' ', '\t', '\n', '\u{b}', '\u{c}', '\r']);
    // A separator inside quoted text is not the number the separator would join.
    if text.contains('_') {
        return None;
    }
    if let Ok(n) = text.parse::<i64>()
        && let Some(neg) = n.checked_neg()
    {
        return Some(integer_default(neg, affinity));
    }
    let real = text.parse::<f64>().ok().filter(|n| n.is_finite())?;
    let neg = -real;
    match affinity {
        Affinity::Text => Some(text_value(real_text(neg).as_bytes())),
        Affinity::Numeric | Affinity::Real => {
            let negated = if let Some(rest) = text.strip_prefix('-') { rest.to_string() } else { format!("-{}", text.strip_prefix('+').unwrap_or(text)) };
            numeric_value(&negated).and_then(finite)
        }
        Affinity::Blob => finite(CellValue::Real(neg)),
    }
}

/// A REAL as SQLite writes it into a TEXT column (`%!.15g`): 15 significant digits, a decimal
/// point always present, scientific notation outside 1e-4..1e15, and `Inf`/`-Inf` for infinity.
fn real_text(n: f64) -> String {
    if n.is_infinite() {
        return if n.is_sign_negative() { "-Inf".to_string() } else { "Inf".to_string() };
    }
    let scientific = format!("{n:.14e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    // SQLite writes both zeroes as "0.0"; keep the sign only for a non-zero value.
    let negative = mantissa.starts_with('-') && n != 0.0;
    let mut mantissa = mantissa.trim_start_matches('-').to_string();
    if mantissa.contains('.') {
        while mantissa.ends_with('0') {
            mantissa.pop();
        }
        if mantissa.ends_with('.') {
            mantissa.pop();
        }
    }
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let point = 1i32.saturating_add(exponent);
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    if (-4..15).contains(&exponent) {
        if point <= 0 {
            out.push_str("0.");
            for _ in 0..-point {
                out.push('0');
            }
            out.push_str(&digits);
        } else if point as usize >= digits.len() {
            out.push_str(&digits);
            for _ in 0..(point as usize - digits.len()) {
                out.push('0');
            }
        } else {
            let (whole, fraction) = digits.split_at(point as usize);
            out.push_str(whole);
            out.push('.');
            out.push_str(fraction);
        }
        if !out.contains('.') {
            out.push_str(".0");
        }
    } else {
        out.push_str(&mantissa);
        if !mantissa.contains('.') {
            out.push_str(".0");
        }
        out.push('e');
        out.push(if exponent < 0 { '-' } else { '+' });
        let abs = exponent.unsigned_abs();
        if abs < 10 {
            out.push('0');
        }
        out.push_str(&abs.to_string());
    }
    out
}

/// Remove a numeric literal's single `_` separators, rejecting any not between two digits.
fn unseparated_number(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut clean = String::with_capacity(bytes.len());
    let mut previous_was_digit = false;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'_' {
            if !previous_was_digit || !bytes.get(i.checked_add(1)?)?.is_ascii_digit() {
                return None;
            }
            previous_was_digit = false;
            continue;
        }
        clean.push(b as char);
        previous_was_digit = b.is_ascii_digit();
    }
    Some(clean)
}

/// An integer literal as a column of this affinity reads it: a TEXT column reads it as text.
fn integer_default(n: i64, affinity: Affinity) -> CellValue {
    if affinity == Affinity::Text { text_value(n.to_string().as_bytes()) } else { CellValue::Int(n) }
}

/// Whether `text` is a number as SQL writes one, sign included: `Some(true)` for an integer
/// (`-007`), `Some(false)` for a real (`1.5`, `.5`, `5.`, `1e3`), `None` for anything else.
/// Rust's float syntax is SQL's once the letters of `inf` and `nan` are ruled out.
fn number_kind(text: &str) -> Option<bool> {
    let numeric = text.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'));
    (numeric && text.parse::<f64>().is_ok()).then(|| !text.contains(['.', 'e', 'E']))
}

/// What a NUMERIC or REAL column makes of text that is a number, spaces around it allowed: an
/// INTEGER when its value is whole and fits in 64 bits, else a REAL. `None` when the text isn't
/// a number, so it stays text.
fn numeric_value(text: &str) -> Option<CellValue> {
    let number = text.trim_matches([' ', '\t', '\n', '\u{b}', '\u{c}', '\r']);
    let integer = number_kind(number)?;
    if integer && let Ok(n) = number.parse::<i64>() {
        return Some(CellValue::Int(n));
    }
    let real = number.parse::<f64>().ok()?;
    // SQLite keeps a whole REAL as an integer only strictly inside the 64-bit range.
    Some(if real.fract() == 0.0 && real > i64::MIN as f64 && real < i64::MAX as f64 { CellValue::Int(real as i64) } else { CellValue::Real(real) })
}

/// SQLite reads a number too large for a REAL as infinity, which isn't shown; such a default
/// stays unsupported.
fn finite(value: CellValue) -> Option<CellValue> {
    (!matches!(value, CellValue::Real(real) if !real.is_finite())).then_some(value)
}

/// A SQLite hex integer is 64-bit two's complement, so `0xffffffffffffffff` is -1. An optional
/// sign is accepted (`-0x10`), and a single underscore may separate two hex digits (`0xF_F`);
/// any other placement is rejected.
fn hex_integer(token: &str) -> Option<i64> {
    let (negative, unsigned) = match token.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, token.strip_prefix('+').unwrap_or(token)),
    };
    let digits = unsigned.strip_prefix("0x").or_else(|| unsigned.strip_prefix("0X"))?;
    if digits.is_empty() {
        return None;
    }
    let bytes = digits.as_bytes();
    let mut clean = String::with_capacity(bytes.len());
    let mut previous_was_digit = false;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'_' {
            if !previous_was_digit || !bytes.get(i.checked_add(1)?)?.is_ascii_hexdigit() {
                return None;
            }
            previous_was_digit = false;
            continue;
        }
        if !b.is_ascii_hexdigit() {
            return None;
        }
        clean.push(b as char);
        previous_was_digit = true;
    }
    let n = u64::from_str_radix(&clean, 16).ok()? as i64;
    if negative { n.checked_neg() } else { Some(n) }
}

/// A single literal wrapped in one pair of parentheses, such as `(5)` or `('text')`. A
/// parenthesized expression (`(5 + 1)`, `(lower('A'))`) stays unknown, as do nested parentheses.
fn parenthesized_literal(tokens: &[&str], affinity: Affinity) -> Option<CellValue> {
    let mut depth = 0usize;
    let mut close = None;
    for (i, token) in tokens.iter().enumerate() {
        match *token {
            "(" => depth = depth.saturating_add(1),
            ")" => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let inside = tokens.get(1..close?)?;
    match inside {
        [one] => literal_value(&[*one], affinity, false),
        [sign, _] if matches!(*sign, "+" | "-") || (*sign).eq_ignore_ascii_case("X") => literal_value(inside, affinity, false),
        _ => None,
    }
}

/// Split on top-level commas (ignoring commas inside parentheses, quotes and comments).
fn split_top_level(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for token in sql_tokens(s) {
        match token {
            "(" => depth = depth.saturating_add(1),
            ")" => depth = depth.saturating_sub(1),
            "," if depth == 0 => {
                let at = (token.as_ptr() as usize).saturating_sub(s.as_ptr() as usize);
                out.push(s.get(start..at).unwrap_or(""));
                start = at.saturating_add(1);
                if out.len() >= MAX_COLUMNS {
                    return out;
                }
            }
            _ => {}
        }
    }
    out.push(s.get(start..).unwrap_or(""));
    out
}

/// Take the leading column identifier (handling "quoted", `back-ticked`, 'single-quoted' and
/// [bracketed] names).
fn take_identifier(seg: &str) -> (String, &str) {
    let seg = seg.trim_start();
    let mut chars = seg.char_indices().peekable();
    match chars.next() {
        Some((_, q @ ('"' | '`' | '\''))) => {
            // Quoted: read to the closing quote. A doubled quote stands for one quote.
            let mut name = String::new();
            while let Some((i, c)) = chars.next() {
                if c != q {
                    name.push(c);
                } else if chars.next_if(|&(_, next)| next == q).is_some() {
                    name.push(q);
                } else {
                    return (name, seg.get(i.saturating_add(1)..).unwrap_or(""));
                }
            }
            (name, "")
        }
        Some((_, '[')) => {
            if let Some(close) = seg.find(']') {
                return (seg.get(1..close).unwrap_or("").to_string(), seg.get(close + 1..).unwrap_or(""));
            }
            (seg.get(1..).unwrap_or("").to_string(), "")
        }
        Some(_) => {
            let endp = seg.find([' ', '\t', '\n', '\r', '\u{c}', '(', ',']).unwrap_or(seg.len());
            // A comment may follow the name with no space between: `id/*key*/` or `id--key`.
            let endp = ["--", "/*"].into_iter().filter_map(|mark| seg.get(..endp)?.find(mark)).fold(endp, usize::min);
            (seg.get(..endp).unwrap_or("").to_string(), seg.get(endp..).unwrap_or(""))
        }
        None => (String::new(), ""),
    }
}

/// The type name is the run of tokens before the first column-constraint keyword. A comma inside
/// the type's parentheses belongs to it (`DECIMAL(10, 2)`); only the column separator is dropped.
fn extract_type(rest: &str) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    for tok in rest.split_whitespace() {
        let bare = tok.trim_end_matches(',');
        let upper = bare.to_ascii_uppercase();
        if depth == 0
            && matches!(
                upper.as_str(),
                "PRIMARY" | "NOT" | "NULL" | "UNIQUE" | "CHECK" | "DEFAULT" | "COLLATE" | "REFERENCES" | "GENERATED" | "AS" | "CONSTRAINT" | "AUTOINCREMENT"
            )
        {
            break;
        }
        depth += tok.matches('(').count() as i32;
        depth -= tok.matches(')').count() as i32;
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(if depth == 0 { bare } else { tok });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TEXT value cut inside a character drops the incomplete character, not a U+FFFD that
    /// wasn't in the database.
    #[test]
    fn a_truncated_text_value_ends_at_a_character_boundary() {
        let mut bytes = vec![b'a'; MAX_TEXT - 1];
        bytes.extend_from_slice("é".as_bytes());
        bytes.extend_from_slice(b"tail");
        let CellValue::Text { text, truncated } = encoded_text_value(&bytes, "utf-8") else {
            panic!("a TEXT value");
        };
        assert!(truncated);
        assert_eq!(text, "a".repeat(MAX_TEXT - 1));
        assert!(!text.contains(char::REPLACEMENT_CHARACTER));
    }

    /// The UTF-16 branch drops a split surrogate pair the same way.
    #[test]
    fn a_truncated_utf16_text_value_drops_a_split_surrogate() {
        let mut bytes = Vec::new();
        for _ in 0..(MAX_TEXT / 2 - 1) {
            bytes.extend_from_slice(&0x0061u16.to_le_bytes());
        }
        bytes.extend_from_slice(&0xD83Du16.to_le_bytes());
        bytes.extend_from_slice(&0xDE00u16.to_le_bytes());
        bytes.extend_from_slice(&0x0062u16.to_le_bytes());
        let CellValue::Text { text, truncated } = encoded_text_value(&bytes, "utf-16le") else {
            panic!("a TEXT value");
        };
        assert!(truncated);
        assert_eq!(text, "a".repeat(MAX_TEXT / 2 - 1));
        assert!(!text.contains(char::REPLACEMENT_CHARACTER));
    }

    /// Invalid bytes before the cut keep their lossy prefix; only an incomplete trailing character
    /// is dropped.
    #[test]
    fn an_invalid_utf8_prefix_is_not_over_trimmed() {
        let bytes = vec![0x80; MAX_TEXT + 1];
        let CellValue::Text { text, truncated } = encoded_text_value(&bytes, "utf-8") else {
            panic!("a TEXT value");
        };
        assert!(truncated);
        assert_eq!(text.chars().count(), MAX_TEXT);
        assert!(text.chars().all(|c| c == char::REPLACEMENT_CHARACTER));
    }

    /// A UTF-16 cut is not a UTF-8 cut: a byte in the continuation range after the cut must not
    /// shorten the take.
    #[test]
    fn a_utf16_cut_is_not_walked_back_as_utf8() {
        let mut bytes = Vec::new();
        for _ in 0..(MAX_TEXT / 2 - 1) {
            bytes.extend_from_slice(&0x0061u16.to_le_bytes());
        }
        bytes.extend_from_slice(&0x80E9u16.to_le_bytes());
        bytes.extend_from_slice(&0x0080u16.to_le_bytes());
        let CellValue::Text { text, truncated } = encoded_text_value(&bytes, "utf-16le") else {
            panic!("a TEXT value");
        };
        assert!(truncated);
        assert_eq!(text.chars().count(), MAX_TEXT / 2);
        assert_eq!(text.chars().last(), Some('\u{80E9}'));
    }

    /// Schema text is read whole only while the walk's budget lasts; after that, text is cut at
    /// the usual cap. Text shown to the reader is cut at the cap on a character boundary.
    #[test]
    fn whole_schema_text_stops_when_its_budget_is_spent() {
        let mut w = RowWalk::new(2);
        w.text_cap = MAX_SCHEMA_TEXT;
        w.text_budget = 10;
        w.keep(Row { rowid: Some(1), values: vec![text("1234"), CellValue::Int(5)] });
        assert_eq!((w.text_cap, w.text_budget), (MAX_SCHEMA_TEXT, 6));
        w.keep(Row { rowid: Some(2), values: vec![text("123456")] });
        assert_eq!((w.text_cap, w.text_budget), (MAX_TEXT, 0));
        assert_eq!(w.out.len(), 2);

        let mut shown = format!("{}é", "a".repeat(MAX_TEXT - 1));
        cut_at_max_text(&mut shown);
        assert_eq!(shown, "a".repeat(MAX_TEXT - 1));
    }

    #[test]
    fn create_table_column_named_key_is_kept() {
        let names = |sql: &str| parse_create_columns(sql).into_iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names("CREATE TABLE kv (key TEXT, value TEXT)"), ["key", "value"]);
        assert_eq!(names("CREATE TABLE kv (key TEXT, value TEXT, PRIMARY KEY(key))"), ["key", "value"]);
    }

    fn payload_cap_case(case: u8) {
        use craft_anything_testkit::sqlite::{Val, overflow_row_db};
        use sha2::Digest as _;
        let values = match case {
            1 => vec![Val::Text("x".repeat(8_000_000))],
            2 => vec![Val::Text("x".repeat(8_400_000))],
            3 => vec![Val::Blob(vec![0x5a; 8_400_000])],
            4 => vec![Val::Text("x".repeat(4_300_000)), Val::Blob(vec![0x5a; 4_300_000])],
            5 => vec![Val::Int(42), Val::Blob(vec![0x5a; 8_400_000]), Val::Int(73)],
            6 => vec![Val::Blob(vec![0x5a; 1_000_000])],
            _ => panic!("unknown payload-cap case"),
        };
        let bytes = overflow_row_db(&values);
        let db = SqliteDb::parse(&bytes).expect("fixture parses");
        let page = db.read_rows(2, 0, 1);
        assert_eq!(page.rows.len(), 1);
        let row = &page.rows[0];
        assert_eq!(row.values.len(), values.len(), "d{case}: every column is present");
        for (i, source) in values.iter().enumerate() {
            let value = serde_json::to_value(&row.values[i]).expect("serializes");
            match source {
                Val::Text(_) => assert_eq!(value, serde_json::json!({"text": "x".repeat(MAX_TEXT), "truncated": true}), "d{case}: text prefix"),
                Val::Blob(blob) => {
                    assert_eq!(value["blob_len"], blob.len(), "d{case}: true blob length");
                    if case == 6 {
                        let hash: String = sha2::Sha256::digest(blob).iter().take(8).map(|b| format!("{b:02x}")).collect();
                        assert_eq!(value, serde_json::json!({"blob_len": blob.len(), "sha256_prefix": hash}));
                    } else {
                        assert_eq!(value["truncated"], true, "d{case}: partial blob marked");
                        assert!(value.get("sha256_prefix").is_none_or(serde_json::Value::is_null), "d{case}: no partial hash");
                    }
                }
                Val::Int(n) if i == 0 => assert_eq!(value, *n),
                Val::Int(_) => assert_eq!(value, serde_json::json!({"not_read": true})),
                _ => panic!("unexpected payload-cap fixture value"),
            }
        }
        assert_eq!(page.notes.is_empty(), matches!(case, 1 | 6), "d{case}: cap disclosure");
    }

    #[test]
    fn payload_cap_d1() {
        payload_cap_case(1);
    }
    #[test]
    fn payload_cap_d2() {
        payload_cap_case(2);
    }
    #[test]
    fn payload_cap_d3() {
        payload_cap_case(3);
    }
    #[test]
    fn payload_cap_d4() {
        payload_cap_case(4);
    }
    #[test]
    fn payload_cap_d5() {
        payload_cap_case(5);
    }
    #[test]
    fn payload_cap_d6() {
        payload_cap_case(6);
    }

    #[test]
    fn payload_cap_d7_without_rowid() {
        use craft_anything_testkit::sqlite::{Val, without_rowid_db};
        let bytes = without_rowid_db(
            "CREATE TABLE t(v BLOB, k INT PRIMARY KEY, tail INT) WITHOUT ROWID",
            &[vec![Val::Int(42), Val::Blob(vec![0x5a; 8_400_000]), Val::Int(73)]],
        );
        let page = SqliteDb::parse(&bytes).expect("fixture").read_rows(2, 0, 10);
        assert_eq!(page.rows.len(), 1, "d7: capped index rows must remain present");
        assert_eq!(page.rows[0].rowid, None);
        assert_eq!(
            serde_json::to_value(&page.rows[0].values).unwrap(),
            serde_json::json!([
                {"blob_len": 8_400_000, "truncated": true}, 42, {"not_read": true}
            ])
        );
        assert!(!page.notes.is_empty(), "d7: cap disclosure");
    }

    fn short_record_case(declarations: &str, defaults: Vec<CellValue>, note: bool) {
        use craft_anything_testkit::sqlite::{Val, table_db};
        let mut full = vec![Val::Int(2)];
        full.extend(defaults.iter().map(|v| match v {
            CellValue::Int(n) => Val::Int(*n),
            CellValue::Real(n) => Val::Real(*n),
            CellValue::Text { text, .. } => Val::Text(text.clone()),
            CellValue::Blob { .. } => Val::Blob(vec![0, 255]),
            CellValue::Null => Val::Null,
            _ => panic!("unexpected default fixture value"),
        }));
        let bytes = table_db(&format!("CREATE TABLE t(n INT, {declarations})"), &[(1, vec![Val::Int(1)]), (2, full)]);
        let db = SqliteDb::parse(&bytes).expect("fixture");
        let page = db.read_rows(2, 0, 10);
        let mut expected = vec![CellValue::Int(1)];
        expected.extend(defaults.clone());
        assert_eq!(page.rows[0].values, expected, "old record must include trailing defaults");
        expected[0] = CellValue::Int(2);
        assert_eq!(page.rows[1].values, expected, "full record must stay unchanged");
        assert_eq!(!page.notes.is_empty(), note, "unsupported defaults must be disclosed");
    }

    #[test]
    fn added_columns_c1_integer() {
        short_record_case("x INT DEFAULT 42", vec![CellValue::Int(42)], false);
    }
    #[test]
    fn added_columns_c2_null() {
        short_record_case("x INT", vec![CellValue::Null], false);
    }
    #[test]
    fn added_columns_c3_real_text() {
        short_record_case("x REAL DEFAULT -1.5, y TEXT DEFAULT 'it''s'", vec![CellValue::Real(-1.5), text_value(b"it's")], false);
    }
    #[test]
    fn added_columns_c4_blob() {
        short_record_case("x BLOB DEFAULT X'00ff'", vec![blob_value(&[0, 255], 2)], false);
    }
    #[test]
    fn added_columns_c7_expression() {
        short_record_case("d TEXT DEFAULT (lower('A'))", vec![CellValue::Null], true);
    }
    #[test]
    fn added_columns_c5_rewritten_control() {
        use craft_anything_testkit::sqlite::{Val, table_db};
        let bytes = table_db("CREATE TABLE t(n INT, x INT DEFAULT 42)", &[(1, vec![Val::Int(1), Val::Int(42)])]);
        let page = SqliteDb::parse(&bytes).expect("fixture").read_rows(2, 0, 10);
        assert_eq!(page.rows[0].values, vec![CellValue::Int(1), CellValue::Int(42)]);
        assert!(page.notes.is_empty());
    }

    #[test]
    fn added_columns_literal_forms_and_unknowns() {
        for (declaration, value) in [
            ("x INT DEFAULT +42 NOT NULL", CellValue::Int(42)),
            ("x INT DEFAULT - 42", CellValue::Int(-42)),
            ("x REAL DEFAULT + 1.25e-2", CellValue::Real(0.0125)),
            ("x INT DEFAULT true", CellValue::Int(1)),
            ("x INT DEFAULT FALSE", CellValue::Int(0)),
            ("x DEFAULT null", CellValue::Null),
            ("x TEXT DEFAULT 'a),b''c'", text_value(b"a),b'c")),
            ("x INT CHECK(x <> 'DEFAULT') DEFAULT /* literal */ 7", CellValue::Int(7)),
            ("x INT DEFAULT 0x10 NOT NULL", CellValue::Int(16)),
            ("x INT DEFAULT -0x10", CellValue::Int(-16)),
            ("x INT DEFAULT 0xF_F", CellValue::Int(255)),
            ("x INT DEFAULT -0xF_F", CellValue::Int(-255)),
            ("x INT DEFAULT 0xFFFFFFFFFFFFFFFF", CellValue::Int(-1)),
            ("x DEFAULT (NULL)", CellValue::Null),
            ("x INT DEFAULT (42)", CellValue::Int(42)),
            ("x INT DEFAULT (5) NOT NULL", CellValue::Int(5)),
            ("x INT DEFAULT (-2)", CellValue::Int(-2)),
            ("x TEXT DEFAULT ('hi')", text_value(b"hi")),
            ("x BLOB DEFAULT (X'00ff')", blob_value(&[0, 255], 2)),
        ] {
            short_record_case(declaration, vec![value], false);
        }
        for declaration in [
            "x TEXT DEFAULT CURRENT_TIMESTAMP",
            "x DEFAULT (lower('A'))",
            "x DEFAULT (5+1)",
            "x DEFAULT ((5))",
            "x INT DEFAULT 0x",
            "x INT DEFAULT 0x10000000000000000",
            "x INT DEFAULT 0x_F",
            "x INT DEFAULT 0xF_",
            "x INT DEFAULT 0xF__F",
            "x INT DEFAULT 0xF_G",
            "x DEFAULT X'zz'",
        ] {
            short_record_case(declaration, vec![CellValue::Null], true);
        }
        assert_eq!(blob_value(&[0, 255], 2), CellValue::Blob { len: 2, sha256_prefix: "06eb7d6a69ee19e5".into() });
        assert_eq!(literal_default(&sql_tokens("x''"), Affinity::Blob), Some(blob_value(&[], 0)));
    }

    #[test]
    fn added_columns_preserve_stored_nulls_and_long_records() {
        use craft_anything_testkit::sqlite::{Val, table_db};
        let bytes = table_db("CREATE TABLE t(n INT, x INT DEFAULT 42)", &[(1, vec![Val::Int(1), Val::Null]), (2, vec![Val::Int(2), Val::Int(7), Val::Int(9)])]);
        let page = SqliteDb::parse(&bytes).expect("fixture").read_rows(2, 0, 10);
        assert_eq!(page.rows[0].values, vec![CellValue::Int(1), CellValue::Null]);
        assert_eq!(page.rows[1].values, vec![CellValue::Int(2), CellValue::Int(7), CellValue::Int(9)]);
        assert!(page.notes.is_empty());
    }

    #[test]
    fn added_columns_c6_short_rowid_alias() {
        use craft_anything_testkit::sqlite::{Val, table_db};
        for stored in [vec![Val::Null, Val::Int(10)], vec![]] {
            let bytes = table_db("CREATE TABLE t(id INTEGER PRIMARY KEY, n INT DEFAULT 10, x INT DEFAULT 42)", &[(7, stored)]);
            let page = SqliteDb::parse(&bytes).expect("fixture").read_rows(2, 0, 10);
            assert_eq!(page.rows[0].rowid, Some(7));
            assert_eq!(page.rows[0].values, vec![CellValue::Int(7), CellValue::Int(10), CellValue::Int(42)]);
            assert!(page.notes.is_empty());
        }
    }

    #[test]
    fn version_formatting() {
        assert_eq!(version_string(3_045_001), "3.45.1");
        assert_eq!(version_string(3_008_000), "3.8.0");
    }

    #[test]
    fn varint_round_trip() {
        assert_eq!(varint(&[0x00], 0), Some((0, 1)));
        assert_eq!(varint(&[0x7f], 0), Some((127, 1)));
        assert_eq!(varint(&[0x81, 0x00], 0), Some((128, 2)));
        assert_eq!(varint(&[0xff; 9], 0).map(|(_, n)| n), Some(9));
        assert_eq!(varint(&[0x80], 0), None); // never terminates within range
    }

    #[test]
    fn column_parsing() {
        let cols = parse_create_columns("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL, bio, price REAL DEFAULT 0)");
        assert_eq!(cols.len(), 4);
        assert_eq!(cols[0].name, "id");
        assert_eq!(cols[0].type_name, "INTEGER");
        assert_eq!(cols[1].name, "name");
        assert_eq!(cols[1].type_name, "TEXT");
        assert_eq!(cols[2].name, "bio");
        assert_eq!(cols[2].type_name, "");
        assert_eq!(cols[3].type_name, "REAL");
    }

    #[test]
    fn column_parsing_quoted_and_constraints() {
        let cols = parse_create_columns("CREATE TABLE \"my tbl\" (\"first name\" VARCHAR(50), [odd] NUMERIC, PRIMARY KEY(\"first name\"))");
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].name, "first name");
        assert_eq!(cols[0].type_name, "VARCHAR(50)");
        assert_eq!(cols[1].name, "odd");
        assert_eq!(cols[1].type_name, "NUMERIC");
    }

    #[test]
    fn column_parsing_keeps_type_commas() {
        let cols = parse_create_columns(
            "CREATE TABLE t (amount DECIMAL(10, 2) NOT NULL, ratio NUMERIC(5,1), note TEXT DEFAULT 'a,b', id INTEGER PRIMARY KEY, name VARCHAR(50))",
        );
        let types: Vec<&str> = cols.iter().map(|c| c.type_name.as_str()).collect();
        assert_eq!(types, ["DECIMAL(10, 2)", "NUMERIC(5,1)", "TEXT", "INTEGER", "VARCHAR(50)"]);
    }

    #[test]
    fn column_parsing_crlf_constraints() {
        for kw in ["UNIQUE", "CHECK", "PRIMARY", "FOREIGN", "CONSTRAINT"] {
            let sql = format!("CREATE TABLE t (a INT,\r\n {kw}\r\n (a))");
            let names: Vec<String> = parse_create_columns(&sql).into_iter().map(|c| c.name).collect();
            assert_eq!(names, vec!["a".to_string()], "{kw}");
        }
    }

    #[test]
    fn rejects_non_sqlite() {
        assert!(SqliteDb::parse(b"not a database").is_err());
        assert!(SqliteDb::parse(&[]).is_err());
        assert!(!is_sqlite(b"SQLite format 2\0"));
    }

    #[test]
    fn reads_fixture_overview_schema_and_rows() {
        let db = craft_anything_testkit::sqlite::simple_db();
        let parsed = SqliteDb::parse(&db).expect("fixture parses");
        let ov = parsed.overview();
        assert_eq!(ov.header.page_size, 4096);
        assert_eq!(ov.header.text_encoding, "utf-8");
        assert_eq!(ov.header.sqlite_version, "3.45.1");
        assert_eq!(ov.page_count_from_size, 2);
        assert!(!ov.header.wal_mode);
        assert_eq!(ov.schema.len(), 1);
        let items = &ov.schema[0];
        assert_eq!(items.obj_type, "table");
        assert_eq!(items.name, "items");
        assert_eq!(items.rootpage, 2);
        assert_eq!(items.columns.len(), 4);
        assert_eq!(items.columns[0].name, "id");
        assert_eq!(items.columns[3].type_name, "BLOB");

        let count = parsed.count_rows(items.rootpage);
        assert_eq!(count.count, 3);
        assert!(!count.bounded);

        let page = parsed.read_rows(items.rootpage, 0, 10);
        assert_eq!(page.rows.len(), 3);
        assert_eq!(page.rows[0].rowid, Some(1));
        assert_eq!(page.rows[0].values[0], CellValue::Int(1));
        assert_eq!(page.rows[0].values[1], CellValue::Text { text: "alpha".into(), truncated: false });
        assert_eq!(page.rows[0].values[2], CellValue::Real(1.5));
        match &page.rows[0].values[3] {
            CellValue::Blob { len, .. } => assert_eq!(*len, 4),
            other => panic!("expected blob, got {other:?}"),
        }
        assert_eq!(page.rows[1].values[3], CellValue::Null);

        // Paging: offset 1, limit 1 yields the second row only.
        let mid = parsed.read_rows(items.rootpage, 1, 1);
        assert_eq!(mid.rows.len(), 1);
        assert_eq!(mid.rows[0].rowid, Some(2));

        // find_object is case-insensitive.
        assert!(parsed.find_object("ITEMS").is_some());
        assert!(parsed.find_object("nope").is_none());
    }

    #[test]
    fn wal_mode_is_reported() {
        let db = craft_anything_testkit::sqlite::wal_mode_db();
        let parsed = SqliteDb::parse(&db).expect("parses");
        assert!(parsed.header().wal_mode);
        assert!(parsed.overview().integrity_notes.iter().any(|n| n.contains("WAL")));
    }

    fn check_rowid_alias(declaration: &str, column: usize, alias: bool) {
        use craft_anything_testkit::sqlite::{Val, table_db};
        let sql = format!("CREATE TABLE t({declaration})");
        let columns = parse_create_columns(&sql);
        let rows: Vec<_> = [-3, 7]
            .into_iter()
            .map(|rowid| {
                let mut values = vec![Val::Text("x".into()); columns.len()];
                values[column] = Val::Null;
                (rowid, values)
            })
            .collect();
        let bytes = table_db(&sql, &rows);
        let db = SqliteDb::parse(&bytes).expect("fixture parses");
        let page = db.read_rows(2, 0, 10);
        assert_eq!(page.rows.len(), 2);
        for (row, (rowid, _)) in page.rows.iter().zip(&rows) {
            assert_eq!(row.rowid, Some(*rowid));
            let mut expected = vec![CellValue::Text { text: "x".into(), truncated: false }; columns.len()];
            expected[column] = if alias { CellValue::Int(*rowid) } else { CellValue::Null };
            assert_eq!(row.values, expected, "{declaration}, rowid {rowid}");
        }
        // Even an alias must preserve a non-NULL stored value.
        let mut values = vec![Val::Null; columns.len()];
        values[column] = Val::Int(99);
        let bytes = table_db(&sql, &[(7, values)]);
        let db = SqliteDb::parse(&bytes).expect("fixture parses");
        assert_eq!(db.read_rows(2, 0, 1).rows[0].values[column], CellValue::Int(99));
    }

    #[test]
    fn integer_primary_key_aliases_show_the_rowid() {
        // (declaration, column, is an alias): the rule from SQLite's CREATE TABLE docs.
        for (declaration, column, alias) in [
            ("id INTEGER PRIMARY KEY, v TEXT", 0, true),
            ("id integer primary key, v text", 0, true),
            ("id INTEGER, v TEXT, PRIMARY KEY(id)", 0, true),
            ("id INTEGER PRIMARY KEY DESC, v TEXT", 0, false),
            ("id INT PRIMARY KEY, v TEXT", 0, false),
            ("v TEXT, id INTEGER PRIMARY KEY, w TEXT", 1, true),
            ("id INTEGER PRIMARY KEY AUTOINCREMENT, v TEXT", 0, true),
            ("a INTEGER, b INTEGER, PRIMARY KEY(a, b)", 0, false),
            ("id INTEGER PRIMARY KEY ASC, v TEXT", 0, true),
            ("id INTEGER NOT NULL PRIMARY KEY, v TEXT", 0, true),
            ("id BIGINT PRIMARY KEY, v TEXT", 0, false),
            ("id INTEGER, v TEXT, CONSTRAINT pk PRIMARY KEY(id DESC)", 0, true),
        ] {
            check_rowid_alias(declaration, column, alias);
        }
    }

    #[test]
    fn a_without_rowid_table_has_no_rowid_alias() {
        // Read through the index b-tree (database_tools `without_rowid_b4_integer_key` covers the rows).
        assert_eq!(rowid_alias_index("CREATE TABLE t(id INTEGER PRIMARY KEY) WITHOUT ROWID"), None);
        assert_eq!(rowid_alias_index("CREATE TABLE t(id INTEGER PRIMARY KEY)"), Some(0));
    }

    #[test]
    fn truncation_never_panics() {
        let db = craft_anything_testkit::sqlite::simple_db();
        for cut in 0..db.len() {
            let slice = &db[..cut];
            if let Ok(parsed) = SqliteDb::parse(slice) {
                let ov = parsed.overview();
                for obj in &ov.schema {
                    let _ = parsed.count_rows(obj.rootpage);
                    let _ = parsed.read_rows(obj.rootpage, 0, 50);
                }
                // Also walk pages that may no longer exist.
                let _ = parsed.count_rows(2);
                let _ = parsed.read_rows(2, 0, 50);
            }
        }
    }

    #[test]
    fn mutation_fuzz_never_panics() {
        let base = craft_anything_testkit::sqlite::simple_db();
        // Deterministic LCG so the test is reproducible.
        let mut state: u64 = 0x1234_5678_9abc_def0;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            state
        };
        for _ in 0..4000 {
            let mut db = base.clone();
            let flips = (next() % 8) as usize + 1;
            for _ in 0..flips {
                let idx = (next() as usize) % db.len();
                db[idx] = (next() & 0xff) as u8;
            }
            if let Ok(parsed) = SqliteDb::parse(&db) {
                let ov = parsed.overview();
                for obj in ov.schema.iter().take(16) {
                    let _ = parsed.count_rows(obj.rootpage);
                    let _ = parsed.read_rows(obj.rootpage, 0, 100);
                }
                let _ = parsed.read_rows(2, 0, 100);
            }
        }
    }

    #[test]
    fn wide_btree_visits_keep_every_row_in_key_order() {
        use craft_anything_testkit::sqlite::{Val, without_rowid_db};
        let rows = (0..1023).map(|i| vec![Val::Int(i), Val::Text(format!("value {i}"))]).collect::<Vec<_>>();
        let bytes = without_rowid_db("CREATE TABLE t(k INT PRIMARY KEY, v TEXT) WITHOUT ROWID", &rows);
        let db = SqliteDb::parse(&bytes).expect("fixture parses");
        let count = db.count_rows(2);
        assert_eq!(count.count, 1023);
        assert!(!count.bounded);
        let page = db.read_rows(2, 0, 1024);
        assert_eq!(page.rows.len(), 1023);
        assert!(!page.truncated);
        assert!(page.notes.is_empty());
        for (i, row) in page.rows.iter().enumerate() {
            assert_eq!(row.rowid, None);
            assert_eq!(row.values, vec![CellValue::Int(i as i64), CellValue::Text { text: format!("value {i}"), truncated: false }]);
        }
    }

    #[test]
    fn repeated_btree_children_and_cycles_do_not_repeat_rows() {
        use craft_anything_testkit::sqlite::{PAGE_SIZE, Val, without_rowid_db};
        let rows = (0..15).map(|i| vec![Val::Int(i)]).collect::<Vec<_>>();
        let base = without_rowid_db("CREATE TABLE t(k INT PRIMARY KEY) WITHOUT ROWID", &rows);
        let cell = usize::from(u16::from_be_bytes([base[PAGE_SIZE + 12], base[PAGE_SIZE + 13]]));
        let first_child = u32::from_be_bytes(base[PAGE_SIZE + cell..PAGE_SIZE + cell + 4].try_into().unwrap());
        for right in [2, first_child] {
            let mut bytes = base.clone();
            bytes[PAGE_SIZE + 8..PAGE_SIZE + 12].copy_from_slice(&right.to_be_bytes());
            let db = SqliteDb::parse(&bytes).expect("fixture parses");
            assert_eq!(db.count_rows(2).count, 12);
            let page = db.read_rows(2, 0, 16);
            assert_eq!(page.rows.len(), 12);
            for (i, row) in page.rows.iter().enumerate() {
                assert_eq!(row.values, vec![CellValue::Int(i as i64)]);
            }
        }
    }

    #[test]
    fn without_rowid_damaged_pages_never_panic() {
        use craft_anything_testkit::sqlite::{Val, without_rowid_db};
        let rows = (0..9).map(|i| vec![Val::Int(i), Val::Text("x".repeat(2000))]).collect::<Vec<_>>();
        let base = without_rowid_db("CREATE TABLE t(k INT PRIMARY KEY, v TEXT) WITHOUT ROWID", &rows);
        for cut in (100..base.len()).step_by(31) {
            let db = SqliteDb::parse(&base[..cut]).unwrap();
            let _ = db.count_rows(2);
            let _ = db.read_rows(2, 1, 20);
        }
        for off in 4096..4120 {
            for value in [0, 2, 10, 255] {
                let mut bytes = base.clone();
                bytes[off] = value;
                let db = SqliteDb::parse(&bytes).unwrap();
                let _ = db.count_rows(2);
                let _ = db.read_rows(2, 0, 20);
            }
        }
    }

    #[test]
    fn decode_serial_types() {
        assert_eq!(decode_serial(0, &[], 0, "utf-8", MAX_TEXT).0, CellValue::Null);
        assert_eq!(decode_serial(8, &[], 0, "utf-8", MAX_TEXT).0, CellValue::Int(0));
        assert_eq!(decode_serial(9, &[], 0, "utf-8", MAX_TEXT).0, CellValue::Int(1));
        assert_eq!(decode_serial(1, &[0xff], 0, "utf-8", MAX_TEXT).0, CellValue::Int(-1));
        assert_eq!(decode_serial(2, &[0x01, 0x00], 0, "utf-8", MAX_TEXT).0, CellValue::Int(256));
        // text: serial 13 -> (13-13)/2 = 0 bytes; serial 19 -> 3 bytes
        assert_eq!(decode_serial(19, b"abc", 0, "utf-8", MAX_TEXT), (CellValue::Text { text: "abc".into(), truncated: false }, 3));
        // blob: serial 18 -> (18-12)/2 = 3 bytes
        match decode_serial(18, b"xyz", 0, "utf-8", MAX_TEXT).0 {
            CellValue::Blob { len, .. } => assert_eq!(len, 3),
            other => panic!("expected blob, got {other:?}"),
        }
    }

    fn text(value: &str) -> CellValue {
        CellValue::Text { text: value.into(), truncated: false }
    }

    /// A VIRTUAL generated column has no field in the record (SQLite computes it when a row is
    /// read), so the fields after it belong to the next stored columns; a STORED one keeps its
    /// field. Each VIRTUAL column is shown as not read and named in a note.
    #[test]
    fn virtual_generated_columns_take_no_record_field() {
        use craft_anything_testkit::sqlite::{Val, table_db};
        let sql = "CREATE TABLE t(a TEXT, b TEXT GENERATED ALWAYS AS (upper(a)) VIRTUAL, c INTEGER, \
                   s INT AS (c * 2) STORED, d TEXT, e AS (a || d))";
        let record = vec![Val::Text("x".into()), Val::Int(7), Val::Int(14), Val::Text("dee".into())];
        let bytes = table_db(sql, &[(1, record)]);
        let page = SqliteDb::parse(&bytes).expect("fixture parses").read_rows(2, 0, 10);
        assert_eq!(page.rows[0].values, vec![text("x"), CellValue::NotRead, CellValue::Int(7), CellValue::Int(14), text("dee"), CellValue::NotRead]);
        let notes: Vec<_> = page.notes.iter().filter(|note| note.contains("VIRTUAL generated column")).collect();
        assert_eq!(notes.len(), 2, "{:?}", page.notes);
        assert!(notes[0].starts_with("Column 2 ") && notes[1].starts_with("Column 6 "), "{notes:?}");
    }

    /// The rowid alias and the defaults of columns added later still land on their own columns
    /// when a VIRTUAL column comes before them.
    #[test]
    fn virtual_generated_columns_keep_aliases_and_added_defaults_in_place() {
        use craft_anything_testkit::sqlite::{Val, table_db};
        let bytes = table_db("CREATE TABLE t(v INT AS (1), id INTEGER PRIMARY KEY, x TEXT)", &[(10, vec![Val::Null, Val::Text("ten".into())])]);
        let page = SqliteDb::parse(&bytes).expect("fixture parses").read_rows(2, 0, 10);
        assert_eq!(page.rows[0].values, vec![CellValue::NotRead, CellValue::Int(10), text("ten")]);

        // ALTER TABLE ADD COLUMN g AS (...), then ADD COLUMN c DEFAULT 3: an old record holds only `a`.
        let sql = "CREATE TABLE t(a TEXT, g TEXT AS (a || '!'), c INTEGER DEFAULT 3)";
        let bytes = table_db(sql, &[(1, vec![Val::Text("old".into())]), (2, vec![Val::Text("new".into()), Val::Int(9)])]);
        let page = SqliteDb::parse(&bytes).expect("fixture parses").read_rows(2, 0, 10);
        assert_eq!(page.rows[0].values, vec![text("old"), CellValue::NotRead, CellValue::Int(3)]);
        assert_eq!(page.rows[1].values, vec![text("new"), CellValue::NotRead, CellValue::Int(9)]);
    }

    /// A WITHOUT ROWID record stores the key, then the other stored columns; VIRTUAL columns
    /// are not among them.
    #[test]
    fn without_rowid_virtual_generated_columns_take_no_record_field() {
        use craft_anything_testkit::sqlite::{Val, without_rowid_db};
        let bytes =
            without_rowid_db("CREATE TABLE t(k TEXT PRIMARY KEY, b TEXT AS (upper(k)), c INTEGER) WITHOUT ROWID", &[vec![Val::Text("x".into()), Val::Int(7)]]);
        let page = SqliteDb::parse(&bytes).expect("fixture parses").read_rows(2, 0, 10);
        assert_eq!(page.rows[0].values, vec![text("x"), CellValue::NotRead, CellValue::Int(7)]);

        let sql = "CREATE TABLE t(k TEXT, n INT, g TEXT AS (k || n), PRIMARY KEY (n, k)) WITHOUT ROWID";
        let bytes = without_rowid_db(sql, &[vec![Val::Int(1), Val::Text("a".into())]]);
        let page = SqliteDb::parse(&bytes).expect("fixture parses").read_rows(2, 0, 10);
        assert_eq!(page.rows[0].values, vec![text("a"), CellValue::Int(1), CellValue::NotRead]);
    }

    /// Only a top-level `AS` makes a generated column, and `STORED` right after its expression
    /// makes it a stored one.
    #[test]
    fn only_a_top_level_as_declares_a_virtual_column() {
        let virtual_of = |sql: &str| column_definitions(sql).iter().map(|(_, rest)| is_virtual_column(rest)).collect::<Vec<_>>();
        assert_eq!(
            virtual_of(
                "CREATE TABLE t(a INT DEFAULT (CAST('1' AS INT)), b TEXT CHECK (CAST(b AS TEXT) <> ''), \
                 c AS (a), d INT GENERATED ALWAYS AS ((a + 1) * 2) VIRTUAL, e GENERATED ALWAYS AS (a) STORED NOT NULL, \
                 f TEXT AS (printf('%d', a)) NOT NULL, g as (a) stored)"
            ),
            vec![false, false, true, true, false, true, false]
        );
    }

    /// SQLite stores a REAL with no fractional part as an integer and reads it back as a REAL.
    /// The declared type is checked in SQLite's order, so `FLOATING POINT` (which contains INT)
    /// is an INTEGER column, and NUMERIC and untyped columns keep their integers too.
    #[test]
    fn whole_numbers_in_real_columns_read_as_real() {
        use craft_anything_testkit::sqlite::{Val, table_db};
        let sql = "CREATE TABLE t(a REAL, b FLOAT, c DOUBLE PRECISION, d \"real\" NOT NULL, e FLOATING POINT, f NUMERIC, g, h REAL, i REAL)";
        let record = vec![
            Val::Int(1),
            Val::Int(-2),
            Val::Int(0),
            Val::Int(140_737_488_355_327),
            Val::Int(5),
            Val::Int(6),
            Val::Int(7),
            Val::Real(1.5),
            Val::Text("x".into()),
        ];
        let bytes = table_db(sql, &[(1, record)]);
        let page = SqliteDb::parse(&bytes).expect("fixture parses").read_rows(2, 0, 10);
        assert_eq!(
            page.rows[0].values,
            vec![
                CellValue::Real(1.0),
                CellValue::Real(-2.0),
                CellValue::Real(0.0),
                CellValue::Real(140_737_488_355_327.0),
                CellValue::Int(5),
                CellValue::Int(6),
                CellValue::Int(7),
                CellValue::Real(1.5),
                text("x"),
            ]
        );
    }

    /// The same holds for a WITHOUT ROWID key and for the default of a REAL column added later.
    #[test]
    fn real_keys_and_added_real_defaults_read_as_real() {
        use craft_anything_testkit::sqlite::{Val, table_db, without_rowid_db};
        let sql = "CREATE TABLE t(k REAL, n INT, v DOUBLE, PRIMARY KEY (k, n)) WITHOUT ROWID";
        let bytes = without_rowid_db(sql, &[vec![Val::Int(3), Val::Int(1), Val::Int(4)]]);
        let page = SqliteDb::parse(&bytes).expect("fixture parses").read_rows(2, 0, 10);
        assert_eq!(page.rows[0].values, vec![CellValue::Real(3.0), CellValue::Int(1), CellValue::Real(4.0)]);

        // ALTER TABLE ADD COLUMN r REAL DEFAULT 5: the old record holds only `a`.
        let bytes =
            table_db("CREATE TABLE t(a TEXT, r REAL DEFAULT 5)", &[(1, vec![Val::Text("old".into())]), (2, vec![Val::Text("new".into()), Val::Int(6)])]);
        let page = SqliteDb::parse(&bytes).expect("fixture parses").read_rows(2, 0, 10);
        assert_eq!(page.rows[0].values, vec![text("old"), CellValue::Real(5.0)]);
        assert_eq!(page.rows[1].values, vec![text("new"), CellValue::Real(6.0)]);
    }

    /// SQLite reads a default filled into an older row with its column's affinity. Each value
    /// here is what SQLite 3.51 reads for that column after ALTER TABLE ADD COLUMN.
    #[test]
    fn added_column_defaults_take_the_column_affinity() {
        for (declaration, value) in [
            // TEXT: an integer written plainly, otherwise SQLite's own REAL text.
            ("x TEXT DEFAULT 5", text("5")),
            ("x VARCHAR(10) DEFAULT -5.0", text("-5.0")),
            ("x TEXT DEFAULT 007", text("7")),
            ("x TEXT DEFAULT - 007", text("-7")),
            ("x CLOB DEFAULT +1e3", text("1000.0")),
            ("x TEXT DEFAULT 5.", text("5.0")),
            ("x TEXT DEFAULT 0.10", text("0.1")),
            ("x TEXT DEFAULT 1.50", text("1.5")),
            ("x TEXT DEFAULT 0001.5", text("1.5")),
            ("x TEXT DEFAULT 1e-7", text("1.0e-07")),
            ("x TEXT DEFAULT 1e16", text("1.0e+16")),
            ("x TEXT DEFAULT -0.0", text("0.0")),
            ("x TEXT DEFAULT 99999999999999999999", text("1.0e+20")),
            ("x TEXT DEFAULT -9223372036854775808", text("-9223372036854775808")),
            ("x TEXT DEFAULT 9223372036854775808", text("9.22337203685478e+18")),
            ("x TEXT DEFAULT -9223372036854775809", text("-9.22337203685478e+18")),
            ("x TEXT DEFAULT 1e400", text("Inf")),
            ("x TEXT DEFAULT -1e400", text("-Inf")),
            ("x TEXT DEFAULT TRUE", text("1")),
            // INTEGER and NUMERIC: text that is a number becomes one; a whole REAL an INTEGER.
            ("x INTEGER DEFAULT '7'", CellValue::Int(7)),
            ("x INT DEFAULT ' \t7\n'", CellValue::Int(7)),
            ("x INTEGER DEFAULT 5.0", CellValue::Int(5)),
            ("x BIGINT DEFAULT -0.0", CellValue::Int(0)),
            ("x INTEGER DEFAULT '1.5'", CellValue::Real(1.5)),
            ("x NUMERIC DEFAULT '+.5e-3'", CellValue::Real(0.0005)),
            ("x NUMERIC DEFAULT 1e3", CellValue::Int(1000)),
            ("x DECIMAL(10,2) DEFAULT '-9223372036854775808'", CellValue::Int(i64::MIN)),
            ("x NUMERIC DEFAULT '9223372036854775808'", CellValue::Real(9_223_372_036_854_775_808.0)),
            ("x NUMERIC DEFAULT 9.3e18", CellValue::Real(9.3e18)),
            ("x BOOLEAN DEFAULT 'abc'", text("abc")),
            ("x NUMERIC DEFAULT '0x10'", text("0x10")),
            ("x NUMERIC DEFAULT '1e'", text("1e")),
            ("x NUMERIC DEFAULT ' '", text(" ")),
            // REAL: the number, then as a REAL.
            ("x REAL DEFAULT '5'", CellValue::Real(5.0)),
            ("x DOUBLE DEFAULT ' -1.5 '", CellValue::Real(-1.5)),
            ("x FLOAT DEFAULT 'abc'", text("abc")),
            // BLOB and no type: the literal as written.
            ("x BLOB DEFAULT '5'", text("5")),
            ("x DEFAULT '5'", text("5")),
            ("x DEFAULT 5.0", CellValue::Real(5.0)),
            ("x BLOB DEFAULT 1e3", CellValue::Real(1000.0)),
            ("x DEFAULT -7", CellValue::Int(-7)),
        ] {
            short_record_case(declaration, vec![value], false);
        }
        // SQLite reads a number too large for a REAL as infinity; that stays unsupported.
        short_record_case("x INTEGER DEFAULT '1e400'", vec![CellValue::Null], true);
    }

    /// Bare and double-quoted words, signed strings and digit separators take the column's
    /// affinity too (SQLite 3.51 readings).
    #[test]
    fn default_words_and_separators_take_the_column_affinity() {
        for (declaration, value) in [
            ("x TEXT DEFAULT abc", text("abc")),
            ("x TEXT DEFAULT \"abc\"", text("abc")),
            ("x TEXT DEFAULT \"a\"\"b\"", text("a\"b")),
            ("x TEXT DEFAULT _1", text("_1")),
            ("x TEXT DEFAULT X", text("X")),
            ("x INTEGER DEFAULT \"5\"", CellValue::Int(5)),
            ("x REAL DEFAULT \"5\"", CellValue::Real(5.0)),
            ("x BLOB DEFAULT \"5\"", text("5")),
            ("x DEFAULT \"5\"", text("5")),
            ("x TEXT DEFAULT +'5'", text("5")),
            ("x INTEGER DEFAULT -'5'", CellValue::Int(-5)),
            ("x REAL DEFAULT -'5'", CellValue::Real(-5.0)),
            ("x BLOB DEFAULT -'5'", CellValue::Int(-5)),
            ("x TEXT DEFAULT -'5'", text("-5")),
            ("x TEXT DEFAULT -'1e3'", text("-1000.0")),
            ("x TEXT DEFAULT -'1.50'", text("-1.5")),
            ("x TEXT DEFAULT -'1.0'", text("-1.0")),
            ("x TEXT DEFAULT -'1000'", text("-1000")),
            ("x TEXT DEFAULT -' 1e3 '", text("-1000.0")),
            ("x TEXT DEFAULT -' -5 '", text("5")),
            ("x INTEGER DEFAULT -'1e3'", CellValue::Int(-1000)),
            ("x INTEGER DEFAULT -'1.50'", CellValue::Real(-1.5)),
            ("x REAL DEFAULT -'1e3'", CellValue::Real(-1000.0)),
            ("x BLOB DEFAULT -'1e3'", CellValue::Real(-1000.0)),
            ("x TEXT DEFAULT 1_000", text("1000")),
            ("x TEXT DEFAULT 1_000.5", text("1000.5")),
            ("x TEXT DEFAULT 1_000.", text("1000.")),
            ("x TEXT DEFAULT .1_0", text(".10")),
            ("x TEXT DEFAULT 1_0e2", text("10e2")),
            ("x TEXT DEFAULT 1_0e-2", text("10e-2")),
            ("x TEXT DEFAULT 1_0e-5", text("10e-5")),
            ("x TEXT DEFAULT -0_0.0", text("-00.0")),
            ("x TEXT DEFAULT -1_0", text("-10")),
            ("x TEXT DEFAULT 12_34.5_0", text("1234.50")),
            ("x TEXT DEFAULT +1_0e+0_2", text("10e+02")),
            ("x TEXT DEFAULT -0_9223372036854775808", text("-09223372036854775808")),
            ("x TEXT DEFAULT 0_07", text("7")),
            ("x NUMERIC DEFAULT 1_0e-0_1", CellValue::Int(1)),
            ("x NUMERIC DEFAULT '1_000'", text("1_000")),
            ("x INTEGER DEFAULT 1_000", CellValue::Int(1000)),
            ("x INTEGER DEFAULT 1_000.5", CellValue::Real(1000.5)),
            ("x INTEGER DEFAULT .1_0", CellValue::Real(0.1)),
            ("x INTEGER DEFAULT 1_000.", CellValue::Int(1000)),
            ("x REAL DEFAULT 1_0e2", CellValue::Real(1000.0)),
            ("x REAL DEFAULT 1_000", CellValue::Real(1000.0)),
            ("x BLOB DEFAULT 1_000", CellValue::Int(1000)),
            ("x DEFAULT 1_000", CellValue::Int(1000)),
        ] {
            short_record_case(declaration, vec![value], false);
        }
        for declaration in [
            "x DEFAULT (abc)",
            "x DEFAULT (\"abc\")",
            "x DEFAULT (\"5\")",
            "x INTEGER DEFAULT -'abc'",
            "x INTEGER DEFAULT -'1_0'",
            "x DEFAULT 1__000",
            "x DEFAULT 1_",
            "x DEFAULT 1e_2",
        ] {
            short_record_case(declaration, vec![CellValue::Null], true);
        }
    }

    /// Hex and parenthesized literals take the column's affinity too (SQLite 3.51 readings).
    #[test]
    fn hex_and_parenthesized_defaults_take_the_column_affinity() {
        for (declaration, value) in [
            ("x TEXT DEFAULT 0x10", text("16")),
            ("x TEXT DEFAULT -0xFFFFFFFFFFFFFFFF", text("1")),
            ("x TEXT DEFAULT (5.0)", text("5.0")),
            ("x TEXT DEFAULT (1e3)", text("1000.0")),
            ("x TEXT DEFAULT (5.)", text("5.0")),
            ("x TEXT DEFAULT (1e400)", text("Inf")),
            ("x TEXT DEFAULT (- 007)", text("-7")),
            ("x TEXT DEFAULT (TRUE)", text("1")),
            ("x INTEGER DEFAULT ('7')", CellValue::Int(7)),
            ("x NUMERIC DEFAULT (1e3)", CellValue::Int(1000)),
            ("x REAL DEFAULT 0x10", CellValue::Real(16.0)),
            ("x REAL DEFAULT ('1.5')", CellValue::Real(1.5)),
            ("x DEFAULT ('7')", text("7")),
            ("x BLOB DEFAULT (5.0)", CellValue::Real(5.0)),
        ] {
            short_record_case(declaration, vec![value], false);
        }
    }

    /// A STRICT table's ANY column keeps values as they are, so its default isn't turned into a
    /// number, while an ANY column elsewhere is NUMERIC. WITHOUT ROWID rows fill the same way.
    #[test]
    fn strict_any_and_without_rowid_defaults_take_the_column_affinity() {
        use craft_anything_testkit::sqlite::{Val, table_db, without_rowid_db};
        let read = |bytes: Vec<u8>| SqliteDb::parse(&bytes).expect("fixture parses").read_rows(2, 0, 10).rows[0].values.clone();
        let strict = "CREATE TABLE t(n INT, a ANY DEFAULT '5', b ANY DEFAULT 5.0, i INTEGER DEFAULT '7', s TEXT DEFAULT 5) STRICT";
        assert_eq!(read(table_db(strict, &[(1, vec![Val::Int(1)])])), vec![CellValue::Int(1), text("5"), CellValue::Real(5.0), CellValue::Int(7), text("5")]);
        assert_eq!(read(table_db("CREATE TABLE t(n INT, a ANY DEFAULT '5')", &[(1, vec![Val::Int(1)])])), vec![CellValue::Int(1), CellValue::Int(5)]);
        let without_rowid = "CREATE TABLE t(k INT PRIMARY KEY, s TEXT DEFAULT 5, n NUMERIC DEFAULT '2.0') WITHOUT ROWID";
        assert_eq!(read(without_rowid_db(without_rowid, &[vec![Val::Int(1)]])), vec![CellValue::Int(1), text("5"), CellValue::Int(2)]);
    }
}
