//! File progress kept between builder turns and runs. File contents, rather than timestamps,
//! decide whether the app changed; a successful write of identical bytes is not progress.

use super::{OWN_NAMES, Workspace, rebuilt};
use crate::CancelToken;
use craft_anything_evidence::{ErrorCode, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A builder gets two consecutive turns without changing the app before it needs attention.
pub const UNCHANGED_TURN_LIMIT: u32 = 2;
// Keep state inside an already reserved Craft directory. A project created by an older
// version may have its own app data named build-progress.json at the root.
pub const BUILD_PROGRESS_FILE: &str = "inbox/build-progress.json";
pub const BUILD_PROGRESS_VERSION: u32 = 2;
const MAX_RECORD_BYTES: u64 = 16 * 1024;
const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 64;

const STEPS: &[crate::versioned::Step<Value>] = &[v0_to_v1, v1_to_v2];
const _: () = assert!(STEPS.len() == BUILD_PROGRESS_VERSION as usize);

fn v0_to_v1(doc: &mut Value) -> Result<(), String> {
    if doc.is_object() { Ok(()) } else { Err("the progress record isn't a JSON object".into()) }
}

/// Version 2 adds `checked`; a version 1 record has no kept check.
fn v1_to_v2(doc: &mut Value) -> Result<(), String> {
    v0_to_v1(doc)
}

/// The app as the builder last left it, and turns since it last changed. Only the rebuild run
/// writes this record. An older project without one has no counted turns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildProgress {
    pub version: u32,
    #[serde(default)]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub unchanged_turns: u32,
    /// The last check round in which every check worked, and what it looked at; while that
    /// stays the same, the round's results stand instead of building and starting the app again.
    #[serde(default)]
    pub checked: Option<CheckedBuild>,
}

/// A check round kept with the state it judged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckedBuild {
    /// A digest of what the round's checks look at, taken after they ran.
    pub key: String,
    /// The round in `craft.json`'s `checks` whose results stand.
    pub round: u32,
}

impl Default for BuildProgress {
    fn default() -> Self {
        Self { version: BUILD_PROGRESS_VERSION, fingerprint: None, unchanged_turns: 0, checked: None }
    }
}

impl BuildProgress {
    /// Read a project's progress. Broken or newer records stop the run rather than quietly
    /// giving it another budget. Version 0 has the same shape; version 1 adds the version number.
    pub fn load(root: &Path) -> Result<Self, ToolError> {
        let path = root.join(BUILD_PROGRESS_FILE);
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(ToolError::io(path.display(), e)),
        };
        let mut bytes = Vec::new();
        file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes).map_err(|e| ToolError::io(path.display(), e))?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(ToolError::new(ErrorCode::OverLimit, format!("{BUILD_PROGRESS_FILE} is too large to read.")));
        }
        let mut doc: Value = serde_json::from_slice(&bytes).map_err(|e| not_ours(format!("Couldn't read {BUILD_PROGRESS_FILE}: {e}")))?;
        let found = match doc.get("version") {
            None => 0,
            Some(v) => v.as_u64().and_then(|v| u32::try_from(v).ok()).ok_or_else(|| not_ours(format!("{BUILD_PROGRESS_FILE} has an invalid version.")))?,
        };
        if found > BUILD_PROGRESS_VERSION {
            return Err(ToolError::unsupported(format!("{BUILD_PROGRESS_FILE} was written by a newer Craft Anything, so this build is left alone."))
                .with_remediation("Update Craft Anything to continue this build."));
        }
        let version =
            crate::versioned::upgrade(&mut doc, found, 0, STEPS).map_err(|e| not_ours(format!("Couldn't bring {BUILD_PROGRESS_FILE} up to date: {e}")))?;
        let obj = doc.as_object_mut().ok_or_else(|| not_ours(format!("{BUILD_PROGRESS_FILE} isn't a JSON object.")))?;
        obj.insert("version".into(), Value::from(version));
        serde_json::from_value(doc).map_err(|e| not_ours(format!("Couldn't read {BUILD_PROGRESS_FILE}: {e}")))
    }

    pub fn save(&self, root: &Path) -> Result<(), ToolError> {
        if self.version > BUILD_PROGRESS_VERSION {
            return Err(ToolError::unsupported("The newer build progress record can't be overwritten."));
        }
        // Recheck on disk: a run must not overwrite a newer record written since it started.
        Self::load(root)?;
        let mut current = self.clone();
        current.version = BUILD_PROGRESS_VERSION;
        let bytes = serde_json::to_vec_pretty(&current).map_err(|e| ToolError::internal(e.to_string()))?;
        crate::project::write_atomic(&root.join(BUILD_PROGRESS_FILE), &bytes)
    }

    /// A fresh request starts a new attempt; a plain retry keeps the existing count.
    pub fn reset(&mut self) {
        self.unchanged_turns = 0;
    }

    /// An unavailable scan breaks the comparison streak. A later readable run needs a new
    /// baseline, not a comparison with files from before the unobserved turns.
    pub fn invalidate(&mut self) {
        self.fingerprint = None;
        self.reset();
    }

    /// An edit made between turns or while the project was closed is progress too.
    pub fn before_turn(&mut self, fingerprint: String) {
        if self.fingerprint.as_deref() != Some(fingerprint.as_str()) {
            self.reset();
        }
        self.fingerprint = Some(fingerprint);
    }

    /// Count only completed builder turns. Checks and conversation records never count as a turn.
    pub fn after_turn(&mut self, fingerprint: String) {
        self.unchanged_turns = if self.fingerprint.as_deref() == Some(fingerprint.as_str()) { self.unchanged_turns.saturating_add(1) } else { 0 };
        self.fingerprint = Some(fingerprint);
    }

    pub fn stalled(&self) -> bool {
        self.unchanged_turns >= UNCHANGED_TURN_LIMIT
    }
}

