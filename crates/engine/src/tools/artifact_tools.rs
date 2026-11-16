//! Artifacts: content-addressed inventories of archives and app bundles, safe extraction, and
//! comparison. Also hosts the filesystem and evidence-input helpers the JavaScript, web and
//! application tool modules share.

use crate::tool::{CancelToken, Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use craft_anything_analysis::js_inventory::{InvLimits, Inventorier, Inventory, compare_inventories};
use craft_anything_analysis::js_source::Ignore;
use craft_anything_evidence::{
    Authority, Confidence, Coverage, Environment, ErrorCode, Evidence, EvidenceBuilder, Limitation, Location, Subject, ToolError, sha256_hex,
};
use craft_anything_formats::{asar, zip};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            id: "core_archive_show",
            title: "Inventory an archive or app bundle",
            summary: "Content-addressed inventory of a ZIP/APK/IPA/JAR, Electron ASAR, app bundle or folder: every file with size, kind, role and sha256; nested archives opened to a bounded depth; ASAR integrity hashes and ZIP CRCs checked. Paged (offset/limit).",
            beginner: "List everything packed inside an app or archive, with a fingerprint for each file.",
            toolset: Toolset::Core,
            effects: Effects::EVIDENCE,
            requires: Requires::Nothing,
            input_schema: schema_of::<InspectArtifactParams>,
            run: core_archive_show,
        },
        ToolSpec {
            id: "core_archive_extract",
            title: "Extract an archive",
            summary: "Extract every regular file of a ZIP/APK/IPA/JAR or ASAR (plus its .asar.unpacked files) into a fresh private temporary folder chosen by Craft Anything, and return its path. Bounded size and count; entries that would escape the folder are refused.",
            beginner: "Unpack an app or archive into a new folder so you can look at the files.",
            toolset: Toolset::Core,
            effects: Effects::MUTATES_SESSION.union(Effects::WRITES_FILESYSTEM),
            requires: Requires::Nothing,
            input_schema: schema_of::<ExtractArtifactParams>,
            run: core_archive_extract,
        },
        ToolSpec {
            id: "core_archives_compare",
            title: "Compare two artifacts",
            summary: "Compare two archives, app bundles or folders file by file: added, removed, changed (by sha256) and moved files. Inputs are absolute paths or core_archive_show evidence.",
            beginner: "See which files differ between two versions of an app or archive.",
            toolset: Toolset::Core,
            effects: Effects::EVIDENCE,
            requires: Requires::Nothing,
            input_schema: schema_of::<CompareArtifactsParams>,
            run: core_archives_compare,
        },
    ]
}

// ---------------------------------------------------------------------------------------------
// Shared helpers

/// `p` relative to `root` with `/` separators on every platform (bundle and archive paths are POSIX);
/// `p` as-is when it is not under `root`.
pub(crate) fn rel_to(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| p.display().to_string())
}

/// Require an absolute path.
pub(crate) fn abs_path(p: &str, what: &str) -> Result<PathBuf, ToolError> {
    let path = PathBuf::from(p);
    if !path.is_absolute() {
        return Err(ToolError::invalid(format!("{what} must be an absolute path, got {p:?}"))
            .with_remediation("Pass the full path, for example /Users/me/Downloads/app.asar."));
    }
    Ok(path)
}

/// Read a whole file, refusing ones larger than `max`.
pub(crate) fn read_capped(path: &Path, max: u64) -> Result<Vec<u8>, ToolError> {
    craft_anything_platform::fs::read_file_capped(path, max)
}

/// Most bytes of a `.gitignore` that [`walk_dir`] reads. A larger file is skipped and reported,
/// so a hostile or runaway ignore file can't size the walk's allocation.
const MAX_GITIGNORE_BYTES: u64 = 1024 * 1024;

/// One file found by [`walk_dir`].
#[derive(Debug, Clone)]
pub(crate) struct WalkedFile {
    /// Relative path with `/` separators.
    pub rel: String,
    pub abs: PathBuf,
    pub size: u64,
}

#[derive(Debug, Default)]
pub(crate) struct Walked {
    /// Sorted by relative path.
    pub files: Vec<WalkedFile>,
    pub dirs: Vec<String>,
    pub links: Vec<String>,
    pub ignored: usize,
    /// A limit cut the listing short (files, depth, or the walk's [`WalkLimits`]), so the tree
    /// holds more than is listed here. The walk stops at the first file or entry over a limit:
    /// what is listed is the same every time, and nothing after the cut was looked at.
    pub truncated: bool,
    /// Folders below the root that couldn't be listed, as (relative path, reason) (#294).
    pub unlisted: Vec<(String, String)>,
    /// `.gitignore` files that couldn't be read whole, as (relative path, reason), so their rules
    /// were skipped.
    pub skipped_gitignores: Vec<(String, String)>,
}

/// A limitation naming the folders that couldn't be listed, as (path, reason), if any (#294).
pub(crate) fn unlisted_limitation(unlisted: &[(String, String)]) -> Option<Limitation> {
    if unlisted.is_empty() {
        return None;
    }
    let first = unlisted.iter().take(5).map(|(rel, e)| format!("{rel} ({e})")).collect::<Vec<_>>().join(", ");
    let more = unlisted.len().saturating_sub(5);
    Some(Limitation::new(
        "unlisted_folders",
        format!(
            "{} folder(s) couldn't be listed, so what they hold is missing: {first}{}",
            unlisted.len(),
            if more > 0 { format!(" and {more} more") } else { String::new() }
        ),
    ))
}

/// A limitation naming the `.gitignore` files whose rules were skipped, if any.
pub(crate) fn skipped_gitignores_limitation(skipped: &[(String, String)]) -> Option<Limitation> {
    if skipped.is_empty() {
        return None;
    }
    let first = skipped.iter().take(5).map(|(path, why)| format!("{path} ({why})")).collect::<Vec<_>>().join(", ");
    let more = skipped.len().saturating_sub(5);
    Some(Limitation::new(
        "gitignore_skipped",
        format!(
            "{} .gitignore file(s) couldn't be read, so files they ignore may be included: {first}{}",
            skipped.len(),
            if more > 0 { format!(" and {more} more") } else { String::new() }
        ),
    ))
}

/// Read one folder's `.gitignore` into `ignore`, recording why its rules were skipped when it
/// can't be read whole. An absent file is silent: that is the normal case.
fn read_gitignore(dir: &Path, rel: &str, ignore: &mut Ignore, skipped: &mut Vec<(String, String)>) {
    let path = dir.join(".gitignore");
    let shown = if rel.is_empty() { ".gitignore".to_string() } else { format!("{rel}/.gitignore") };
    if std::fs::symlink_metadata(&path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return;
    }
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => {
            skipped.push((shown, "couldn't be read".into()));
            return;
        }
    };
    if !meta.is_file() {
        skipped.push((shown, "not a regular file".into()));
        return;
    }
    if meta.len() > MAX_GITIGNORE_BYTES {
        skipped.push((shown, format!("{} bytes; the limit is {MAX_GITIGNORE_BYTES}", meta.len())));
        return;
    }
    match read_capped(&path, MAX_GITIGNORE_BYTES) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => ignore.add_file(rel, &text),
            Err(_) => skipped.push((shown, "not UTF-8".into())),
        },
        Err(_) => skipped.push((shown, "couldn't be read".into())),
    }
}

pub(crate) struct WalkOptions<'a> {
    pub max_files: usize,
    pub max_depth: usize,
    /// Skip a directory (relative path) entirely.
    pub skip_dir: &'a dyn Fn(&str) -> bool,
    /// Honour `.gitignore` files found on the way.
    pub gitignore: bool,
    pub include_hidden: bool,
}

/// What one walk may look at in total, whatever the tree holds. Files have their own limit
/// (`WalkOptions::max_files`), but a tree can be all empty folders, links or ignored names, and one
/// folder can hold millions of entries: these bound the entries examined and the memory held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WalkLimits {
    /// Most directory entries examined: files, folders, links, hidden and ignored names alike.
    pub max_entries: usize,
    /// Most bytes of relative paths examined, which bounds what the walk holds in memory.
    pub max_path_bytes: usize,
}

impl WalkLimits {
    /// Entries allowed besides twice the file limit, so a small file limit still walks an ordinary tree.
    const SPARE_ENTRIES: usize = 100_000;
    /// The most entries any walk examines, however large a file limit the caller passes.
    const MOST_ENTRIES: usize = 4_000_000;
    /// The mean relative path length a full walk may have; real trees stay well under it.
    const PATH_BYTES_PER_ENTRY: usize = 256;

    /// The limits for a walk that lists up to `max_files` files: room for as many folders, links
    /// and ignored names again, and some to spare.
    pub(crate) fn for_files(max_files: usize) -> WalkLimits {
        let max_entries = max_files.saturating_mul(2).saturating_add(Self::SPARE_ENTRIES).min(Self::MOST_ENTRIES);
        WalkLimits { max_entries, max_path_bytes: max_entries.saturating_mul(Self::PATH_BYTES_PER_ENTRY) }
    }
}

