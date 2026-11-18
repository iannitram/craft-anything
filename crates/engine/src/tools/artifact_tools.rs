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
    /// The tree may hold more than is listed here: a limit cut the listing short (files, depth, or
    /// the walk's [`WalkLimits`]), or some entries couldn't be read (their folders are in
    /// `unlisted`). The walk stops at the first file or entry over a limit, and nothing after the
    /// cut is read. A folder cut by a limit lists the entries the system returned first, so which
    /// ones a cut listing holds follows the system's own order: it is the same on the next walk
    /// only where the system lists an unchanged folder in the same order again.
    pub truncated: bool,
    /// Folders that couldn't be listed, or only in part, as (relative path, reason); the root is
    /// `.` (#294). A root that can't be listed at all is an error instead.
    pub unlisted: Vec<(String, String)>,
    /// `.gitignore` files that couldn't be read whole, as (relative path, reason), so their rules
    /// were skipped.
    pub skipped_gitignores: Vec<(String, String)>,
}

/// What a walk that was cut short says in a tool's notes.
pub(crate) const WALK_CUT_NOTE: &str = "the folder listing was cut short by a file, entry, path-length or depth limit, or by entries that couldn't be read";

/// A limitation naming the folders that couldn't be listed, or only in part, as (path, reason), if any (#294).
pub(crate) fn unlisted_limitation(unlisted: &[(String, String)]) -> Option<Limitation> {
    unlisted_note(unlisted).map(|note| Limitation::new("unlisted_folders", note))
}

/// What [`unlisted_limitation`] says, for a tool that reports in notes: the first folders by name,
/// each with its reason, and how many more there are.
pub(crate) fn unlisted_note(unlisted: &[(String, String)]) -> Option<String> {
    if unlisted.is_empty() {
        return None;
    }
    let first = unlisted.iter().take(5).map(|(rel, e)| format!("{rel} ({e})")).collect::<Vec<_>>().join(", ");
    let more = unlisted.len().saturating_sub(5);
    Some(format!(
        "{} folder(s) couldn't be listed in full, so some or all of what they hold is missing: {first}{}",
        unlisted.len(),
        if more > 0 { format!(" and {more} more") } else { String::new() }
    ))
}