/// A progress record this version can't read and didn't write: something else changed it (a
/// builder once kept its own notes there). The run still stops, so a builder can't hand itself a
/// fresh count, but the person is told what happened and how to go on.
fn not_ours(message: String) -> ToolError {
    let message = message.trim_end_matches('.');
    ToolError::invalid(format!(
        "{message}. This file is Craft Anything's record of the build, and something else changed it (a builder may have kept its own notes there)."
    ))
    .with_remediation(format!(
        "Rename or delete {BUILD_PROGRESS_FILE} in the project's folder, then try again: the count of builder turns that changed nothing starts over."
    ))
}

/// The published schema of the progress record (`cargo xtask schemas`).
pub fn build_progress_schema() -> String {
    let schema = schemars::generate::SchemaSettings::draft2020_12().into_generator().into_root_schema_for::<BuildProgress>();
    serde_json::to_string_pretty(&schema).map(|s| s + "\n").unwrap_or_default()
}

impl Workspace {
    /// A stable digest of the app's file names and contents. Dependency and build caches follow
    /// the same exclusions as project copies; Craft's own records and git metadata are left out.
    /// Symlinks are recorded as links, never followed. Reads are streamed and cancellable.
    pub fn app_fingerprint(&self, stop: &CancelToken) -> Result<String, ToolError> {
        AppFingerprintCache::default().scan(self, stop)
    }

    /// A digest of what a check round looks at: the app's files (as [`Workspace::app_fingerprint`]
    /// reads them), the names, sizes and times of the files in `reference/` and `capture/` it
    /// compares with, and `extra` (the folder, kind, stack, mode and plugin checks, from the caller).
    /// Equal digests mean the same round would judge the same build against the same things.
    pub fn check_fingerprint(&self, cache: &mut AppFingerprintCache, extra: &[&str], stop: &CancelToken) -> Result<String, ToolError> {
        let mut hash = Sha256::new();
        hash.update(b"craft-check-inputs-v1\0");
        hash.update(cache.scan(self, stop)?.as_bytes());
        for dir in [self.reference_dir(), self.capture_dir()] {
            hash.update(b"\0dir\0");
            let mut visited = 0;
            stamp_tree(&dir, &dir, &mut hash, &mut visited, 0, stop)?;
        }
        for e in extra {
            hash.update(b"\0extra\0");
            hash.update(e.as_bytes());
        }
        Ok(hex::encode(hash.finalize()))
    }
}

/// Names, sizes, kinds and modification times under `dir`, in name order; a missing folder adds
/// nothing. Bounded like the app scan, and symlinks are recorded, never followed.
fn stamp_tree(root: &Path, dir: &Path, hash: &mut Sha256, visited: &mut usize, depth: usize, stop: &CancelToken) -> Result<(), ToolError> {
    if depth > MAX_DEPTH {
        return Err(ToolError::new(ErrorCode::OverLimit, format!("{} is nested too deeply to compare.", root.display())));
    }
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && depth == 0 => return Ok(()),
        Err(e) => return Err(ToolError::io(dir.display(), e)),
    };
    let mut entries = Vec::new();
    for entry in read {
        stop.check()?;
        let entry = entry.map_err(|e| ToolError::io(dir.display(), e))?;
        *visited = visited.saturating_add(1);
        if *visited > MAX_ENTRIES {
            return Err(ToolError::new(ErrorCode::OverLimit, format!("{} has too many files to compare.", root.display())));
        }
        entries.push(entry.path());
    }
    entries.sort();
    for path in entries {
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| ToolError::io(path.display(), e))?;
        let name = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        let kind = if metadata.is_symlink() {
            "l"
        } else if metadata.is_dir() {
            "d"
        } else {
            "f"
        };
        let modified = metadata.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
        hash.update(format!("{kind}\0{name}\0{}\0{modified}\n", metadata.len()).as_bytes());
        if metadata.is_dir() {
            stamp_tree(root, &path, hash, visited, depth.saturating_add(1), stop)?;
        }
    }
    Ok(())
}