/// One folder's entries that fit what is left of a walk's limits, sorted by name.
struct Listing {
    /// (name, name as the system has it).
    names: Vec<(String, std::ffi::OsString)>,
    /// The path bytes `names` uses against the limits.
    path_bytes: usize,
    /// The folder holds more than `names`.
    cut: bool,
}

/// Keeps the first names of a folder in name order that fit `max_entries` and `max_path_bytes`,
/// holding no more than that in memory however many are offered. The names kept don't depend on
/// the order they are offered in (the system's listing order isn't ours to rely on), so a cut
/// listing is the same every time.
struct Smallest {
    /// The largest kept name is on top.
    kept: std::collections::BinaryHeap<(String, std::ffi::OsString)>,
    path_bytes: usize,
    /// The smallest name left out. Everything kept sorts before it, so the kept names are always a
    /// prefix of the sorted folder.
    cut: Option<(String, std::ffi::OsString)>,
    /// What the folder's own relative path adds to each entry's.
    prefix_len: usize,
    max_entries: usize,
    max_path_bytes: usize,
}

impl Smallest {
    fn new(prefix_len: usize, max_entries: usize, max_path_bytes: usize) -> Smallest {
        Smallest { kept: std::collections::BinaryHeap::new(), path_bytes: 0, cut: None, prefix_len, max_entries, max_path_bytes }
    }

    fn cost(&self, name: &str) -> usize {
        self.prefix_len.saturating_add(name.len())
    }

    fn offer(&mut self, raw: std::ffi::OsString) {
        let item = (raw.to_string_lossy().into_owned(), raw);
        if self.cut.as_ref().is_some_and(|c| item >= *c) {
            return;
        }
        self.path_bytes = self.path_bytes.saturating_add(self.cost(&item.0));
        self.kept.push(item);
        while self.kept.len() > self.max_entries || self.path_bytes > self.max_path_bytes {
            let Some(largest) = self.kept.pop() else { break };
            self.path_bytes = self.path_bytes.saturating_sub(self.cost(&largest.0));
            self.cut = Some(largest);
        }
    }

    fn finish(self) -> Listing {
        Listing { names: self.kept.into_sorted_vec(), path_bytes: self.path_bytes, cut: self.cut.is_some() }
    }
}

/// List a folder's entry names within what is left of a walk's limits (see [`Smallest`]). Reading a
/// huge folder to its end is what keeps a cut listing the same every time; it holds nothing extra,
/// and cancelling stops it.
fn list_within(
    entries: impl Iterator<Item = std::io::Result<std::ffi::OsString>>,
    prefix_len: usize,
    max_entries: usize,
    max_path_bytes: usize,
    cancel: &CancelToken,
) -> Result<Listing, ToolError> {
    let mut smallest = Smallest::new(prefix_len, max_entries, max_path_bytes);
    let mut failed_in_a_row = 0usize;
    for (seen, entry) in entries.enumerate() {
        if seen.is_multiple_of(256) {
            cancel.check()?;
        }
        match entry {
            Ok(name) => {
                failed_in_a_row = 0;
                smallest.offer(name);
            }
            // An entry the system couldn't return is skipped; one that keeps failing ends the listing.
            Err(_) => {
                failed_in_a_row += 1;
                if failed_in_a_row >= 64 {
                    break;
                }
            }
        }
    }
    Ok(smallest.finish())
}

/// Walk a folder without following symbolic links. Bounded by file count and depth, and by
/// [`WalkLimits::for_files`] for everything else a tree can hold.
/// A root that can't be listed is an error; a folder below it that can't be is in `unlisted`.
pub(crate) fn walk_dir(root: &Path, opts: &WalkOptions<'_>, cancel: &CancelToken) -> Result<Walked, ToolError> {
    walk_dir_within(root, opts, WalkLimits::for_files(opts.max_files), cancel)
}

/// [`walk_dir`] with the limits given. The walk stops at the first file over `max_files` and at
/// the first entry over `limits`, so nothing is examined, held or listed past a limit.
pub(crate) fn walk_dir_within(root: &Path, opts: &WalkOptions<'_>, limits: WalkLimits, cancel: &CancelToken) -> Result<Walked, ToolError> {
    let mut out = Walked::default();
    let mut ignore = craft_anything_analysis::js_source::Ignore::default();
    let mut stack: Vec<(PathBuf, String, usize)> = vec![(root.to_path_buf(), String::new(), 0)];
    let mut visited = 0usize;
    let (mut entries_left, mut path_bytes_left) = (limits.max_entries, limits.max_path_bytes);
    'walk: while let Some((dir, rel, depth)) = stack.pop() {
        cancel.check()?;
        if entries_left == 0 || path_bytes_left == 0 {
            // Nothing is left to list this folder with. Whether it is empty isn't looked up, so a
            // tree that fits its limits exactly can read as cut short; never the other way round.
            out.truncated = true;
            break;
        }
        if opts.gitignore {
            read_gitignore(&dir, &rel, &mut ignore, &mut out.skipped_gitignores);
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if rel.is_empty() => return Err(ToolError::io(dir.display(), e)),
            Err(e) => {
                out.unlisted.push((rel, e.to_string()));
                continue;
            }
        };
        let prefix_len = if rel.is_empty() { 0 } else { rel.len().saturating_add(1) };
        let listing = list_within(entries.map(|e| e.map(|e| e.file_name())), prefix_len, entries_left, path_bytes_left, cancel)?;
        entries_left = entries_left.saturating_sub(listing.names.len());
        path_bytes_left = path_bytes_left.saturating_sub(listing.path_bytes);
        for (name, raw) in listing.names {
            visited += 1;
            if visited.is_multiple_of(256) {
                cancel.check()?;
            }
            let path = dir.join(&raw);
            let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if !opts.include_hidden && name.starts_with('.') && name != ".gitignore" {
                out.ignored += 1;
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            let ft = meta.file_type();
            if ft.is_symlink() {
                out.links.push(child_rel);
                continue;
            }
            if ft.is_dir() {
                if name == ".git" || (opts.skip_dir)(&child_rel) || (opts.gitignore && ignore.is_ignored(&child_rel, true)) {
                    out.ignored += 1;
                    continue;
                }
                if depth + 1 > opts.max_depth {
                    out.truncated = true;
                    continue;
                }
                out.dirs.push(child_rel.clone());
                stack.push((path, child_rel, depth + 1));
            } else if ft.is_file() {
                if opts.gitignore && ignore.is_ignored(&child_rel, false) {
                    out.ignored += 1;
                    continue;
                }
                if out.files.len() >= opts.max_files {
                    out.truncated = true;
                    break 'walk;
                }
                out.files.push(WalkedFile { rel: child_rel, abs: path, size: meta.len() });
            }
        }
        if listing.cut {
            out.truncated = true;
            break;
        }
    }
    out.files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

/// Evidence passed inline, as `{"kind":"saved","evidence_id":…}`, or as a bare id.
pub(crate) fn resolve_evidence_input(ctx: &Ctx<'_>, v: &Value, expected: &[&str]) -> Result<Evidence, ToolError> {
    let ev = if let Some(id) = v.as_str() {
        ctx.session.resolve_evidence(id)?
    } else if v.get("kind").and_then(Value::as_str) == Some("saved") {
        let id = v.get("evidence_id").and_then(Value::as_str).ok_or_else(|| ToolError::invalid("saved needs evidence_id"))?;
        ctx.session.resolve_evidence(id)?
    } else {
        let e: Evidence = serde_json::from_value(v.clone()).map_err(|e| {
            ToolError::invalid(format!("not an evidence record: {e}"))
                .with_remediation("Pass the evidence object a tool returned, or {\"kind\":\"saved\",\"evidence_id\":\"ev_…\"}.")
        })?;
        if !e.verify() {
            return Err(ToolError::new(ErrorCode::EvidenceHashMismatch, format!("evidence {} does not match its content", e.id))
                .with_remediation("Pass the record exactly as it was returned, or reference it by evidence_id."));
        }
        e
    };
    if !expected.is_empty() && !expected.contains(&ev.operation.as_str()) {
        return Err(ToolError::invalid(format!("evidence {} came from {}, but this needs {}", ev.id, ev.operation, expected.join(" or "))));
    }
    Ok(ev)
}

/// True when a value names a path rather than evidence.
pub(crate) fn is_path_input(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !craft_anything_evidence::is_evidence_id(s))
}

/// A unique id for a fresh folder name.
pub(crate) fn unique_suffix(seed: &str, attempt: u32) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let h = sha256_hex(format!("{seed}|{}|{nanos}|{attempt}", std::process::id()).as_bytes());
    h.get(..16).unwrap_or(&h).to_string()
}

/// Create a new private directory in the system temp folder.
pub(crate) fn fresh_temp_dir(prefix: &str) -> Result<PathBuf, ToolError> {
    fresh_dir_in(&std::env::temp_dir(), prefix)
}