/// The error for a walk that found nothing to work on: `what` alone when the walk saw every folder,
/// or with the folders it couldn't list and their reasons, since what was looked for may be in them.
pub(crate) fn nothing_found_error(what: String, unlisted: &[(String, String)]) -> ToolError {
    match unlisted_note(unlisted) {
        None => ToolError::invalid(what),
        Some(note) => {
            ToolError::invalid(format!("{what}; {note}")).with_remediation("Make those folders readable (or close what holds them open), then try again.")
        }
    }
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

/// A walk's `unlisted` folders named from `base`, the folder walked: one input among several.
pub(crate) fn unlisted_under(base: &str, unlisted: Vec<(String, String)>) -> impl Iterator<Item = (String, String)> + use<'_> {
    unlisted.into_iter().map(move |(rel, e)| (if rel == "." { base.to_string() } else { format!("{base}/{rel}") }, e))
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

/// Entries of one folder that couldn't be read, so they are missing from what the walk lists.
#[derive(Debug, Default)]
struct Unread {
    count: usize,
    /// The first one's error.
    first: Option<String>,
    /// So many in a row failed that the listing stopped there: the rest of the folder wasn't read.
    gave_up: bool,
}

impl Unread {
    fn note(&mut self, e: &std::io::Error) {
        self.count = self.count.saturating_add(1);
        if self.first.is_none() {
            self.first = Some(e.to_string());
        }
    }

    /// Why the folder is in [`Walked::unlisted`], if any of its entries couldn't be read.
    fn reason(&self) -> Option<String> {
        let first = self.first.as_deref()?;
        let stopped = if self.gave_up { " and its listing stopped there" } else { "" };
        Some(format!("{} of its entries couldn't be read{stopped}: {first}", self.count))
    }
}

/// One folder's entries as far as a walk's limits let it be read, sorted by name.
#[derive(Debug, Default)]
struct Listing {
    /// (name, name as the system has it).
    names: Vec<(String, std::ffi::OsString)>,
    /// The path bytes `names` uses against the limits.
    path_bytes: usize,
    /// The entries this listing uses of the walk's entry limit: the names kept and the entries
    /// that couldn't be read.
    examined: usize,
    /// A limit ended the listing before the folder did: the folder holds at least one entry more.
    cut: bool,
    /// The entries the system couldn't return.
    unread: Unread,
}

/// Most entries in a row that may fail to read before a folder's listing stops.
const FAILED_IN_A_ROW: usize = 64;

/// List a folder's entries within what is left of a walk's limits, sorted by name. The folder is
/// read only as far as the limits go: at most `max_entries` entries are taken from it, names and
/// failed reads alike, and one more, which only tells a folder that ends there from one that holds
/// more (`cut`); a name that would pass `max_path_bytes` is that last entry too. So a cut listing
/// holds the entries the system returned first, not the folder's first names in order: finding
/// those means reading a folder of any size to its end. Cancelling is looked at before every read.
fn list_within(
    mut entries: impl Iterator<Item = std::io::Result<std::ffi::OsString>>,
    prefix_len: usize,
    max_entries: usize,
    max_path_bytes: usize,
    cancel: &CancelToken,
) -> Result<Listing, ToolError> {
    let mut out = Listing::default();
    let mut failed_in_a_row = 0usize;
    loop {
        cancel.check()?;
        let Some(entry) = entries.next() else { break };
        if out.examined >= max_entries {
            out.cut = true;
            break;
        }
        match entry {
            Ok(raw) => {
                let name = raw.to_string_lossy().into_owned();
                let path_bytes = out.path_bytes.saturating_add(prefix_len).saturating_add(name.len());
                if path_bytes > max_path_bytes {
                    out.cut = true;
                    break;
                }
                failed_in_a_row = 0;
                out.examined = out.examined.saturating_add(1);
                out.path_bytes = path_bytes;
                out.names.push((name, raw));
            }
            // An entry the system couldn't return is counted and passed; one that keeps failing
            // ends the listing. Either way the folder wasn't seen whole, and `unread` says so.
            Err(e) => {
                out.examined = out.examined.saturating_add(1);
                out.unread.note(&e);
                failed_in_a_row = failed_in_a_row.saturating_add(1);
                if failed_in_a_row >= FAILED_IN_A_ROW {
                    out.unread.gave_up = true;
                    break;
                }
            }
        }
    }
    out.names.sort();
    Ok(out)
}

/// Walk a folder without following symbolic links. Bounded by file count and depth, and by
/// [`WalkLimits::for_files`] for everything else a tree can hold.
/// A root that can't be listed is an error; a folder below it that can't be is in `unlisted`, and
/// so is any folder, the root too, with entries that couldn't be read.
pub(crate) fn walk_dir(root: &Path, opts: &WalkOptions<'_>, cancel: &CancelToken) -> Result<Walked, ToolError> {
    walk_dir_within(root, opts, WalkLimits::for_files(opts.max_files), cancel)
}

/// [`walk_dir`] with the limits given. The walk stops at the first file over `max_files` and at
/// the first entry over `limits`: it takes at most `limits.max_entries` entries from the system's
/// folder listings and one more, so nothing is read, held or listed past a limit.
pub(crate) fn walk_dir_within(root: &Path, opts: &WalkOptions<'_>, limits: WalkLimits, cancel: &CancelToken) -> Result<Walked, ToolError> {
    walk_dir_reading(root, opts, limits, cancel, |dir| Ok(std::fs::read_dir(dir)?.map(|e| e.map(|e| e.file_name()))))
}

/// [`walk_dir_within`], taking each folder's entry names from `read_dir`: the system's listing, or
/// a test's that counts what is read from it.
fn walk_dir_reading<I: Iterator<Item = std::io::Result<std::ffi::OsString>>>(
    root: &Path,
    opts: &WalkOptions<'_>,
    limits: WalkLimits,
    cancel: &CancelToken,
    mut read_dir: impl FnMut(&Path) -> std::io::Result<I>,
) -> Result<Walked, ToolError> {
    let mut out = Walked::default();
    let mut ignore = craft_anything_analysis::js_source::Ignore::default();
    let mut stack: Vec<(PathBuf, String, usize)> = vec![(root.to_path_buf(), String::new(), 0)];
    let (mut entries_left, mut path_bytes_left) = (limits.max_entries, limits.max_path_bytes);
    while let Some((dir, rel, depth)) = stack.pop() {
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
        let entries = match read_dir(&dir) {
            Ok(e) => e,
            Err(e) if rel.is_empty() => return Err(ToolError::io(dir.display(), e)),
            // What this folder holds is missing from the listing, so the walk didn't see the tree whole.
            Err(e) => {
                out.truncated = true;
                out.unlisted.push((rel, e.to_string()));
                continue;
            }
        };
        let prefix_len = if rel.is_empty() { 0 } else { rel.len().saturating_add(1) };
        let listing = list_within(entries, prefix_len, entries_left, path_bytes_left, cancel)?;
        entries_left = entries_left.saturating_sub(listing.examined);
        path_bytes_left = path_bytes_left.saturating_sub(listing.path_bytes);
        let mut unread = listing.unread;
        let mut over_files = false;
        for (name, raw) in listing.names {
            cancel.check()?;
            let path = dir.join(&raw);
            let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if !opts.include_hidden && name.starts_with('.') && name != ".gitignore" {
                out.ignored += 1;
                continue;
            }
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                // Gone since the folder was listed: there is nothing to miss.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                // It is there, and what it is couldn't be read.
                Err(e) => {
                    unread.note(&e);
                    continue;
                }
            };
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
                    over_files = true;
                    break;
                }
                out.files.push(WalkedFile { rel: child_rel, abs: path, size: meta.len() });
            }
        }
        if let Some(reason) = unread.reason() {
            out.truncated = true;
            out.unlisted.push((if rel.is_empty() { ".".to_string() } else { rel }, reason));
        }
        if over_files || listing.cut {
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

/// What a folder walk found at one path, for listing its results in one order.
enum WalkEntry<'a> {
    Dir,
    Link,
    File(&'a WalkedFile),
}

/// [`inventory_path`], also returning the folders that couldn't be listed, or only in part, as (path, reason).
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
        // One list in path order, so `max_entries` cuts the tree where a listing of it would end.
        // Adding every folder and link first let them push the files out: a folder with five
        // files and thousands of subfolders read as holding none.
        let mut entries: Vec<(&str, WalkEntry<'_>)> = Vec::with_capacity(walked.dirs.len() + walked.links.len() + walked.files.len());
        entries.extend(walked.dirs.iter().map(|d| (d.as_str(), WalkEntry::Dir)));
        entries.extend(walked.links.iter().map(|l| (l.as_str(), WalkEntry::Link)));
        entries.extend(walked.files.iter().map(|f| (f.rel.as_str(), WalkEntry::File(f))));
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let mut total = 0u64;
        for (rel, entry) in entries {
            cancel.check()?;
            if inv.inv.entries.len() >= limits.max_entries {
                // Nothing after this point is listed, so nothing after it is read either.
                inv.inv.truncated = true;
                break;
            }
            match entry {
                WalkEntry::Dir => {
                    let failed = walked.unlisted.iter().find(|(r, _)| r == rel);
                    inv.add_marker(rel, "directory", 0, failed.map(|(_, e)| format!("couldn't be listed in full: {e}")));
                }
                WalkEntry::Link => inv.add_marker(rel, "link", 0, Some("symbolic link (not followed)".into())),
                WalkEntry::File(f) => {
                    if total.saturating_add(f.size) > limits.max_total_bytes || f.size > limits.max_entry_bytes {
                        inv.add_marker(rel, "unread", 0, Some("size budget reached".into()));
                        inv.inv.truncated = true;
                        continue;
                    }
                    match std::fs::read(&f.abs) {
                        Ok(b) => {
                            total += b.len() as u64;
                            inv.add_file(rel, &b, 0).map_err(|_| ToolError::cancelled())?;
                        }
                        Err(e) => inv.add_marker(rel, "unread", 0, Some(e.to_string())),
                    }
                }
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
        if inv.truncated { " (listing cut short)" } else { "" },
        inv.containers.len().saturating_sub(1),
        if inv.contradictions.is_empty() { String::new() } else { format!(", {} integrity contradiction(s)", inv.contradictions.len()) }
    );
    let mut lims = Vec::new();
    if inv.truncated {
        lims.push(Limitation::new(
            "truncated",
            "A size, count or depth limit stopped the inventory early, or part of a folder couldn't be read; raise max_entries, max_total_bytes or max_depth to see more of what a limit left out.",
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

/// For tests of what a tool says about a walk: a real folder that can't be listed.
#[cfg(test)]
pub(crate) mod walk_testing {
    use serde_json::Value;
    use std::path::Path;

    /// Keeps a folder impossible to list until it is dropped.
    pub(crate) struct Closed {
        #[cfg(unix)]
        path: std::path::PathBuf,
        #[cfg(windows)]
        _held: std::fs::File,
    }

    /// Make `dir` fail to list (or a file fail to read) for as long as the result lives: no
    /// permission on Unix, held without sharing on Windows. `None` where that stops nothing (a
    /// test run as root).
    pub(crate) fn close(dir: &Path) -> Option<Closed> {
        #[cfg(unix)]
        let closed = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).unwrap();
            Closed { path: dir.to_path_buf() }
        };
        #[cfg(windows)]
        let closed = {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_FLAG_BACKUP_SEMANTICS: what opening a folder takes.
            Closed { _held: std::fs::OpenOptions::new().read(true).share_mode(0).custom_flags(0x0200_0000).open(dir).unwrap() }
        };
        let stops = if dir.is_dir() { std::fs::read_dir(dir).is_err() } else { std::fs::read(dir).is_err() };
        // Only root on Unix lists a folder it has no permission for; a test that returns early
        // anywhere else would pass without having looked at anything.
        assert!(stops || cfg!(unix), "{} is held without sharing and can still be listed", dir.display());
        stops.then_some(closed)
    }

    #[cfg(unix)]
    impl Drop for Closed {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o755));
        }
    }

    /// The `unlisted_folders` limitation of a tool's evidence, if it has one.
    pub(crate) fn unlisted_message(evidence: &Value) -> Option<String> {
        let limitations = evidence["limitations"].as_array()?;
        limitations.iter().find(|l| l["code"] == "unlisted_folders").and_then(|l| l["message"].as_str()).map(str::to_string)
    }

    /// The evidence says it is partial and names `folder` with the system's reason for it.
    pub(crate) fn assert_names_unlisted(evidence: &Value, folder: &str) {
        assert_eq!(evidence["coverage"]["status"], "partial", "{evidence}");
        let message = unlisted_message(evidence).unwrap_or_else(|| panic!("no unlisted_folders limitation: {evidence}"));
        assert!(message.contains(&format!("{folder} (")) && message.contains("os error"), "{message}");
    }
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

    /// A folder's inventory used to add every folder and link before any file, so a small
    /// `max_entries` filled up with folders and the files beside them were never listed.
    #[test]
    fn a_folder_inventory_is_cut_in_path_order_not_folders_first() {
        let dir = tempfile::tempdir().unwrap();
        for i in 1..=5 {
            std::fs::write(dir.path().join(format!("a{i}.txt")), format!("file {i}")).unwrap();
        }
        for d in 0..20 {
            std::fs::create_dir_all(dir.path().join("zz").join(format!("sub{d:02}"))).unwrap();
        }
        let limits = InvLimits { max_entries: 10, ..InvLimits::default() };
        let (inv, _) = inventory_path(dir.path(), limits, &CancelToken::default()).unwrap();
        let paths: Vec<&str> = inv.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["a1.txt", "a2.txt", "a3.txt", "a4.txt", "a5.txt", "zz", "zz/sub00", "zz/sub01", "zz/sub02", "zz/sub03"]);
        assert_eq!(inv.entries.iter().filter(|e| e.sha256.is_some()).count(), 5, "every file is read and hashed");
        assert!(inv.truncated);
        // Through the tool, as the person meets it: the summary counts the files.
        let e = Engine::new(crate::testing::session());
        let out = call(&e, "core_archive_show", json!({"path": dir.path(), "max_entries": 10})).unwrap();
        assert_eq!(out["result"]["files"], 5, "{}", out["summary"]);
        assert_eq!(out["result"]["truncated"], true);
        // A limit the tree fits in lists all of it, in the same order, and isn't cut.
        let (whole, _) = inventory_path(dir.path(), InvLimits { max_entries: 26, ..InvLimits::default() }, &CancelToken::default()).unwrap();
        assert_eq!(whole.entries.len(), 26);
        assert_eq!(whole.entries.iter().map(|e| e.path.as_str()).take(10).collect::<Vec<_>>(), paths);
        assert!(!whole.truncated);
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

    type Names = Box<dyn Iterator<Item = std::io::Result<std::ffi::OsString>>>;

    fn walk(root: &Path, max_files: usize, limits: WalkLimits) -> Walked {
        let skip = |_: &str| false;
        let opts = WalkOptions { max_files, max_depth: 64, skip_dir: &skip, gitignore: false, include_hidden: true };
        walk_dir_within(root, &opts, limits, &CancelToken::default()).unwrap()
    }

    /// [`walk`], with what `add` returns after each folder's own entries, and the number of entries
    /// the walk took from the folder listings in all.
    fn walk_adding(root: &Path, max_files: usize, limits: WalkLimits, add: impl Fn(&Path) -> Names) -> (Walked, usize) {
        let skip = |_: &str| false;
        let opts = WalkOptions { max_files, max_depth: 64, skip_dir: &skip, gitignore: false, include_hidden: true };
        let read = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let walked = walk_dir_reading(root, &opts, limits, &CancelToken::default(), |dir| {
            let read = read.clone();
            let names: Names = Box::new(names_in(dir).chain(add(dir)).inspect(move |_| read.set(read.get() + 1)));
            Ok(names)
        })
        .unwrap();
        (walked, read.get())
    }

    /// [`walk`], and the number of entries it took from the system's folder listings.
    fn walk_counting(root: &Path, max_files: usize, limits: WalkLimits) -> (Walked, usize) {
        walk_adding(root, max_files, limits, |_| Box::new(std::iter::empty()))
    }

    fn rels(w: &Walked) -> Vec<&str> {
        w.files.iter().map(|f| f.rel.as_str()).collect()
    }

    fn names_in(dir: &Path) -> impl Iterator<Item = std::io::Result<std::ffi::OsString>> + use<> {
        std::fs::read_dir(dir).unwrap().map(|e| e.map(|e| e.file_name()))
    }

    fn named(name: impl Into<String>) -> std::io::Result<std::ffi::OsString> {
        Ok(std::ffi::OsString::from(name.into()))
    }

    fn failed() -> std::io::Result<std::ffi::OsString> {
        Err(std::io::Error::other("stale entry"))
    }

    #[test]
    fn a_walk_within_its_limits_lists_the_whole_tree_in_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub/empty")).unwrap();
        for f in ["b.txt", "a.txt", "sub/c.txt"] {
            std::fs::write(dir.path().join(f), f).unwrap();
        }
        let (w, read) = walk_counting(dir.path(), 10, ROOMY);
        assert_eq!(rels(&w), ["a.txt", "b.txt", "sub/c.txt"]);
        assert_eq!(w.dirs, ["sub", "sub/empty"]);
        assert!(!w.truncated && w.links.is_empty() && w.unlisted.is_empty());
        assert_eq!(read, 5);
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

    /// A folder is read only as far as its limits go: the entries it may keep and one more, which
    /// tells a folder that ends there from one that holds more. It used to be read to its end
    /// (100,000 entries for a limit of 2) to find its first names in order.
    #[test]
    fn a_listing_reads_one_entry_past_its_limit_and_no_further() {
        // Names of 7 bytes each. (entry limit, byte limit, names kept)
        for (max_entries, max_bytes, kept) in [(2, 1 << 20, 2), (0, 1 << 20, 0), (1000, 35, 5), (1000, 0, 0), (7, 49, 7), (7, 48, 6)] {
            let read = std::cell::Cell::new(0usize);
            let many = (0..100_000usize).rev().map(|i| {
                read.set(read.get() + 1);
                named(format!("f{i:06}"))
            });
            let listing = list_within(many, 0, max_entries, max_bytes, &CancelToken::default()).unwrap();
            assert_eq!(read.get(), kept + 1, "limits {max_entries}/{max_bytes}: entries read from a folder of 100,000");
            // The first ones returned (the highest numbers here), sorted by name.
            let want: Vec<String> = (100_000 - kept..100_000).map(|i| format!("f{i:06}")).collect();
            assert_eq!(listing.names.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), want);
            assert_eq!((listing.examined, listing.path_bytes, listing.cut), (kept, kept * 7, true));
            assert!(listing.unread.reason().is_none());
        }
        // A folder that ends at the limit isn't cut, and no entry past its end is asked for twice.
        let read = std::cell::Cell::new(0usize);
        let two = ["b", "a"].into_iter().map(|n| {
            read.set(read.get() + 1);
            named(n)
        });
        let listing = list_within(two, 0, 2, 2, &CancelToken::default()).unwrap();
        assert_eq!((listing.names.len(), listing.cut, read.get()), (2, false, 2));
    }

    /// A listing that isn't cut is in name order whatever order the system lists the folder in. A
    /// cut one holds the entries the system returned first, in name order: which entries those are
    /// follows the system's order, for the entry limit and the byte limit alike.
    #[test]
    fn a_listing_is_in_name_order_and_a_cut_one_holds_the_entries_read_first() {
        let names = ["delta", "a", "charlie-with-a-long-name", "bb", "echo", "b", "alpha", "zz", "c"];
        // All nine take 63 bytes with their 2-byte prefix: the last two cases are all, and all but one.
        for (max_entries, max_bytes) in [(4, 1000), (1000, 12), (3, 7), (0, 1000), (1000, 0), (1000, 1000), (9, 63), (9, 62)] {
            for rotation in 0..names.len() {
                for reversed in [false, true] {
                    let mut order = names.to_vec();
                    order.rotate_left(rotation);
                    if reversed {
                        order.reverse();
                    }
                    // The answer: the longest run from the start of that order that fits both limits.
                    let mut want: Vec<&str> = Vec::new();
                    let mut bytes = 0;
                    for n in &order {
                        if want.len() + 1 > max_entries || bytes + 2 + n.len() > max_bytes {
                            break;
                        }
                        bytes += 2 + n.len();
                        want.push(n);
                    }
                    let cut = want.len() < names.len();
                    let read = std::cell::Cell::new(0usize);
                    let offered = order.iter().map(|n| {
                        read.set(read.get() + 1);
                        named(*n)
                    });
                    let listing = list_within(offered, 2, max_entries, max_bytes, &CancelToken::default()).unwrap();
                    assert_eq!(read.get(), want.len() + usize::from(cut), "limits {max_entries}/{max_bytes}, order {order:?}");
                    want.sort();
                    let got: Vec<&str> = listing.names.iter().map(|(n, _)| n.as_str()).collect();
                    assert_eq!(got, want, "limits {max_entries}/{max_bytes}, order {order:?}");
                    assert_eq!((listing.path_bytes, listing.examined, listing.cut), (bytes, want.len(), cut));
                    assert!(listing.unread.reason().is_none());
                }
            }
        }
    }

    /// A folder with more entries than the walk may examine: the walk takes the entries it may
    /// keep and one more from the system, however many the folder holds.
    #[test]
    fn a_huge_folder_is_read_only_up_to_the_entry_limit() {
        let dir = tempfile::tempdir().unwrap();
        for i in (0..300).rev() {
            std::fs::write(dir.path().join(format!("f{i:03}.txt")), b"x").unwrap();
        }
        let all: std::collections::BTreeSet<String> = (0..300).map(|i| format!("f{i:03}.txt")).collect();
        let (w, read) = walk_counting(dir.path(), 1000, WalkLimits { max_entries: 50, max_path_bytes: 1 << 20 });
        assert_eq!((w.files.len(), w.truncated, read), (50, true, 51));
        assert!(rels(&w).is_sorted() && rels(&w).iter().all(|r| all.contains(*r)), "{:?}", rels(&w));
        assert!(w.unlisted.is_empty(), "a limit isn't a folder that couldn't be read");
        // The path-byte limit bounds it the same way: 8 bytes a name, so 12 fit in 100.
        let (by_bytes, read) = walk_counting(dir.path(), 1000, WalkLimits { max_entries: 1000, max_path_bytes: 100 });
        assert_eq!((by_bytes.files.len(), by_bytes.truncated, read), (12, true, 13));
        assert!(rels(&by_bytes).is_sorted() && rels(&by_bytes).iter().all(|r| all.contains(*r)));
        // Within its limits the folder is read once, whole, and listed in name order.
        let (whole, read) = walk_counting(dir.path(), 1000, ROOMY);
        assert_eq!((rels(&whole), whole.truncated, read), (all.iter().map(String::as_str).collect(), false, 300));
        // A folder below the root is charged its own path too ("deep/" + 8 bytes): "deep", then
        // three names that fit and the one that doesn't.
        let nested = tempfile::tempdir().unwrap();
        std::fs::create_dir(nested.path().join("deep")).unwrap();
        for i in 0..20 {
            std::fs::write(nested.path().join("deep").join(format!("f{i:03}.txt")), b"x").unwrap();
        }
        let (w, read) = walk_counting(nested.path(), 1000, WalkLimits { max_entries: 1000, max_path_bytes: 4 + 3 * 13 });
        assert_eq!((w.files.len(), w.truncated, read), (3, true, 1 + 4));
        assert!(rels(&w).is_sorted() && rels(&w).iter().all(|r| r.starts_with("deep/f0")), "{:?}", rels(&w));
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
        // Ten of the root's thirty, and nothing below them: eleven entries read in all.
        let (w, read) = walk_counting(dir.path(), 1000, WalkLimits { max_entries: 10, max_path_bytes: 1 << 20 });
        assert_eq!((w.dirs.len(), w.truncated, read), (10, true, 11));
        assert!(w.dirs.is_sorted() && w.dirs.iter().all(|d| d.len() == 3 && d.starts_with('d')), "{:?}", w.dirs);
        assert!(w.files.is_empty());
        // 30 at the root, d29's three, then two of d28's and the one that says d28 holds more: 36
        // read, and the rest never opened.
        let (w, read) = walk_counting(dir.path(), 1000, WalkLimits { max_entries: 35, max_path_bytes: 1 << 20 });
        assert_eq!((w.dirs.len(), w.truncated, read), (35, true, 36));
        assert!(w.dirs.iter().rev().take(2).all(|d| d.starts_with("d28/e")), "{:?}", w.dirs);
        let (whole, read) = walk_counting(dir.path(), 1000, WalkLimits { max_entries: 121, max_path_bytes: 1 << 20 });
        assert_eq!((whole.dirs.len(), whole.truncated, read), (120, false, 120));
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
        let (w, read) = walk_counting(&tree, 1000, WalkLimits { max_entries: 15, max_path_bytes: 1 << 20 });
        assert_eq!((w.links.len(), w.truncated, read), (15, true, 16));
        assert!(w.links.is_sorted() && w.links.iter().all(|l| l.len() == 3 && l.starts_with('l')), "{:?}", w.links);
        assert!(w.files.is_empty() && w.dirs.is_empty());
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
        // The root's six entries are read (they fit the entry limit); "zz" is never opened.
        let (w, read) = walk_counting(dir.path(), 3, ROOMY);
        assert_eq!(rels(&w), ["a1.txt", "a2.txt", "a3.txt"]);
        assert!(w.truncated);
        assert!(w.dirs.is_empty(), "the folders after the cut were never listed: {:?}", w.dirs);
        assert_eq!(read, 6);
        // A limit that isn't passed doesn't cut anything.
        let w = walk(dir.path(), 5, ROOMY);
        assert_eq!((w.files.len(), w.dirs.len(), w.truncated), (5, 21, false));
    }

    /// Cancelling stops the listing of one huge folder, not only the walk between folders: no
    /// entry is read once it is cancelled, the one past a limit included.
    #[test]
    fn cancelling_stops_a_folder_while_it_is_being_listed() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..300 {
            std::fs::write(dir.path().join(format!("f{i:03}")), b"").unwrap();
        }
        let cancel = CancelToken::default();
        let all = list_within(names_in(dir.path()), 0, 1000, 1 << 20, &cancel).unwrap();
        assert_eq!((all.names.len(), all.cut), (300, false));
        // A folder that never ends, cancelled while its 301st entry is read: that is the last one read.
        let read = std::cell::Cell::new(0usize);
        let endless = (0u64..).map(|i| {
            read.set(read.get() + 1);
            if i == 300 {
                cancel.cancel();
            }
            named(format!("f{i:012}"))
        });
        let err = list_within(endless, 0, 1_000_000, 1 << 30, &cancel).err().unwrap();
        assert_eq!((err.code, read.get()), (ErrorCode::Cancelled, 301));
        // Cancelled before it starts, nothing is read at all.
        read.set(0);
        let endless = std::iter::repeat_with(|| {
            read.set(read.get() + 1);
            named("f")
        });
        assert_eq!(list_within(endless, 0, 1000, 1 << 20, &cancel).err().unwrap().code, ErrorCode::Cancelled);
        assert_eq!(read.get(), 0);
        let skip = |_: &str| false;
        let opts = WalkOptions { max_files: 1000, max_depth: 64, skip_dir: &skip, gitignore: false, include_hidden: true };
        assert_eq!(walk_dir(dir.path(), &opts, &cancel).unwrap_err().code, ErrorCode::Cancelled);
        // Cancelled while the last entry within the limit is read: the one past the limit isn't.
        let cancel = CancelToken::default();
        let endless = (0u64..).map(|i| {
            read.set(read.get() + 1);
            if i == 1 {
                cancel.cancel();
            }
            named(format!("f{i:012}"))
        });
        let err = list_within(endless, 0, 2, 1 << 20, &cancel).err().unwrap();
        assert_eq!((err.code, read.get()), (ErrorCode::Cancelled, 2));
    }

    /// A folder whose entries keep failing to read ends its listing instead of going round for
    /// ever, and says it wasn't seen whole. It used to come back as an empty folder listed in full.
    #[test]
    fn a_listing_that_keeps_failing_ends_and_says_what_it_missed() {
        let cancel = CancelToken::default();
        let read = std::cell::Cell::new(0usize);
        let failing = std::iter::repeat_with(|| {
            read.set(read.get() + 1);
            failed()
        });
        let listing = list_within(failing, 0, 100, 1 << 20, &cancel).unwrap();
        assert_eq!((listing.names.len(), listing.examined, read.get()), (0, 64, 64));
        assert_eq!((listing.unread.count, listing.unread.gave_up, listing.cut), (64, true, false));
        assert_eq!(listing.unread.reason().as_deref(), Some("64 of its entries couldn't be read and its listing stopped there: stale entry"));
        // Failed entries use the entry limit as names do, so a nearer limit ends it first.
        read.set(0);
        let failing = std::iter::repeat_with(|| {
            read.set(read.get() + 1);
            failed()
        });
        let listing = list_within(failing, 0, 10, 1 << 20, &cancel).unwrap();
        assert_eq!((listing.examined, listing.unread.count, listing.unread.gave_up, listing.cut, read.get()), (10, 10, false, true, 11));
        // One that fails now and then is read past the failures, and each one is counted.
        read.set(0);
        let patchy = |read: &std::cell::Cell<usize>, i: usize| {
            read.set(read.get() + 1);
            if i.is_multiple_of(3) { failed() } else { named(format!("f{i:03}")) }
        };
        let listing = list_within((0..500).map(|i| patchy(&read, i)), 0, 1000, 1 << 20, &cancel).unwrap();
        assert_eq!((listing.names.len(), listing.unread.count, listing.examined, read.get()), (333, 167, 500, 500));
        assert_eq!((listing.unread.gave_up, listing.cut), (false, false));
        assert_eq!(listing.unread.reason().as_deref(), Some("167 of its entries couldn't be read: stale entry"));
        // The same folder under a limit of 100: 34 failures and 66 names, then the one read past it.
        read.set(0);
        let listing = list_within((0..500).map(|i| patchy(&read, i)), 0, 100, 1 << 20, &cancel).unwrap();
        assert_eq!((listing.names.len(), listing.unread.count, listing.examined, listing.cut, read.get()), (66, 34, 100, true, 101));
        // 63 failures, a name, 63 more: never 64 in a row, so it is read to its end.
        read.set(0);
        let nearly = (0..127).map(|i| {
            read.set(read.get() + 1);
            if i == 63 { named("f") } else { failed() }
        });
        let listing = list_within(nearly, 0, 1000, 1 << 20, &cancel).unwrap();
        assert_eq!((listing.names.len(), listing.unread.count, listing.unread.gave_up, read.get()), (1, 126, false, 127));
    }

    /// Entries that couldn't be read leave the walk marked incomplete and name their folder: what
    /// was read is kept, the walk goes on to the other folders, and nothing reads as a whole tree.
    #[test]
    fn entries_that_cant_be_read_leave_the_walk_marked_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["a.txt", "ok/d.txt", "sub/b.txt", "sub/c.txt"] {
            std::fs::create_dir_all(dir.path().join(f).parent().unwrap()).unwrap();
            std::fs::write(dir.path().join(f), f).unwrap();
        }
        let root = dir.path().to_path_buf();
        let everything = ["a.txt", "ok/d.txt", "sub/b.txt", "sub/c.txt"];
        let (whole, read) = walk_counting(&root, 10, ROOMY);
        assert_eq!((rels(&whole), whole.truncated, whole.unlisted.is_empty(), read), (everything.to_vec(), false, true, 6));
        // Two of sub's entries can't be read: its two names are kept, and it is named as incomplete.
        let (w, read) =
            walk_adding(&root, 10, ROOMY, |d| if d.ends_with("sub") { Box::new([failed(), failed()].into_iter()) } else { Box::new(std::iter::empty()) });
        assert_eq!((rels(&w), w.truncated, read), (everything.to_vec(), true, 8));
        assert_eq!(w.unlisted, [("sub".to_string(), "2 of its entries couldn't be read: stale entry".to_string())]);
        let limitation = unlisted_limitation(&w.unlisted).unwrap();
        assert_eq!(limitation.code, "unlisted_folders");
        assert!(
            limitation.message.contains("couldn't be listed in full") && limitation.message.contains("sub (2 of its entries couldn't be read"),
            "{}",
            limitation.message
        );
        // sub never stops failing: its listing ends after 64 in a row, and "ok" is still listed after it.
        let (w, read) =
            walk_adding(&root, 10, ROOMY, |d| if d.ends_with("sub") { Box::new(std::iter::repeat_with(failed)) } else { Box::new(std::iter::empty()) });
        assert_eq!((rels(&w), w.truncated, read), (everything.to_vec(), true, 3 + (2 + 64) + 1));
        assert_eq!(w.unlisted, [("sub".to_string(), "64 of its entries couldn't be read and its listing stopped there: stale entry".to_string())]);
        // The file limit ending the walk inside that folder doesn't lose the mark.
        let (w, _) = walk_adding(&root, 2, ROOMY, |d| if d.ends_with("sub") { Box::new([failed()].into_iter()) } else { Box::new(std::iter::empty()) });
        assert_eq!((rels(&w), w.truncated), (vec!["a.txt", "sub/b.txt"], true));
        assert_eq!(w.unlisted, [("sub".to_string(), "1 of its entries couldn't be read: stale entry".to_string())]);
        // The root's own entries: it is named ".".
        let at_root = root.clone();
        let (w, _) = walk_adding(&root, 10, ROOMY, move |d| if d == at_root { Box::new([failed()].into_iter()) } else { Box::new(std::iter::empty()) });
        assert_eq!((rels(&w), w.truncated), (everything.to_vec(), true));
        assert_eq!(w.unlisted, [(".".to_string(), "1 of its entries couldn't be read: stale entry".to_string())]);
        // A name that is gone by the time it is looked at is nothing missed; one that is there
        // but can't be looked at (no system takes a name with a zero byte) is.
        let at_root = root.clone();
        let (w, _) =
            walk_adding(&root, 10, ROOMY, move |d| if d == at_root { Box::new([named("gone.txt")].into_iter()) } else { Box::new(std::iter::empty()) });
        assert_eq!((rels(&w), w.truncated, w.unlisted.is_empty()), (everything.to_vec(), false, true));
        let at_root = root.clone();
        let (w, _) =
            walk_adding(&root, 10, ROOMY, move |d| if d == at_root { Box::new([named("bad\0name")].into_iter()) } else { Box::new(std::iter::empty()) });
        assert_eq!((rels(&w), w.truncated, w.unlisted.len()), (everything.to_vec(), true, 1));
        assert!(w.unlisted.iter().all(|(rel, why)| rel == "." && why.starts_with("1 of its entries couldn't be read: ")), "{:?}", w.unlisted);
    }

    /// A folder below the root that can't be listed at all leaves the walk marked as cut short, as
    /// entries that can't be read do: it used to be named in `unlisted` with `truncated` still
    /// false, so a caller reading only `truncated` took the listing for the whole tree.
    #[test]
    fn a_folder_that_cant_be_listed_leaves_the_walk_marked_as_cut_short() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["a.txt", "ok/d.txt", "sub/b.txt"] {
            std::fs::create_dir_all(dir.path().join(f).parent().unwrap()).unwrap();
            std::fs::write(dir.path().join(f), f).unwrap();
        }
        let skip = |_: &str| false;
        let opts = WalkOptions { max_files: 10, max_depth: 64, skip_dir: &skip, gitignore: false, include_hidden: true };
        // The system refuses to list "sub", whatever the reason.
        let refused = walk_dir_reading(dir.path(), &opts, ROOMY, &CancelToken::default(), |d| {
            if d.ends_with("sub") { Err(std::io::Error::other("no entry")) } else { Ok(names_in(d)) }
        })
        .unwrap();
        assert_eq!((rels(&refused), refused.truncated), (vec!["a.txt", "ok/d.txt"], true));
        assert_eq!(refused.unlisted, [("sub".to_string(), "no entry".to_string())]);
        // The same with a real folder the system won't list.
        let Some(closed) = walk_testing::close(&dir.path().join("sub")) else { return };
        let real = walk_dir(dir.path(), &opts, &CancelToken::default()).unwrap();
        drop(closed);
        assert_eq!((rels(&real), real.truncated, real.unlisted.len()), (vec!["a.txt", "ok/d.txt"], true, 1));
        assert!(real.unlisted.iter().all(|(rel, why)| rel == "sub" && why.contains("os error")), "{:?}", real.unlisted);
        assert_eq!(real.dirs, ["ok", "sub"]);
        // Listed again once it can be.
        let whole = walk_dir(dir.path(), &opts, &CancelToken::default()).unwrap();
        assert_eq!((rels(&whole), whole.truncated, whole.unlisted.is_empty()), (vec!["a.txt", "ok/d.txt", "sub/b.txt"], false, true));
        assert_eq!(
            unlisted_under("dist", vec![(".".into(), "x".into()), ("sub".into(), "y".into())]).collect::<Vec<_>>(),
            [("dist".to_string(), "x".to_string()), ("dist/sub".to_string(), "y".to_string())]
        );
    }

    /// The same on every system the tests run on: `core_archive_show` names the folder it couldn't list.
    #[test]
    fn an_inventory_names_a_folder_it_couldnt_list() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("bundle");
        std::fs::create_dir_all(bundle.join("closed")).unwrap();
        std::fs::write(bundle.join("closed/hidden.js"), "console.log('hidden');\n").unwrap();
        std::fs::write(bundle.join("visible.js"), "console.log('visible');\n").unwrap();
        let e = Engine::new(crate::testing::session());
        let readable = call(&e, "core_archive_show", json!({"path": bundle})).unwrap();
        assert_eq!((readable["result"]["truncated"].clone(), walk_testing::unlisted_message(&readable)), (json!(false), None));
        let Some(closed) = walk_testing::close(&bundle.join("closed")) else { return };
        let v = call(&e, "core_archive_show", json!({"path": bundle}));
        drop(closed);
        let v = v.unwrap();
        walk_testing::assert_names_unlisted(&v, "closed");
        assert_eq!(v["result"]["truncated"], true, "{v}");
        assert!(v["limitations"].as_array().unwrap().iter().any(|l| l["code"] == "truncated"), "{v}");
        assert!(v["summary"].as_str().unwrap().contains("listing cut short"), "{v}");
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