/// Content digests reused within one build run. The first scan reads the files; subsequent
/// scans only reread files whose metadata changed. Nothing here is persisted or added to the
/// fingerprint, so identical bytes still have the same version-1 fingerprint after a rewrite.
#[derive(Default)]
pub struct AppFingerprintCache {
    root: Option<PathBuf>,
    files: HashMap<PathBuf, CachedFile>,
    #[cfg(test)]
    content_bytes: u64,
}

struct CachedFile {
    stamp: FileStamp,
    digest: [u8; 32],
    seen: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: SystemTime,
    executable: u32,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl FileStamp {
    fn of(metadata: &std::fs::Metadata) -> Option<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok()?,
            executable: executable_mode(metadata),
            #[cfg(unix)]
            identity: (metadata.dev(), metadata.ino(), metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

struct Scan {
    hash: Sha256,
    visited: usize,
}

impl AppFingerprintCache {
    /// Return the same content fingerprint as a fresh scan while avoiding unchanged reads.
    /// Root changes, scan errors and cancellation discard cached comparisons. Missing files
    /// and paths that become another type are removed after every successful scan.
    pub fn scan(&mut self, ws: &Workspace, stop: &CancelToken) -> Result<String, ToolError> {
        let root = ws.app_dir();
        if self.root.as_ref() != Some(&root) {
            self.files.clear();
            self.root = Some(root.clone());
        }
        for file in self.files.values_mut() {
            file.seen = false;
        }
        let mut scan = Scan { hash: Sha256::new(), visited: 0 };
        scan.hash.update(b"craft-build-files-v1\0");
        if let Err(error) = fingerprint_tree(ws, &root, &root, &mut scan, self, 0, stop) {
            self.files.clear();
            self.root = None;
            return Err(error);
        }
        self.files.retain(|_, file| file.seen);
        Ok(hex::encode(scan.hash.finalize()))
    }

    fn file(&mut self, path: &Path, relative: &Path, stop: &CancelToken) -> Result<[u8; 32], ToolError> {
        stop.check()?;
        let metadata = std::fs::symlink_metadata(path).map_err(|e| ToolError::io(path.display(), e))?;
        if !metadata.is_file() {
            return Err(ToolError::new(ErrorCode::FileChanged, format!("{} changed type while checking build progress.", path.display())));
        }
        if let Some(stamp) = FileStamp::of(&metadata)
            && let Some(file) = self.files.get_mut(relative)
            && file.stamp == stamp
        {
            file.seen = true;
            return Ok(file.digest);
        }
        let read = file_fingerprint(path, stop)?;
        #[cfg(test)]
        {
            self.content_bytes = self.content_bytes.saturating_add(read.content_bytes);
        }
        // Use the metadata of the file actually opened, only after its entire stable read
        // succeeded. Without a usable modification time it is deliberately not cached.
        if let Some(stamp) = read.stamp {
            self.files.insert(relative.to_path_buf(), CachedFile { stamp, digest: read.digest, seen: true });
        }
        Ok(read.digest)
    }
}

fn fingerprint_tree(
    ws: &Workspace,
    root: &Path,
    dir: &Path,
    scan: &mut Scan,
    cache: &mut AppFingerprintCache,
    depth: usize,
    stop: &CancelToken,
) -> Result<(), ToolError> {
    stop.check()?;
    if depth > MAX_DEPTH {
        return Err(ToolError::new(
            ErrorCode::OverLimit,
            format!("Can't check build progress: {} is nested more than {MAX_DEPTH} folders deep.", dir.display()),
        ));
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| ToolError::io(dir.display(), e))? {
        stop.check()?;
        let entry = entry.map_err(|e| ToolError::io(dir.display(), e))?;
        let path = entry.path();
        let name = entry.file_name();
        if name == ".git" || (ws.manifest.app_in_root && dir == root && OWN_NAMES.iter().any(|n| name == *n)) {
            continue;
        }
        let kind = entry.file_type().map_err(|e| ToolError::io(path.display(), e))?;
        if kind.is_dir() && rebuilt(&path) {
            continue;
        }
        scan.visited = scan.visited.saturating_add(1);
        if scan.visited > MAX_ENTRIES {
            return Err(ToolError::new(
                ErrorCode::OverLimit,
                format!("Can't check build progress: {} contains more than {MAX_ENTRIES} files and folders.", root.display()),
            ));
        }
        entries.push((path, kind));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, kind) in entries {
        stop.check()?;
        if kind.is_dir() {
            fingerprint_tree(ws, root, &path, scan, cache, depth + 1, stop)?;
            continue;
        }
        let relative = path.strip_prefix(root).map_err(|_| ToolError::internal("A build file is outside its app folder."))?;
        for component in relative.components() {
            let name = component.as_os_str().as_encoded_bytes();
            scan.hash.update((name.len() as u64).to_le_bytes());
            scan.hash.update(name);
        }
        scan.hash.update(0u64.to_le_bytes());
        if kind.is_symlink() {
            scan.hash.update(b"link\0");
            let target = std::fs::read_link(&path).map_err(|e| ToolError::io(path.display(), e))?;
            let target = target.as_os_str().as_encoded_bytes();
            scan.hash.update((target.len() as u64).to_le_bytes());
            scan.hash.update(target);
        } else if kind.is_file() {
            scan.hash.update(b"file\0");
            scan.hash.update(cache.file(&path, relative, stop)?);
        } else {
            // A dev server can leave sockets or pipes beside its source. Their contents are
            // not comparable (and opening a FIFO can block); only their name and type count.
            scan.hash.update(special_type(kind));
        }
    }
    Ok(())
}

fn special_type(kind: std::fs::FileType) -> &'static [u8] {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if kind.is_socket() {
            return b"socket\0";
        }
        if kind.is_fifo() {
            return b"fifo\0";
        }
        if kind.is_char_device() {
            return b"character-device\0";
        }
        if kind.is_block_device() {
            return b"block-device\0";
        }
    }
    let _ = kind;
    b"special\0"
}