/// Create a new private directory in `base`, making `base` first when it's missing.
pub(crate) fn fresh_dir_in(base: &Path, prefix: &str) -> Result<PathBuf, ToolError> {
    std::fs::create_dir_all(base).map_err(|e| ToolError::io(base.display(), e))?;
    for attempt in 0..16 {
        let p = base.join(format!("{prefix}-{}", unique_suffix(prefix, attempt)));
        #[cfg(unix)]
        let b = {
            use std::os::unix::fs::DirBuilderExt as _;
            let mut b = std::fs::DirBuilder::new();
            b.mode(0o700);
            b
        };
        #[cfg(not(unix))]
        let b = std::fs::DirBuilder::new();
        match b.create(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(ToolError::io(p.display(), e)),
        }
    }
    Err(ToolError::new(ErrorCode::FileError, "could not create a fresh temporary folder"))
}

/// Create an output directory that must not exist yet (its parent must).
pub(crate) fn create_absent_dir(p: &str) -> Result<PathBuf, ToolError> {
    let path = abs_path(p, "output_directory")?;
    if std::fs::symlink_metadata(&path).is_ok() {
        return Err(
            ToolError::invalid(format!("{} already exists", path.display())).with_remediation("Choose a folder that does not exist yet; it will be created.")
        );
    }
    let parent = path.parent().ok_or_else(|| ToolError::invalid("output_directory has no parent folder"))?;
    if !parent.is_dir() {
        return Err(ToolError::invalid(format!("{} does not exist", parent.display())));
    }
    std::fs::create_dir(&path).map_err(|e| ToolError::io(path.display(), e))?;
    Ok(path)
}

/// Removes a folder from [`create_absent_dir`] when the tool stops before writing anything into
/// it, so the same call can be retried (#234). Call `keep` once the tool has succeeded.
pub(crate) struct UnlessWritten<'a> {
    dir: &'a Path,
    keep: bool,
}

impl<'a> UnlessWritten<'a> {
    pub(crate) fn new(dir: &'a Path) -> Self {
        UnlessWritten { dir, keep: false }
    }

    pub(crate) fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for UnlessWritten<'_> {
    fn drop(&mut self) {
        if !self.keep {
            // remove_dir only removes an empty folder, so anything written stays.
            let _ = std::fs::remove_dir(self.dir);
        }
    }
}

/// Turn an archive member name into safe relative components, or explain why not.
pub(crate) fn safe_components(name: &str) -> Result<Vec<String>, &'static str> {
    if name.contains('\0') {
        return Err("name contains a NUL byte");
    }
    let norm = name.replace('\\', "/");
    if norm.starts_with('/') {
        return Err("absolute path");
    }
    let mut out = Vec::new();
    for c in norm.split('/') {
        match c {
            "" | "." => {}
            ".." => return Err("path climbs out of the folder (..)"),
            c if c.len() == 2 && c.ends_with(':') => return Err("drive-letter path"),
            c if c.contains(':') => return Err("name contains ':' (a Windows stream or drive separator)"),
            c if c.ends_with('.') || c.ends_with(' ') => return Err("name ends with a dot or space, which Windows drops"),
            c => out.push(c.to_string()),
        }
    }
    if out.is_empty() {
        return Err("empty name");
    }
    Ok(out)
}