struct FileFingerprint {
    digest: [u8; 32],
    stamp: Option<FileStamp>,
    #[cfg(test)]
    content_bytes: u64,
}

fn file_fingerprint(path: &Path, stop: &CancelToken) -> Result<FileFingerprint, ToolError> {
    let mut file = std::fs::File::open(path).map_err(|e| ToolError::io(path.display(), e))?;
    let before = file.metadata().map_err(|e| ToolError::io(path.display(), e))?;
    // A growing log can't keep the scan running forever. A changed file is checked again on the
    // next run instead of being mistaken for an unchanged app.
    let mut reader = (&mut file).take(before.len());
    let mut bytes = 0u64;
    let mut hash = Sha256::new();
    // Executability changes what can be built or launched. Other permission and timestamp
    // changes alone do not count as progress. Non-Unix platforms have no execute mode bits.
    hash.update(executable_mode(&before).to_le_bytes());
    let mut buffer = [0u8; 64 * 1024];
    loop {
        stop.check()?;
        let n = reader.read(&mut buffer).map_err(|e| ToolError::io(path.display(), e))?;
        if n == 0 {
            break;
        }
        bytes = bytes.saturating_add(n as u64);
        hash.update(buffer.get(..n).ok_or_else(|| ToolError::internal("A file read exceeded its buffer."))?);
    }
    let after = file.metadata().map_err(|e| ToolError::io(path.display(), e))?;
    if bytes != before.len()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || executable_mode(&before) != executable_mode(&after)
        || FileStamp::of(&before) != FileStamp::of(&after)
    {
        return Err(ToolError::new(
            ErrorCode::FileChanged,
            format!("{} changed while checking build progress. Try again after its write finishes.", path.display()),
        ));
    }
    Ok(FileFingerprint {
        digest: hash.finalize().into(),
        stamp: FileStamp::of(&before),
        #[cfg(test)]
        content_bytes: bytes,
    })
}

#[cfg(unix)]
fn executable_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111
}