/// Write a file under `root`, never overwriting and never leaving `root`.
pub(crate) fn write_new_file(root: &Path, rel: &[String], bytes: &[u8]) -> Result<PathBuf, String> {
    let mut path = root.to_path_buf();
    for c in rel {
        if matches!(Path::new(c).components().next(), Some(Component::Normal(_))) && Path::new(c).components().count() == 1 {
            path.push(c);
        } else {
            return Err(format!("unsafe path component {c:?}"));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        // A component that already exists as a symlink would let a write escape.
        let mut cur = root.to_path_buf();
        for c in rel.iter().take(rel.len().saturating_sub(1)) {
            cur.push(c);
            if std::fs::symlink_metadata(&cur).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err("a parent folder is a symbolic link".into());
            }
        }
    }
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| e.to_string())?;
    f.write_all(bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// The evidence environment for built-in engines.
pub(crate) fn env(ctx: &Ctx<'_>) -> Environment {
    Environment::builtin(ctx.session.clean_room())
}

// ---------------------------------------------------------------------------------------------
// Inventory

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum IntegrityPolicy {
    /// Stop with an error when a recorded hash or checksum does not match the content.
    #[default]
    Fail,
    /// Record mismatches as contradictions and keep going.
    Continue,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectArtifactParams {
    /// Absolute path of the archive, app bundle or folder. Defaults to the active target.
    #[serde(default)]
    pub path: Option<String>,
    /// What to do when an ASAR integrity hash or ZIP CRC does not match.
    #[serde(default)]
    pub if_tampered: IntegrityPolicy,
    /// First entry of the page.
    #[serde(default)]
    pub offset: usize,
    /// Entries per page (1–20000).
    #[serde(default = "default_inv_limit")]
    pub limit: usize,
    /// How deep nested archives are opened (0–6).
    #[serde(default = "default_depth")]
    pub max_depth: u32,
    /// Most entries listed in total.
    #[serde(default = "default_max_entries")]
    pub max_entries: usize,
    /// Most bytes read and hashed in total.
    #[serde(default = "default_max_total")]
    pub max_total_bytes: u64,
}

fn default_inv_limit() -> usize {
    500
}
fn default_depth() -> u32 {
    3
}
fn default_max_entries() -> usize {
    200_000
}
fn default_max_total() -> u64 {
    4 * 1024 * 1024 * 1024
}

/// Build an inventory of a path (file or folder).
pub(crate) fn inventory_path(path: &Path, limits: InvLimits, cancel: &CancelToken) -> Result<(Inventory, Subject), ToolError> {
    inventory_path_unlisted(path, limits, cancel).map(|(inv, subject, _)| (inv, subject))
}

/// [`inventory_path`], also returning the folders that couldn't be listed, as (path, reason).
#[allow(clippy::type_complexity)]
fn inventory_path_unlisted(path: &Path, limits: InvLimits, cancel: &CancelToken) -> Result<(Inventory, Subject, Vec<(String, String)>), ToolError> {
    let meta = std::fs::metadata(path).map_err(|e| ToolError::io(path.display(), e))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
    let check = || cancel.is_cancelled();
    if meta.is_dir() {
        let skip = |_: &str| false;
        let walked =
            walk_dir(path, &WalkOptions { max_files: limits.max_entries, max_depth: 64, skip_dir: &skip, gitignore: false, include_hidden: true }, cancel)?;
        let mut inv = Inventorier::new(&name, "directory", limits, &check);
        for d in &walked.dirs {
            let failed = walked.unlisted.iter().find(|(rel, _)| rel == d);
            inv.add_marker(d, "directory", 0, failed.map(|(_, e)| format!("couldn't be listed: {e}")));
        }
        for l in &walked.links {
            inv.add_marker(l, "link", 0, Some("symbolic link (not followed)".into()));
        }
        let mut total = 0u64;
        for f in &walked.files {
            cancel.check()?;
            if total.saturating_add(f.size) > limits.max_total_bytes || f.size > limits.max_entry_bytes {
                inv.add_marker(&f.rel, "unread", 0, Some("size budget reached".into()));
                inv.inv.truncated = true;
                continue;
            }
            match std::fs::read(&f.abs) {
                Ok(b) => {
                    total += b.len() as u64;
                    inv.add_file(&f.rel, &b, 0).map_err(|_| ToolError::cancelled())?;
                }
                Err(e) => inv.add_marker(&f.rel, "unread", 0, Some(e.to_string())),
            }
        }
        let mut out = inv.finish();
        out.truncated |= walked.truncated;
        let id = out.content_id();
        let subject = Subject { kind: "directory".into(), sha256: Some(id.trim_start_matches("inv_").to_string()), name: Some(name) };
        return Ok((out, subject, walked.unlisted));
    }
    let bytes = read_capped(path, limits.max_total_bytes.min(crate::target::MAX_TARGET_BYTES))?;
    let kind = craft_anything_analysis::js_inventory::sniff_kind(&bytes, &name);
    let mut inv = Inventorier::new(&name, kind, limits, &check);
    let mut unlisted = Vec::new();
    match kind {
        "zip" | "apk" | "ipa" | "jar" | "hap" | "msix" | "asar" => {
            inv.add_archive("", &bytes, kind, 0).map_err(|_| ToolError::cancelled())?;
            // Electron keeps some files next to the archive.
            if kind == "asar" {
                let unpacked = PathBuf::from(format!("{}.unpacked", path.display()));
                if unpacked.is_dir() {
                    let skip = |_: &str| false;
                    let walked = walk_dir(
                        &unpacked,
                        &WalkOptions { max_files: limits.max_entries, max_depth: 64, skip_dir: &skip, gitignore: false, include_hidden: true },
                        cancel,
                    )?;
                    inv.inv.truncated |= walked.truncated;
                    unlisted.extend(walked.unlisted.into_iter().map(|(rel, e)| (format!("{name}.unpacked/{rel}"), e)));
                    for f in walked.files.iter().take(limits.max_entries) {
                        if let Ok(b) = std::fs::read(&f.abs) {
                            inv.add_file(&format!("{name}.unpacked/{}", f.rel), &b, 0).map_err(|_| ToolError::cancelled())?;
                        }
                    }
                }
            }
        }
        _ => inv.add_file(&name, &bytes, 0).map_err(|_| ToolError::cancelled())?,
    }
    Ok((inv.finish(), Subject { kind: kind.into(), sha256: Some(sha256_hex(&bytes)), name: Some(name) }, unlisted))
}

fn target_path(ctx: &Ctx<'_>, p: Option<&str>) -> Result<PathBuf, ToolError> {
    match p {
        Some(p) => abs_path(p, "path"),
        None => Ok(ctx.session.active()?.target.path.clone()),
    }
}

fn integrity_error(inv: &Inventory) -> ToolError {
    ToolError::new(
        ErrorCode::FileHashMismatch,
        format!("{} integrity contradiction(s), first: {}", inv.contradictions.len(), inv.contradictions.first().map(Value::to_string).unwrap_or_default()),
    )
    .with_remediation("Pass if_tampered=continue to inventory it anyway and see every mismatch.")
    .with_details(json!({"contradictions": inv.contradictions.iter().take(50).collect::<Vec<_>>()}))
}

fn show_limits(max_depth: u32, max_entries: usize, max_total_bytes: u64) -> InvLimits {
    InvLimits { max_entries: max_entries.clamp(1, 1_000_000), max_depth: max_depth.min(6), max_total_bytes, ..InvLimits::default() }
}

/// Re-read the path a core_archive_show record names and check it is unchanged, then return its inventory at `limits`.
/// The check uses the scope the record was made with, since a folder's id covers only the entries that scope lists.
pub(crate) fn recorded_inventory(ev: &Evidence, limits: InvLimits, cancel: &CancelToken) -> Result<(Inventory, Subject), ToolError> {
    let path = ev.params.get("path").and_then(Value::as_str).ok_or_else(|| ToolError::invalid("inventory evidence has no path"))?;
    let num = |k: &str| ev.params.get(k).and_then(Value::as_u64);
    let recorded = show_limits(
        num("max_depth").map_or(default_depth(), |v| u32::try_from(v).unwrap_or(u32::MAX)),
        num("max_entries").map_or(default_max_entries(), |v| usize::try_from(v).unwrap_or(usize::MAX)),
        num("max_total_bytes").unwrap_or_else(default_max_total),
    );
    // A file's id is the hash of its bytes, whatever the scope; a folder's depends on the scope.
    let check = if ev.subject.kind == "directory" { recorded } else { limits };
    let (inv, subject) = inventory_path(Path::new(path), check, cancel)?;
    if subject.sha256 != ev.subject.sha256 {
        return Err(ToolError::new(ErrorCode::FileChanged, format!("{path} changed since evidence {} was recorded", ev.id))
            .with_remediation("Run core_archive_show again, or pass the path directly."));
    }
    if same_limits(&check, &limits) {
        return Ok((inv, subject));
    }
    inventory_path(Path::new(path), limits, cancel)
}

fn same_limits(a: &InvLimits, b: &InvLimits) -> bool {
    a.max_entries == b.max_entries
        && a.max_depth == b.max_depth
        && a.max_nested_bytes == b.max_nested_bytes
        && a.max_entry_bytes == b.max_entry_bytes
        && a.max_total_bytes == b.max_total_bytes
}

fn core_archive_show(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: InspectArtifactParams = parse_params(params)?;
    let path = target_path(ctx, p.path.as_deref())?;
    let limits = show_limits(p.max_depth, p.max_entries, p.max_total_bytes);
    ctx.report(None, format!("Inventorying {}", path.display()));
    let (inv, subject, unlisted) = inventory_path_unlisted(&path, limits, &ctx.cancel)?;
    let blocking = inv.contradictions.iter().any(|c| c.get("reason").and_then(Value::as_str).is_some_and(|r| r.contains("integrity") || r.contains("CRC")));
    if blocking && p.if_tampered == IntegrityPolicy::Fail {
        return Err(integrity_error(&inv));
    }
    let total = inv.entries.len();
    let limit = p.limit.clamp(1, 20_000);
    let page: Vec<&craft_anything_analysis::js_inventory::InvEntry> = inv.entries.iter().skip(p.offset).take(limit).collect();
    let files = inv.entries.iter().filter(|e| e.sha256.is_some()).count();
    let summary = format!(
        "{} ({}) holds {} files in {} entries{}; {} nested container(s){}.",
        inv.root,
        inv.root_kind,
        files,
        total,
        if inv.truncated { " (listing cut short by a limit)" } else { "" },
        inv.containers.len().saturating_sub(1),
        if inv.contradictions.is_empty() { String::new() } else { format!(", {} integrity contradiction(s)", inv.contradictions.len()) }
    );
    let mut lims = Vec::new();
    if inv.truncated {
        lims.push(Limitation::new(
            "truncated",
            "A size, count or depth limit stopped the inventory early; raise max_entries, max_total_bytes or max_depth to see more.",
        ));
    }
    if inv.unopened_nested > 0 {
        lims.push(Limitation::new(
            "nested_not_opened",
            format!(
                "{} nested archive(s) were not opened (depth or size limit; each entry's note says which); raise max_depth to open deeper ones.",
                inv.unopened_nested
            ),
        ));
    }
    lims.extend(unlisted_limitation(&unlisted));
    lims.push(Limitation::new("no_disk_images", "Disk images (DMG), installers (PKG/MSI) and compressed tarballs are listed as single files, not opened."));
    let mut coverage = Coverage::page(page.len() as u64, total as u64, p.offset as u64);
    if !unlisted.is_empty() || inv.unopened_nested > 0 {
        coverage.status = craft_anything_evidence::CoverageStatus::Partial;
    }
    let ev = EvidenceBuilder::new("core_archive_show", subject, env(ctx))
        .params(json!({"path": path, "if_tampered": p.if_tampered == IntegrityPolicy::Fail, "offset": p.offset, "limit": limit, "max_depth": limits.max_depth, "max_entries": limits.max_entries, "max_total_bytes": limits.max_total_bytes}))
        .result(json!({
            "path": path,
            "root": inv.root, "root_kind": inv.root_kind, "content_id": inv.content_id(),
            "files": files, "entries_total": total, "by_kind": inv.by_kind(), "hashed_bytes": inv.hashed_bytes,
            "containers": inv.containers, "contradictions": inv.contradictions, "truncated": inv.truncated,
            "unopened_nested": inv.unopened_nested,
            "entries": page,
        }))
        .summary(summary)
        .confidence(Confidence::Observed)
        .authority(Authority::OriginalFiles)
        .limitations(lims)
        .coverage(coverage)
        .location(Location::new("path", path.display().to_string()))
        .build()?;
    Ok(ToolOutput::evidence(ev))
}

// ---------------------------------------------------------------------------------------------
// Extraction

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtractArtifactParams {
    /// Absolute path of the ZIP/APK/IPA/JAR or ASAR. Defaults to the active target.
    #[serde(default)]
    pub path: Option<String>,
    /// What to do when an ASAR integrity hash or ZIP CRC does not match.
    #[serde(default)]
    pub if_tampered: IntegrityPolicy,
    /// Most files written.
    #[serde(default = "default_extract_files")]
    pub max_files: usize,
    /// Most bytes written in total.
    #[serde(default = "default_max_total")]
    pub max_bytes: u64,
}

fn default_extract_files() -> usize {
    100_000
}

fn core_archive_extract(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ExtractArtifactParams = parse_params(params)?;
    let path = target_path(ctx, p.path.as_deref())?;
    if path.is_dir() {
        return Err(
            ToolError::unsupported(format!("{} is already a folder", path.display())).with_remediation("Folders don't need extracting; analyse them directly.")
        );
    }
    let bytes = read_capped(&path, crate::target::MAX_TARGET_BYTES)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let kind = craft_anything_analysis::js_inventory::sniff_kind(&bytes, &name);
    // Collect (name, bytes-or-error) without touching the disk first.
    enum Member<'a> {
        Data(&'a [u8]),
        Owned(Vec<u8>),
        Skip(String),
    }
    let mut members: Vec<(String, Member<'_>)> = Vec::new();
    let mut contradictions = Vec::new();
    let listing_truncated;
    match kind {
        "asar" => {
            let a = asar::parse(&bytes, p.max_files.saturating_mul(2)).map_err(|e| ToolError::new(ErrorCode::GarbledOutput, e.to_string()))?;
            listing_truncated = a.truncated;
            for r in &a.rejected_names {
                members.push((r.clone(), Member::Skip("name could escape the folder".into())));
            }
            for e in a.entries.iter().filter(|e| e.kind == asar::AsarKind::File) {
                if e.unpacked {
                    let side = PathBuf::from(format!("{}.unpacked", path.display()));
                    match safe_components(&e.path) {
                        Ok(c) => {
                            let src = c.iter().fold(side, |acc, x| acc.join(x));
                            match std::fs::symlink_metadata(&src) {
                                Ok(m) if m.is_file() && m.len() <= p.max_bytes => match std::fs::read(&src) {
                                    Ok(b) => members.push((e.path.clone(), Member::Owned(b))),
                                    Err(err) => members.push((e.path.clone(), Member::Skip(err.to_string()))),
                                },
                                _ => members.push((e.path.clone(), Member::Skip("unpacked file not found beside the archive".into()))),
                            }
                        }
                        Err(r) => members.push((e.path.clone(), Member::Skip(r.into()))),
                    }
                    continue;
                }
                match asar::file_bytes(&bytes, &a, e) {
                    Some(d) => {
                        if let Some(i) = &e.integrity
                            && i.algorithm.eq_ignore_ascii_case("sha256")
                            && sha256_hex(d) != i.hash
                        {
                            contradictions.push(json!({"path": e.path, "reason": "ASAR integrity hash does not match"}));
                        }
                        members.push((e.path.clone(), Member::Data(d)));
                    }
                    None => members.push((e.path.clone(), Member::Skip("data lies outside the archive".into()))),
                }
            }
        }
        "zip" | "apk" | "ipa" | "jar" | "hap" | "msix" => {
            let z = zip::parse(&bytes, p.max_files.saturating_mul(2)).map_err(|e| ToolError::new(ErrorCode::GarbledOutput, e.to_string()))?;
            listing_truncated = z.truncated;
            for e in z.entries.iter().filter(|e| !e.is_dir) {
                if let Some(kind) = e.special_kind() {
                    members.push((e.name.clone(), Member::Skip(format!("{kind} member is not a regular file"))));
                    continue;
                }
                match zip::read_entry(&bytes, e, p.max_bytes) {
                    Ok(d) => members.push((e.name.clone(), Member::Owned(d))),
                    Err(err @ zip::ZipError::CrcMismatch { .. }) => {
                        contradictions.push(json!({"path": e.name, "reason": err.to_string()}));
                        let m = if p.if_tampered == IntegrityPolicy::Continue {
                            match zip::read_entry_unverified(&bytes, e, p.max_bytes) {
                                Ok(d) => Member::Owned(d),
                                Err(read_err) => Member::Skip(read_err.to_string()),
                            }
                        } else {
                            Member::Skip(err.to_string())
                        };
                        members.push((e.name.clone(), m));
                    }
                    Err(err) => members.push((e.name.clone(), Member::Skip(err.to_string()))),
                }
            }
        }
        other => {
            return Err(ToolError::unsupported(format!("{name} is a {other} file, not an archive this tool extracts"))
                .with_remediation("core_archive_extract handles ZIP-based packages (zip, apk, ipa, jar, hap, msix) and Electron ASAR archives."));
        }
    }
    if !contradictions.is_empty() && p.if_tampered == IntegrityPolicy::Fail {
        return Err(ToolError::new(ErrorCode::FileHashMismatch, format!("{} member(s) fail their integrity check", contradictions.len()))
            .with_remediation("Pass if_tampered=continue to extract anyway.")
            .with_details(json!({"contradictions": contradictions})));
    }
    let out = fresh_temp_dir("craft-anything-extract")?;
    let mut written = 0usize;
    let mut total = 0u64;
    let mut skipped = Vec::new();
    for (i, (mname, m)) in members.iter().enumerate() {
        if i % 64 == 0 {
            ctx.cancel.check()?;
        }
        let data: &[u8] = match m {
            Member::Data(d) => d,
            Member::Owned(d) => d,
            Member::Skip(r) => {
                skipped.push(json!({"path": mname, "reason": r}));
                continue;
            }
        };
        if written >= p.max_files || total.saturating_add(data.len() as u64) > p.max_bytes {
            skipped.push(json!({"path": mname, "reason": "file count or size limit reached"}));
            continue;
        }
        let comps = match safe_components(mname) {
            Ok(c) => c,
            Err(r) => {
                skipped.push(json!({"path": mname, "reason": r}));
                continue;
            }
        };
        match write_new_file(&out, &comps, data) {
            Ok(_) => {
                written += 1;
                total += data.len() as u64;
            }
            Err(r) => skipped.push(json!({"path": mname, "reason": r})),
        }
    }
    let summary = format!(
        "Extracted {written} files ({total} bytes) from {name} into {}{}.",
        out.display(),
        if skipped.is_empty() { String::new() } else { format!("; skipped {}", skipped.len()) }
    );
    let mut limitations =
        vec![Limitation::new("nested_not_extracted", "Archives inside the archive are written as files, not unpacked; run core_archive_extract on them.")];
    if listing_truncated {
        limitations.push(Limitation::new("truncated", "The archive listing was cut short; members beyond it were not read."));
    }
    let ev = EvidenceBuilder::new("core_archive_extract", Subject { kind: kind.into(), sha256: Some(sha256_hex(&bytes)), name: Some(name) }, env(ctx))
        .params(json!({"path": path, "max_files": p.max_files, "max_bytes": p.max_bytes}))
        .result(json!({"output_directory": out, "files_written": written, "bytes_written": total, "skipped": skipped.iter().take(2000).collect::<Vec<_>>(), "contradictions": contradictions}))
        .summary(summary)
        .confidence(Confidence::Observed)
        .limitations(limitations)
        .coverage(Coverage { status: if skipped.is_empty() && !listing_truncated { craft_anything_evidence::CoverageStatus::Complete } else { craft_anything_evidence::CoverageStatus::Partial }, examined: Some(written as u64), total: (!listing_truncated).then_some(members.len() as u64), next_offset: None })
        .location(Location::new("path", out.display().to_string()).with_label("output folder"))
        .build()?;
    Ok(ToolOutput::evidence(ev))
}

// ---------------------------------------------------------------------------------------------
// Compare

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompareArtifactsParams {
    /// The older artifact: an absolute path, or core_archive_show evidence (inline or {"kind":"saved","evidence_id":…}).
    pub left: Value,
    /// The newer artifact, in the same forms.
    pub right: Value,
    /// Most rows per list.
    #[serde(default = "default_rows")]
    pub max_rows: usize,
}

fn default_rows() -> usize {
    500
}

fn inventory_input(ctx: &Ctx<'_>, v: &Value) -> Result<(Inventory, Option<String>), ToolError> {
    if let Some(p) = is_path_input(v) {
        let (inv, _) = inventory_path(&abs_path(p, "path")?, InvLimits::default(), &ctx.cancel)?;
        return Ok((inv, None));
    }
    let ev = resolve_evidence_input(ctx, v, &["core_archive_show"])?;
    // Evidence holds one page; re-read the recorded path for the full inventory and check it is unchanged.
    let (inv, _) = recorded_inventory(&ev, InvLimits::default(), &ctx.cancel)?;
    Ok((inv, Some(ev.id)))
}

/// Compare two inventories, reporting capped row lists and partial coverage while the counts stay
/// complete.
fn core_archives_compare(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: CompareArtifactsParams = parse_params(params)?;
    let (l, lid) = inventory_input(ctx, &p.left)?;
    ctx.cancel.check()?;
    let (r, rid) = inventory_input(ctx, &p.right)?;
    let max_rows = p.max_rows.clamp(1, 100_000);
    let d = compare_inventories(&l, &r, max_rows);
    let rows_capped = d["rows_capped"].as_bool().unwrap_or(false);
    let c = &d["counts"];
    let summary = format!(
        "{} → {}: {} added, {} removed, {} changed, {} moved, {} unchanged.",
        l.root, r.root, c["added"], c["removed"], c["changed"], c["moved"], c["unchanged"]
    );
    let mut b = EvidenceBuilder::new(
        "core_archives_compare",
        Subject { kind: "artifact-pair".into(), sha256: None, name: Some(format!("{} vs {}", l.root, r.root)) },
        env(ctx),
    )
    .params(json!({"left": l.content_id(), "right": r.content_id()}))
    .result(d)
    .summary(summary)
    .confidence(Confidence::Derived);
    if l.truncated || r.truncated {
        b = b.limitation(Limitation::new("truncated", "One inventory was cut short by a limit; files beyond it are not compared."));
    }
    if rows_capped {
        b = b
            .limitation(Limitation::new("rows_capped", format!("lists are capped at max_rows={max_rows}; counts are complete")))
            .coverage(Coverage { status: craft_anything_evidence::CoverageStatus::Partial, ..Default::default() });
    }
    for id in [lid, rid].into_iter().flatten() {
        b = b.link(id);
    }
    Ok(ToolOutput::evidence(b.build()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CallOptions, Caller, Engine};
    use craft_anything_testkit::electron as fx;

    fn call(e: &Engine, id: &str, p: Value) -> Result<Value, ToolError> {
        e.call(id, p, CallOptions::new(Caller::Cli)).map(|o| o.to_json())
    }

    #[test]
    fn unopened_nested_archives_are_disclosed_in_coverage() {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::new(crate::testing::session());
        for depth in [2, 5] {
            let mut bytes = fx::zip_stored(&[("leaf.txt", b"leaf")]);
            for _ in 0..depth {
                bytes = fx::zip_stored(&[("inner.zip", &bytes)]);
            }
            let path = dir.path().join(format!("nested-{depth}.zip"));
            std::fs::write(&path, bytes).unwrap();
            let inv = call(&e, "core_archive_show", json!({"path": path})).unwrap();
            let unopened = depth == 5;
            assert_eq!(inv["limitations"].as_array().unwrap().iter().any(|l| l["code"] == "nested_not_opened"), unopened);
            assert_eq!(inv["coverage"]["status"], if unopened { "partial" } else { "complete" });
            assert_eq!(inv["result"]["entries"].as_array().unwrap().iter().any(|entry| entry["note"] == "nested archive not opened: depth limit"), unopened);
            assert_eq!(inv["result"]["truncated"], false);
        }
    }

    #[test]
    fn inventory_and_extract_asar() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("app.asar");
        std::fs::write(&a, fx::notes_app_asar()).unwrap();
        let e = Engine::new(crate::testing::session());
        let inv = call(&e, "core_archive_show", json!({"path": a})).unwrap();
        assert_eq!(inv["result"]["root_kind"], "asar");
        assert_eq!(inv["result"]["files"], 5);
        assert!(inv["result"]["entries"].as_array().unwrap().iter().any(|x| x["path"] == "renderer/app.js" && x["kind"] == "javascript"));
        let page = call(&e, "core_archive_show", json!({"path": a, "offset": 0, "limit": 2})).unwrap();
        assert_eq!(page["coverage"]["status"], "partial");
        assert_eq!(page["coverage"]["next_offset"], 2);
        let x = call(&e, "core_archive_extract", json!({"path": a})).unwrap();
        let out = PathBuf::from(x["result"]["output_directory"].as_str().unwrap());
        assert_eq!(std::fs::read_to_string(out.join("main.js")).unwrap(), fx::MAIN_JS);
        assert_eq!(x["result"]["files_written"], 5);
        std::fs::remove_dir_all(out).unwrap();
    }

    #[test]
    fn zip_slip_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let z = dir.path().join("evil.zip");
        std::fs::write(&z, fx::zip_stored(&[("../../escape.txt", b"x"), ("/etc/abs", b"y"), ("C:/win", b"z"), ("ok/fine.txt", b"ok")])).unwrap();
        let e = Engine::new(crate::testing::session());
        let x = call(&e, "core_archive_extract", json!({"path": z})).unwrap();
        assert_eq!(x["result"]["files_written"], 1);
        assert_eq!(x["result"]["skipped"].as_array().unwrap().len(), 3);
        let out = PathBuf::from(x["result"]["output_directory"].as_str().unwrap());
        assert!(out.join("ok/fine.txt").exists());
        assert!(!out.parent().unwrap().join("escape.txt").exists());
        std::fs::remove_dir_all(out).unwrap();
    }

    #[test]
    fn extraction_refuses_names_windows_would_change() {
        let dir = tempfile::tempdir().unwrap();
        let z = dir.path().join("names.zip");
        std::fs::write(&z, fx::zip_stored(&[("dot.txt.", b"wrong"), ("dot.txt", b"real"), ("ads.txt:hidden", b"hidden")])).unwrap();
        let e = Engine::new(crate::testing::session());
        let x = call(&e, "core_archive_extract", json!({"path": z})).unwrap();
        let out = PathBuf::from(x["result"]["output_directory"].as_str().unwrap());
        let real = std::fs::read(out.join("dot.txt")).unwrap();
        let ads_exists = out.join("ads.txt").exists();
        std::fs::remove_dir_all(out).unwrap();
        assert_eq!(real, b"real");
        let skipped = x["result"]["skipped"].as_array().unwrap();
        assert!(skipped.iter().any(|s| s["path"] == "dot.txt."));
        assert!(skipped.iter().any(|s| s["path"] == "ads.txt:hidden"));
        assert!(!ads_exists);
        assert_eq!(x["result"]["files_written"], 1);
    }

    #[test]
    fn zip_symlink_member_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let z = dir.path().join("link.zip");
        let mut b = fx::zip_stored(&[("good.txt", b"regular\n"), ("link_out", b"../../target.txt")]);
        let at = (0..b.len().saturating_sub(54)).find(|&i| b.get(i..i + 4) == Some(b"PK\x01\x02") && b.get(i + 46..i + 54) == Some(b"link_out")).unwrap();
        b[at + 5] = 3;
        b[at + 38..at + 42].copy_from_slice(&((0o120777u32) << 16).to_le_bytes());
        std::fs::write(&z, b).unwrap();
        let e = Engine::new(crate::testing::session());
        let x = call(&e, "core_archive_extract", json!({"path": z})).unwrap();
        assert_eq!(x["result"]["files_written"], 1);
        let sk = x["result"]["skipped"].as_array().unwrap();
        assert_eq!(sk.len(), 1);
        assert_eq!(sk[0]["path"], "link_out");
        assert!(sk[0]["reason"].as_str().unwrap().contains("symlink"));
        let out = PathBuf::from(x["result"]["output_directory"].as_str().unwrap());
        assert!(out.join("good.txt").exists());
        assert!(!out.join("link_out").exists());
        std::fs::remove_dir_all(out).unwrap();
    }

    #[test]
    fn compare_two_folders_and_evidence_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let a = fx::write_files(&dir.path().join("v1"), &fx::notes_app_files()).unwrap();
        let b = fx::write_files(&dir.path().join("v2"), &fx::notes_app_v2_files()).unwrap();
        let e = Engine::new(crate::testing::session());
        let d = call(&e, "core_archives_compare", json!({"left": a, "right": b})).unwrap();
        assert_eq!(d["result"]["counts"]["changed"], 2, "{}", d["result"]);
        let inv = call(&e, "core_archive_show", json!({"path": a})).unwrap();
        let id = inv["id"].as_str().unwrap();
        let d2 = call(&e, "core_archives_compare", json!({"left": {"kind": "saved", "evidence_id": id}, "right": b})).unwrap();
        assert_eq!(d2["result"]["counts"], d["result"]["counts"]);
        assert!(call(&e, "core_archives_compare", json!({"left": "relative/path", "right": b})).is_err());
    }

    /// A comparison with more changed files than `max_rows` reports the complete counts but says
    /// the rows were capped, with partial coverage.
    #[test]
    fn archive_compare_discloses_capped_rows() {
        let dir = tempfile::tempdir().unwrap();
        let a = fx::write_files(&dir.path().join("v1"), &fx::notes_app_files()).unwrap();
        let b = fx::write_files(&dir.path().join("v2"), &fx::notes_app_v2_files()).unwrap();
        let e = Engine::new(crate::testing::session());
        let d = call(&e, "core_archives_compare", json!({"left": a, "right": b, "max_rows": 1})).unwrap();
        assert_eq!(d["result"]["counts"]["changed"], 2, "{}", d["result"]);
        assert_eq!(d["result"]["changed"].as_array().map(Vec::len), Some(1), "{}", d["result"]);
        assert_eq!(d["result"]["rows_capped"], true);
        assert!(d["limitations"].as_array().unwrap().iter().any(|l| l["code"] == "rows_capped"), "{d}");
        assert_eq!(d["coverage"]["status"], "partial", "{d}");
    }

    #[test]
    fn folder_evidence_scope_is_not_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app");
        std::fs::create_dir(&app).unwrap();
        let l3 = fx::zip_stored(&[("leaf.txt", b"leaf")]);
        let l2 = fx::zip_stored(&[("l3.zip", &l3)]);
        let l1 = fx::zip_stored(&[("l2.zip", &l2)]);
        std::fs::write(app.join("l1.zip"), &l1).unwrap();
        std::fs::write(app.join("other.txt"), b"other").unwrap();
        let e = Engine::new(crate::testing::session());
        let tools = ["core_archives_compare", "android_app_graph_build", "apple_app_graph_build", "harmony_app_graph_build"];
        let consume = |tool: &str, input: Value| {
            let p = if tool == "core_archives_compare" { json!({"left": input, "right": app}) } else { json!({"inventory": input}) };
            call(&e, tool, p)
        };
        let scopes = [json!({"max_depth": 2}), json!({"max_depth": 3}), json!({"max_entries": 2}), json!({"max_total_bytes": 10})];
        let mut recorded = Vec::new();
        for scope in scopes {
            let mut p = scope.clone();
            p["path"] = json!(app);
            let inv = call(&e, "core_archive_show", p).unwrap();
            let id = inv["id"].as_str().unwrap().to_string();
            for tool in tools {
                for input in [json!({"kind": "saved", "evidence_id": id}), inv.clone()] {
                    let out = consume(tool, input).unwrap_or_else(|err| panic!("{scope} {tool}: {err:?}"));
                    assert!(out["links"].as_array().unwrap().contains(&json!(id)), "{scope} {tool}");
                    if tool == "core_archives_compare" {
                        for k in ["added", "removed", "changed", "moved"] {
                            assert_eq!(out["result"]["counts"][k], 0, "{scope} {k}");
                        }
                    }
                }
            }
            recorded.push((id, inv));
        }
        // Real changes are still refused (both files change, so each recorded scope covers one).
        std::fs::write(app.join("l1.zip"), fx::zip_stored(&[("l2.zip", b"changed")])).unwrap();
        std::fs::write(app.join("other.txt"), b"changed").unwrap();
        for (id, inv) in recorded {
            for tool in tools {
                for input in [json!({"kind": "saved", "evidence_id": id}), inv.clone()] {
                    let err = consume(tool, input).unwrap_err();
                    assert_eq!(err.code, ErrorCode::FileChanged, "{tool}");
                }
            }
        }
    }

    #[test]
    fn if_tampered() {
        let dir = tempfile::tempdir().unwrap();
        let mut z = fx::zip_stored(&[("a.txt", b"hello")]);
        // Corrupt the stored data so the CRC no longer matches.
        let pos = z.windows(5).position(|w| w == b"hello").unwrap();
        z[pos] = b'j';
        let p = dir.path().join("bad.zip");
        std::fs::write(&p, &z).unwrap();
        let e = Engine::new(crate::testing::session());
        let err = call(&e, "core_archive_show", json!({"path": p})).unwrap_err();
        assert_eq!(err.code, ErrorCode::FileHashMismatch);
        let ok = call(&e, "core_archive_show", json!({"path": p, "if_tampered": "continue"})).unwrap();
        assert_eq!(ok["result"]["contradictions"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn extract_if_tampered_continue_writes_the_member() {
        let dir = tempfile::tempdir().unwrap();
        let mut z = fx::zip_stored(&[("a.txt", b"hello")]);
        let pos = z.windows(5).position(|w| w == b"hello").unwrap();
        z[pos] = b'j';
        let p = dir.path().join("bad.zip");
        std::fs::write(&p, &z).unwrap();
        let e = Engine::new(crate::testing::session());
        let err = call(&e, "core_archive_extract", json!({"path": p})).unwrap_err();
        assert_eq!(err.code, ErrorCode::FileHashMismatch);
        let ok = call(&e, "core_archive_extract", json!({"path": p, "if_tampered": "continue"})).unwrap();
        let r = &ok["result"];
        assert_eq!(r["files_written"], 1, "{r}");
        assert_eq!(r["contradictions"].as_array().unwrap().len(), 1);
        let out = std::path::PathBuf::from(r["output_directory"].as_str().unwrap());
        assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"jello");
    }

    #[test]
    fn safe_components_rules() {
        assert!(safe_components("../x").is_err());
        assert!(safe_components("/x").is_err());
        assert!(safe_components("a\\..\\..\\x").is_err());
        assert_eq!(safe_components("C:\\x"), Err("drive-letter path"));
        assert_eq!(safe_components("C:x"), Err("name contains ':' (a Windows stream or drive separator)"));
        for name in ["ads.txt:hidden", "dir/a:b", "dot.txt.", "space.txt ", "dir./x"] {
            assert!(safe_components(name).is_err(), "{name:?} must be refused");
        }
        assert_eq!(safe_components("a.b.txt").unwrap(), vec!["a.b.txt"]);
        assert_eq!(safe_components(".hidden").unwrap(), vec![".hidden"]);
        assert_eq!(safe_components("dir/file.txt").unwrap(), vec!["dir", "file.txt"]);
        assert_eq!(safe_components("./a//b").unwrap(), vec!["a", "b"]);
        assert!(safe_components("").is_err());
    }

    const ROOMY: WalkLimits = WalkLimits { max_entries: 1_000_000, max_path_bytes: 1 << 30 };

    fn walk(root: &Path, max_files: usize, limits: WalkLimits) -> Walked {
        let skip = |_: &str| false;
        let opts = WalkOptions { max_files, max_depth: 64, skip_dir: &skip, gitignore: false, include_hidden: true };
        walk_dir_within(root, &opts, limits, &CancelToken::default()).unwrap()
    }

    fn rels(w: &Walked) -> Vec<&str> {
        w.files.iter().map(|f| f.rel.as_str()).collect()
    }

    fn names_in(dir: &Path) -> impl Iterator<Item = std::io::Result<std::ffi::OsString>> {
        std::fs::read_dir(dir).unwrap().map(|e| e.map(|e| e.file_name()))
    }

    #[test]
    fn a_walk_within_its_limits_lists_the_whole_tree_in_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub/empty")).unwrap();
        for f in ["b.txt", "a.txt", "sub/c.txt"] {
            std::fs::write(dir.path().join(f), f).unwrap();
        }
        let w = walk(dir.path(), 10, ROOMY);
        assert_eq!(rels(&w), ["a.txt", "b.txt", "sub/c.txt"]);
        assert_eq!(w.dirs, ["sub", "sub/empty"]);
        assert!(!w.truncated && w.links.is_empty() && w.unlisted.is_empty());
        // The tree's 5 entries and 31 path bytes (5 + 5 + 3 + 9 + 9) fit limits with one to spare.
        let snug = walk(dir.path(), 3, WalkLimits { max_entries: 6, max_path_bytes: 32 });
        assert_eq!((rels(&snug), &snug.dirs, snug.truncated), (rels(&w), &w.dirs, false));
        // With nothing to spare the last folder isn't opened, so the walk can't say it saw it all.
        for exact in [WalkLimits { max_entries: 5, max_path_bytes: 32 }, WalkLimits { max_entries: 6, max_path_bytes: 31 }] {
            let cut = walk(dir.path(), 3, exact);
            assert_eq!((rels(&cut), &cut.dirs, cut.truncated), (rels(&w), &w.dirs, true));
        }
        // The limits every caller gets leave an ordinary tree whole.
        let skip = |_: &str| false;
        let opts = WalkOptions { max_files: 3, max_depth: 64, skip_dir: &skip, gitignore: false, include_hidden: true };
        let usual = walk_dir(dir.path(), &opts, &CancelToken::default()).unwrap();
        assert_eq!((rels(&usual), &usual.dirs, usual.truncated), (rels(&w), &w.dirs, false));
    }

    #[test]
    fn walk_limits_follow_the_file_limit_and_never_pass_the_ceiling() {
        assert_eq!(WalkLimits::for_files(0).max_entries, 100_000);
        assert_eq!(WalkLimits::for_files(200_000).max_entries, 500_000);
        assert_eq!(WalkLimits::for_files(1_000_000).max_entries, 2_100_000, "the largest inventory keeps room for its folders and links");
        for huge in [3_000_000, usize::MAX] {
            let l = WalkLimits::for_files(huge);
            assert_eq!((l.max_entries, l.max_path_bytes), (4_000_000, 4_000_000 * 256));
        }
    }

    /// A folder with more entries than the walk may examine: only the first by name are held and listed.
    #[test]
    fn a_huge_folder_is_listed_up_to_the_entry_limit_in_name_order() {
        let dir = tempfile::tempdir().unwrap();
        for i in (0..300).rev() {
            std::fs::write(dir.path().join(format!("f{i:03}.txt")), b"x").unwrap();
        }
        let limits = WalkLimits { max_entries: 50, max_path_bytes: 1 << 20 };
        let w = walk(dir.path(), 1000, limits);
        let first: Vec<String> = (0..50).map(|i| format!("f{i:03}.txt")).collect();
        assert_eq!(rels(&w), first);
        assert!(w.truncated);
        assert_eq!(rels(&walk(dir.path(), 1000, limits)), first, "a cut listing is the same every time");
        let listing = list_within(names_in(dir.path()), 0, 50, 1 << 20, &CancelToken::default()).unwrap();
        assert_eq!((listing.names.len(), listing.path_bytes, listing.cut), (50, 50 * 8, true));
        // The path-byte limit bounds it the same way: 8 bytes a name, so 12 fit in 100.
        let by_bytes = walk(dir.path(), 1000, WalkLimits { max_entries: 1000, max_path_bytes: 100 });
        assert_eq!(rels(&by_bytes), first.iter().take(12).collect::<Vec<_>>());
        assert!(by_bytes.truncated);
        // A folder below the root is charged its own path too ("deep/" + 8 bytes).
        let nested = tempfile::tempdir().unwrap();
        std::fs::create_dir(nested.path().join("deep")).unwrap();
        for i in 0..20 {
            std::fs::write(nested.path().join("deep").join(format!("f{i:03}.txt")), b"x").unwrap();
        }
        let w = walk(nested.path(), 1000, WalkLimits { max_entries: 1000, max_path_bytes: 4 + 3 * 13 });
        assert_eq!(rels(&w), ["deep/f000.txt", "deep/f001.txt", "deep/f002.txt"]);
        assert!(w.truncated);
    }

    /// The names kept from a folder are the first in name order whatever order the system lists
    /// them in, for the entry limit and the byte limit alike.
    #[test]
    fn the_names_kept_from_a_folder_dont_depend_on_listing_order() {
        let names = ["delta", "a", "charlie-with-a-long-name", "bb", "echo", "b", "alpha", "zz", "c"];
        let mut sorted = names.to_vec();
        sorted.sort();
        // All nine take 63 bytes with their 2-byte prefix: the last two cases are all, and all but one.
        for (max_entries, max_bytes) in [(4, 1000), (1000, 12), (3, 7), (0, 1000), (1000, 0), (1000, 1000), (9, 63), (9, 62)] {
            // The answer: the longest run from the start of the sorted names that fits both limits.
            let mut want: Vec<&str> = Vec::new();
            let mut bytes = 0;
            for n in &sorted {
                if want.len() + 1 > max_entries || bytes + 2 + n.len() > max_bytes {
                    break;
                }
                bytes += 2 + n.len();
                want.push(n);
            }
            for rotation in 0..names.len() {
                for reversed in [false, true] {
                    let mut order = names.to_vec();
                    order.rotate_left(rotation);
                    if reversed {
                        order.reverse();
                    }
                    let mut smallest = Smallest::new(2, max_entries, max_bytes);
                    for n in &order {
                        smallest.offer(std::ffi::OsString::from(*n));
                        assert!(smallest.kept.len() <= max_entries && smallest.path_bytes <= max_bytes, "never holds more than the limits");
                    }
                    let listing = smallest.finish();
                    let got: Vec<&str> = listing.names.iter().map(|(n, _)| n.as_str()).collect();
                    assert_eq!(got, want, "limits {max_entries}/{max_bytes}, order {order:?}");
                    assert_eq!((listing.path_bytes, listing.cut), (bytes, want.len() < names.len()));
                }
            }
        }
    }

    /// A tree with no files at all never reaches the file limit; the entry limit still ends the walk.
    #[test]
    fn a_tree_of_empty_folders_stops_at_the_entry_limit() {
        let dir = tempfile::tempdir().unwrap();
        for d in 0..30 {
            for e in 0..3 {
                std::fs::create_dir_all(dir.path().join(format!("d{d:02}/e{e}"))).unwrap();
            }
        }
        let w = walk(dir.path(), 1000, WalkLimits { max_entries: 10, max_path_bytes: 1 << 20 });
        assert_eq!(w.dirs, (0..10).map(|d| format!("d{d:02}")).collect::<Vec<_>>(), "the root's first ten, and nothing below them");
        assert!(w.truncated && w.files.is_empty());
        // 30 at the root, d29's three, then two of d28's: 35 examined, and the rest never opened.
        let w = walk(dir.path(), 1000, WalkLimits { max_entries: 35, max_path_bytes: 1 << 20 });
        assert_eq!(w.dirs.len(), 35);
        assert_eq!(w.dirs.iter().rev().take(2).collect::<Vec<_>>(), ["d28/e1", "d28/e0"]);
        assert!(w.truncated);
        let whole = walk(dir.path(), 1000, WalkLimits { max_entries: 121, max_path_bytes: 1 << 20 });
        assert_eq!((whole.dirs.len(), whole.truncated), (120, false));
    }

    /// Links are noted and never followed, so a tree of nothing but links has no files either.
    #[test]
    fn a_tree_of_links_stops_at_the_entry_limit() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        std::fs::create_dir(&tree).unwrap();
        let target = dir.path().join("target.txt");
        std::fs::write(&target, b"x").unwrap();
        for i in 0..40 {
            let link = tree.join(format!("l{i:02}"));
            #[cfg(unix)]
            let made = std::os::unix::fs::symlink(&target, &link);
            #[cfg(windows)]
            let made = std::os::windows::fs::symlink_file(&target, &link);
            if made.is_err() {
                eprintln!("skipped: this account can't create symbolic links");
                return;
            }
        }
        let w = walk(&tree, 1000, WalkLimits { max_entries: 15, max_path_bytes: 1 << 20 });
        assert_eq!(w.links, (0..15).map(|i| format!("l{i:02}")).collect::<Vec<_>>());
        assert!(w.truncated && w.files.is_empty() && w.dirs.is_empty());
        let whole = walk(&tree, 1000, ROOMY);
        assert_eq!((whole.links.len(), whole.truncated), (40, false));
    }

    /// Once the file limit is reached nothing more is examined: no later folder is opened or listed.
    #[test]
    fn the_walk_stops_at_the_first_file_over_the_file_limit() {
        let dir = tempfile::tempdir().unwrap();
        for i in 1..=5 {
            std::fs::write(dir.path().join(format!("a{i}.txt")), b"x").unwrap();
        }
        for d in 0..20 {
            std::fs::create_dir_all(dir.path().join(format!("zz/sub{d:02}"))).unwrap();
        }
        let w = walk(dir.path(), 3, ROOMY);
        assert_eq!(rels(&w), ["a1.txt", "a2.txt", "a3.txt"]);
        assert!(w.truncated);
        assert!(w.dirs.is_empty(), "the folders after the cut were never listed: {:?}", w.dirs);
        // A limit that isn't passed doesn't cut anything.
        let w = walk(dir.path(), 5, ROOMY);
        assert_eq!((w.files.len(), w.dirs.len(), w.truncated), (5, 21, false));
    }

    /// Cancelling stops the listing of one huge folder, not only the walk between folders.
    #[test]
    fn cancelling_stops_a_folder_while_it_is_being_listed() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..300 {
            std::fs::write(dir.path().join(format!("f{i:03}")), b"").unwrap();
        }
        let cancel = CancelToken::default();
        let all = list_within(names_in(dir.path()), 0, 1000, 1 << 20, &cancel).unwrap();
        assert_eq!((all.names.len(), all.cut), (300, false));
        // A folder that never ends, cancelled while its 301st entry is read: the listing stops at
        // its next check, holding no more than its limit meanwhile.
        let read = std::cell::Cell::new(0usize);
        let endless = (0u64..).map(|i| {
            read.set(read.get() + 1);
            if i == 300 {
                cancel.cancel();
            }
            Ok(std::ffi::OsString::from(format!("f{i:012}")))
        });
        let err = list_within(endless, 0, 100, 1 << 20, &cancel).err().unwrap();
        assert_eq!(err.code, ErrorCode::Cancelled);
        assert!((301..=513).contains(&read.get()), "read {} entries before stopping", read.get());
        let skip = |_: &str| false;
        let opts = WalkOptions { max_files: 1000, max_depth: 64, skip_dir: &skip, gitignore: false, include_hidden: true };
        assert_eq!(walk_dir(dir.path(), &opts, &cancel).unwrap_err().code, ErrorCode::Cancelled);
    }

    /// A folder whose entries keep failing to read ends its listing instead of going round forever.
    #[test]
    fn a_listing_that_keeps_failing_ends() {
        let read = std::cell::Cell::new(0usize);
        let failing = std::iter::repeat_with(|| {
            read.set(read.get() + 1);
            Err(std::io::Error::other("stale entry"))
        });
        let listing = list_within(failing, 0, 100, 1 << 20, &CancelToken::default()).unwrap();
        assert_eq!((listing.names.len(), listing.cut, read.get()), (0, false, 64));
        // One that fails now and then is read past the failures.
        let patchy = (0..500).map(|i| if i % 3 == 0 { Err(std::io::Error::other("stale entry")) } else { Ok(std::ffi::OsString::from(format!("f{i:03}"))) });
        assert_eq!(list_within(patchy, 0, 1000, 1 << 20, &CancelToken::default()).unwrap().names.len(), 333);
    }

    /// #294: a folder that couldn't be listed looked like an ordinary empty folder.
    #[cfg(unix)]
    #[test]
    fn a_folder_that_cant_be_listed_is_marked_and_leaves_coverage_partial() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("bundle");
        let closed = bundle.join("closed");
        std::fs::create_dir_all(&closed).unwrap();
        std::fs::write(closed.join("hidden.js"), "console.log('hidden');\n").unwrap();
        std::fs::write(bundle.join("visible.js"), "console.log('visible');\n").unwrap();
        let e = Engine::new(crate::testing::session());
        let readable = call(&e, "core_archive_show", json!({"path": bundle})).unwrap();
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(&closed).is_ok() {
            std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755)).unwrap();
            return; // Run as root: nothing is unreadable.
        }
        let child = call(&e, "core_archive_show", json!({"path": bundle}));
        let root = call(&e, "core_archive_show", json!({"path": closed}));
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755)).unwrap();
        let v = child.unwrap();
        assert_eq!(v["coverage"]["status"], "partial", "{v}");
        assert!(v["limitations"].as_array().unwrap().iter().any(|l| l["code"] == "unlisted_folders"), "{v}");
        let entry = v["result"]["entries"].as_array().unwrap().iter().find(|x| x["path"] == "closed").cloned().unwrap();
        assert!(entry["note"].as_str().is_some_and(|n| n.contains("couldn't be listed")), "{v}");
        assert!(root.is_err(), "a root folder that can't be listed is an error, not an empty inventory");
        assert_eq!(call(&e, "core_archive_show", json!({"path": bundle})).unwrap()["result"], readable["result"]);
    }
}