#[cfg(not(unix))]
fn executable_mode(_metadata: &std::fs::Metadata) -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{ExistingFiles, TargetRef};

    fn fingerprint(ws: &Workspace) -> String {
        ws.app_fingerprint(&CancelToken::default()).unwrap()
    }

    fn modified(path: &Path, seconds: u64) {
        let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
        std::fs::File::options().write(true).open(path).unwrap().set_times(std::fs::FileTimes::new().set_modified(time)).unwrap();
    }

    #[test]
    fn cache_does_not_reread_an_unchanged_large_tree_or_count_identical_rewrites_as_progress() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Game", "A game with imported assets", TargetRef::None).unwrap();
        let bytes = vec![b'a'; 2 * 1024 * 1024];
        for name in ["Original/a.bin", "Original/b.bin", "assets/c.bin", "assets/d.bin"] {
            let path = ws.app_dir().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            modified(&path, 1);
        }
        let mut cache = AppFingerprintCache::default();
        let stop = CancelToken::default();
        let first = cache.scan(&ws, &stop).unwrap();
        assert_eq!(cache.content_bytes, 8 * 1024 * 1024, "the first scan actually read every asset byte");
        let second = cache.scan(&ws, &stop).unwrap();
        assert_eq!(second, first);
        assert_eq!(cache.content_bytes, 8 * 1024 * 1024, "the unchanged second scan read zero content bytes");
        let file = ws.app_dir().join("Original/a.bin");
        std::fs::write(&file, &bytes).unwrap();
        modified(&file, 2);
        let rewritten = cache.scan(&ws, &stop).unwrap();
        assert_eq!(rewritten, first, "mtime is only a cache key, not build progress");
        assert_eq!(cache.content_bytes, 10 * 1024 * 1024, "only the rewritten file was reread");
        let mut progress = BuildProgress::default();
        progress.before_turn(first.clone());
        progress.after_turn(second);
        progress.after_turn(rewritten);
        assert!(progress.stalled(), "rewriting identical bytes cannot reset the unchanged-turn limit");
        let mut changed = bytes;
        changed[0] = b'b';
        std::fs::write(&file, changed).unwrap();
        modified(&file, 3);
        let edited = cache.scan(&ws, &stop).unwrap();
        assert_ne!(edited, first, "a same-length content edit is still progress");
        assert_eq!(cache.content_bytes, 12 * 1024 * 1024);
        assert_eq!(edited, fingerprint(&ws), "cached and fresh scans use the same content hash");
    }

    #[test]
    fn cache_tracks_added_deleted_and_retyped_paths() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Files", "A project", TargetRef::None).unwrap();
        std::fs::create_dir_all(ws.app_dir()).unwrap();
        let path = ws.app_dir().join("item");
        std::fs::write(&path, "a").unwrap();
        let mut cache = AppFingerprintCache::default();
        let stop = CancelToken::default();
        let first = cache.scan(&ws, &stop).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("child"), "b").unwrap();
        assert_ne!(cache.scan(&ws, &stop).unwrap(), first);
        std::fs::remove_dir_all(&path).unwrap();
        std::fs::write(&path, "a").unwrap();
        assert_eq!(cache.scan(&ws, &stop).unwrap(), first);
        assert_eq!(cache.content_bytes, 3, "a type change evicts the old file digest");
        std::fs::remove_file(&path).unwrap();
        let empty = cache.scan(&ws, &stop).unwrap();
        assert_ne!(empty, first);
        std::fs::write(&path, "a").unwrap();
        assert_eq!(cache.scan(&ws, &stop).unwrap(), first);
        assert_eq!(cache.content_bytes, 4, "a removed path is not retained in the cache");
        std::fs::write(ws.app_dir().join("new"), "c").unwrap();
        assert_ne!(cache.scan(&ws, &stop).unwrap(), first);
        assert_eq!(cache.content_bytes, 5, "adding a file leaves unchanged file contents cached");
    }

    #[test]
    fn cache_discards_comparisons_after_scan_errors_and_cancellation() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Files", "A project", TargetRef::None).unwrap();
        std::fs::create_dir_all(ws.app_dir()).unwrap();
        std::fs::write(ws.app_dir().join("source"), "abc").unwrap();
        let mut cache = AppFingerprintCache::default();
        let stop = CancelToken::default();
        let first = cache.scan(&ws, &stop).unwrap();
        let nested = (0..=MAX_DEPTH).fold(ws.app_dir().join("deep"), |p, _| p.join("d"));
        std::fs::create_dir_all(nested).unwrap();
        assert_eq!(cache.scan(&ws, &stop).unwrap_err().code, ErrorCode::OverLimit);
        std::fs::remove_dir_all(ws.app_dir().join("deep")).unwrap();
        assert_eq!(cache.scan(&ws, &stop).unwrap(), first);
        assert_eq!(cache.content_bytes, 6, "an error discarded the previous successful read too");
        let cancelled = CancelToken::default();
        cancelled.cancel();
        assert_eq!(cache.scan(&ws, &cancelled).unwrap_err().code, ErrorCode::Cancelled);
        assert_eq!(cache.scan(&ws, &stop).unwrap(), first);
        assert_eq!(cache.content_bytes, 9, "cancellation left no cached partial scan");
    }

    #[test]
    fn cache_never_reuses_reads_from_another_project_root() {
        let d = tempfile::tempdir().unwrap();
        let mut cache = AppFingerprintCache::default();
        let stop = CancelToken::default();
        for name in ["First", "Second"] {
            let ws = Workspace::create(d.path(), name, "A project", TargetRef::None).unwrap();
            std::fs::create_dir_all(ws.app_dir()).unwrap();
            let path = ws.app_dir().join("source");
            std::fs::write(&path, "abc").unwrap();
            modified(&path, 1);
            assert_eq!(cache.scan(&ws, &stop).unwrap(), fingerprint(&ws));
        }
        assert_eq!(cache.content_bytes, 6, "matching relative paths and metadata in another root still need reading");
    }

    #[test]
    fn progress_counts_identical_bytes_and_resets_for_edits() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Notes", "A notes app", TargetRef::None).unwrap();
        std::fs::create_dir_all(ws.app_dir()).unwrap();
        let file = ws.app_dir().join("main.txt");
        std::fs::write(&file, "first").unwrap();
        let first = fingerprint(&ws);
        let mut progress = BuildProgress::load(&ws.root).unwrap();
        progress.before_turn(first.clone());
        std::fs::write(&file, "first").unwrap();
        progress.after_turn(fingerprint(&ws));
        assert_eq!(progress.unchanged_turns, 1);
        progress.save(&ws.root).unwrap();
        let mut progress = BuildProgress::load(&Workspace::open(&ws.root).unwrap().root).unwrap();
        progress.before_turn(fingerprint(&ws));
        progress.after_turn(fingerprint(&ws));
        assert!(progress.stalled());
        std::fs::write(&file, "other").unwrap();
        assert_ne!(first, fingerprint(&ws), "same length, different contents");
        progress.before_turn(fingerprint(&ws));
        assert_eq!(progress.unchanged_turns, 0);
        std::fs::remove_file(file).unwrap();
        progress.after_turn(fingerprint(&ws));
        assert_eq!(progress.unchanged_turns, 0, "deleting an app file is a change too");
    }

    #[test]
    fn root_apps_ignore_craft_records_and_caches_but_keep_runtime_assets() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("mine");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("index.html"), "the app").unwrap();
        let ws = Workspace::create_at(&root, "Mine", "A page", TargetRef::None, Some(ExistingFiles::Edit)).unwrap();
        let before = fingerprint(&ws);
        for dir in ["checks", "node_modules", "target", ".git"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join("changed.txt"), "changed outside the app's source").unwrap();
        }
        std::fs::write(root.join("feed.json"), "changed feed").unwrap();
        BuildProgress::default().save(&ws.root).unwrap();
        assert_eq!(before, fingerprint(&ws));
        std::fs::create_dir_all(root.join("assets/obj")).unwrap();
        std::fs::write(root.join("assets/obj/shape.obj"), "runtime model").unwrap();
        assert_ne!(before, fingerprint(&ws), "an obj asset folder is not a managed build cache");
    }

    #[test]
    fn progress_records_migrate_and_newer_or_broken_records_are_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join(BUILD_PROGRESS_FILE);
        assert_eq!(BuildProgress::load(d.path()).unwrap(), BuildProgress::default());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"fingerprint":"previous","unchanged_turns":2}"#).unwrap();
        let old = BuildProgress::load(d.path()).unwrap();
        assert_eq!(old.version, BUILD_PROGRESS_VERSION);
        assert!(old.stalled());
        assert_eq!(old.checked, None);
        old.save(d.path()).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("\"version\": 2"));
        // A version 1 record has no kept check; a kept one round-trips.
        std::fs::write(&path, r#"{"version":1,"fingerprint":"previous","unchanged_turns":1}"#).unwrap();
        assert_eq!(BuildProgress::load(d.path()).unwrap().checked, None);
        let kept = BuildProgress { checked: Some(CheckedBuild { key: "k".into(), round: 3 }), ..old.clone() };
        kept.save(d.path()).unwrap();
        assert_eq!(BuildProgress::load(d.path()).unwrap(), kept);
        // An OpenCode builder kept its own notes here; the run stops, and says what to do.
        let builders_notes = r#"{"step":"done","text":"Done: the game builds."}"#;
        for bad in [r#"{"version":99,"future":true}"#, "not json", r#"{"version":-1}"#, r#"{"app_data":"not a Craft progress record"}"#, builders_notes] {
            std::fs::write(&path, bad).unwrap();
            let e = BuildProgress::load(d.path()).unwrap_err();
            let fix = e.remediation.clone().unwrap_or_default();
            if bad.contains("99") {
                assert!(fix.contains("Update Craft Anything"), "{bad}: {fix}");
            } else {
                assert!(fix.starts_with("Rename or delete inbox/build-progress.json") && e.message.contains("something else changed it"), "{bad}: {e:?}");
            }
            assert!(old.save(d.path()).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), bad);
        }
        let e = BuildProgress::load(d.path()).unwrap_err();
        assert!(e.message.contains("unknown field `step`") && !e.message.contains(".."), "{}", e.message);
        std::fs::write(&path, vec![b' '; MAX_RECORD_BYTES as usize + 1]).unwrap();
        assert_eq!(BuildProgress::load(d.path()).unwrap_err().code, ErrorCode::OverLimit);
    }

    #[test]
    fn a_duplicate_has_a_new_attempt_and_progress_is_never_app_source() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("mine");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("index.html"), "the app").unwrap();
        let ws = Workspace::create_at(&root, "Mine", "A page", TargetRef::None, Some(ExistingFiles::Edit)).unwrap();
        let progress = BuildProgress { fingerprint: Some(fingerprint(&ws)), unchanged_turns: 2, ..BuildProgress::default() };
        progress.save(&ws.root).unwrap();
        let copy = ws.duplicate("Another attempt").unwrap();
        assert_eq!(BuildProgress::load(&copy.root).unwrap(), BuildProgress::default());
        assert!(!copy.app_dir().join(BUILD_PROGRESS_FILE).exists());
        assert_eq!(fingerprint(&ws), fingerprint(&copy));
        assert_eq!(BuildProgress::load(&ws.root).unwrap(), progress, "the original keeps its limit");
    }

    #[test]
    fn an_existing_root_build_progress_name_remains_the_persons_app_data() {
        for (data, directory) in
            [(r#"{"player":"saved"}"#, false), ("{}", false), (r#"{"version":1,"fingerprint":null,"unchanged_turns":2}"#, false), ("a runtime asset", true)]
        {
            let d = tempfile::tempdir().unwrap();
            let root = d.path().join("mine");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("index.html"), "the app").unwrap();
            let ws = Workspace::create_at(&root, "Mine", "A page", TargetRef::None, Some(ExistingFiles::Edit)).unwrap();
            let before = fingerprint(&ws);
            // Simulate an existing in-place project from before progress was recorded. Even a
            // JSON object matching the new record's shape does not prove Craft owns it.
            let relative = if directory { "build-progress.json/state.txt" } else { "build-progress.json" };
            let app_file = root.join(relative);
            std::fs::create_dir_all(app_file.parent().unwrap()).unwrap();
            std::fs::write(&app_file, data).unwrap();
            assert!(!ws.is_own_record(&root.join("build-progress.json")));
            let with_data = fingerprint(&ws);
            assert_ne!(before, with_data, "the root name belongs to the app");
            let progress = BuildProgress { fingerprint: Some(with_data.clone()), unchanged_turns: 2, ..BuildProgress::default() };
            progress.save(&root).unwrap();
            assert_eq!(std::fs::read_to_string(&app_file).unwrap(), data, "save must not claim or migrate the root file");
            assert_eq!(fingerprint(&ws), with_data, "the inbox record is not app progress");
            std::fs::write(&app_file, format!("{data}\n")).unwrap();
            assert_ne!(fingerprint(&ws), with_data, "changes to that app file count");
            std::fs::write(&app_file, data).unwrap();
            let copy = ws.duplicate("Another attempt").unwrap();
            assert_eq!(std::fs::read_to_string(copy.app_dir().join(relative)).unwrap(), data);
            assert_eq!(BuildProgress::load(&copy.root).unwrap(), BuildProgress::default());
            assert!(!copy.app_dir().join(BUILD_PROGRESS_FILE).exists());
            ws.delete().unwrap();
            assert_eq!(std::fs::read_to_string(&app_file).unwrap(), data, "removing Craft's records must keep the app's data");
            assert!(!root.join(BUILD_PROGRESS_FILE).exists());
        }
    }

    #[test]
    fn inbox_messages_do_not_consume_the_progress_record() {
        use crate::inbox::{Answer, Inbox, RequestKind};
        let d = tempfile::tempdir().unwrap();
        let progress = BuildProgress { unchanged_turns: 2, ..BuildProgress::default() };
        progress.save(d.path()).unwrap();
        let inbox = Inbox::new(d.path().join("inbox"));
        assert!(inbox.pending().is_empty());
        let id = inbox.post(RequestKind::Question, "Continue?", "", Vec::new(), true, None).unwrap();
        assert_eq!(inbox.pending().len(), 1);
        inbox.answer(&id, &Answer { text: Some("Yes".into()), ..Default::default() }).unwrap();
        let answer = inbox.wait(&id, std::time::Duration::from_millis(100), &|| false).unwrap();
        assert_eq!(answer.text.as_deref(), Some("Yes"));
        assert!(inbox.pending().is_empty());
        assert_eq!(BuildProgress::load(d.path()).unwrap(), progress);
    }

    #[test]
    fn moving_the_project_folder_keeps_its_unchanged_turn_count() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Notes", "A notes app", TargetRef::None).unwrap();
        std::fs::create_dir_all(ws.app_dir()).unwrap();
        std::fs::write(ws.app_dir().join("index.html"), "the app").unwrap();
        let progress = BuildProgress { fingerprint: Some(fingerprint(&ws)), unchanged_turns: 2, ..BuildProgress::default() };
        progress.save(&ws.root).unwrap();
        let moved = d.path().join("moved-project");
        std::fs::rename(&ws.root, &moved).unwrap();
        let ws = Workspace::open(&moved).unwrap();
        let mut loaded = BuildProgress::load(&ws.root).unwrap();
        loaded.before_turn(fingerprint(&ws));
        assert_eq!(loaded, progress, "the fingerprint is relative to the app, not its original folder");
        assert!(loaded.stalled());
    }

    #[cfg(unix)]
    #[test]
    fn cache_keeps_executable_mode_as_progress_but_not_other_permission_bits() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Script", "A script", TargetRef::None).unwrap();
        std::fs::create_dir_all(ws.app_dir()).unwrap();
        let file = ws.app_dir().join("build.sh");
        std::fs::write(&file, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let mut cache = AppFingerprintCache::default();
        let stop = CancelToken::default();
        let before = cache.scan(&ws, &stop).unwrap();
        let mut progress = BuildProgress { fingerprint: Some(before.clone()), unchanged_turns: 2, ..BuildProgress::default() };
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        progress.before_turn(cache.scan(&ws, &stop).unwrap());
        assert!(progress.stalled(), "read/write mode alone is not source progress");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        progress.before_turn(cache.scan(&ws, &stop).unwrap());
        assert_ne!(progress.fingerprint.as_deref(), Some(before.as_str()));
        assert_eq!(progress.unchanged_turns, 0, "making the script runnable is progress");
    }

    #[test]
    fn deeply_nested_apps_stop_without_recursing_indefinitely() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Nested", "A nested app", TargetRef::None).unwrap();
        let mut dir = ws.app_dir();
        for _ in 0..=MAX_DEPTH {
            dir.push("d");
        }
        std::fs::create_dir_all(dir).unwrap();
        let error = ws.app_fingerprint(&CancelToken::default()).unwrap_err();
        assert_eq!(error.code, ErrorCode::OverLimit);
        assert!(error.message.contains("nested"));
    }

    #[test]
    fn the_progress_schema_is_current() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/schemas/build-progress.schema.json");
        let saved = std::fs::read_to_string(path).unwrap_or_default();
        assert_eq!(saved.replace("\r\n", "\n"), build_progress_schema());
    }

    #[cfg(unix)]
    #[test]
    fn special_files_contribute_their_name_and_type_without_reading_them() {
        use std::os::unix::net::UnixListener;
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Socket", "An app with a dev server", TargetRef::None).unwrap();
        std::fs::create_dir_all(ws.app_dir()).unwrap();
        let path = ws.app_dir().join("server.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let socket = fingerprint(&ws);
        assert_eq!(fingerprint(&ws), socket, "an idle socket is stable and is not read");
        drop(listener);
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "").unwrap();
        assert_ne!(fingerprint(&ws), socket, "a regular file at the same name has a different type");
        std::fs::remove_file(&path).unwrap();
        let _listener = UnixListener::bind(&path).unwrap();
        assert_eq!(fingerprint(&ws), socket, "recreating the same endpoint is not source progress");
        std::fs::write(ws.app_dir().join("main.txt"), "new source").unwrap();
        assert_ne!(fingerprint(&ws), socket, "regular app changes still count beside a socket");
    }

    #[cfg(unix)]
    #[test]
    fn links_are_not_followed_and_cancel_stops_the_scan() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Notes", "A notes app", TargetRef::None).unwrap();
        std::fs::create_dir_all(ws.app_dir()).unwrap();
        let outside = d.path().join("outside");
        std::fs::write(&outside, "one").unwrap();
        std::os::unix::fs::symlink(&outside, ws.app_dir().join("link")).unwrap();
        std::os::unix::fs::symlink(ws.app_dir(), ws.app_dir().join("cycle")).unwrap();
        let before = fingerprint(&ws);
        std::fs::write(outside, "two").unwrap();
        assert_eq!(before, fingerprint(&ws));
        let stop = CancelToken::default();
        stop.cancel();
        assert_eq!(ws.app_fingerprint(&stop).unwrap_err().code, ErrorCode::Cancelled);
    }
}
