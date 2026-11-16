//! Targets: what we're looking at, loaded once, identified by content hash.

use craft_anything_evidence::{ErrorCode, Subject, ToolError, sha256_hex};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Biggest file we load into memory. Bigger inputs get a clear error rather than an OOM.
pub const MAX_TARGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub type TargetId = String;

/// Metadata handed to an agent's MCP session. No file contents or caches cross the command
/// line; Deep mode reloads and verifies the file, while Clean room keeps only this metadata.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetContext {
    pub path: PathBuf,
    pub sha256: String,
    pub kind: TargetKind,
    pub raw: Option<RawSpec>,
}

impl TargetContext {
    pub fn from_target(target: &Target) -> Self {
        Self { path: target.path.clone(), sha256: target.sha256.clone(), kind: target.kind, raw: target.raw.clone() }
    }

    pub fn open(&self, clean_room: bool) -> Result<Target, ToolError> {
        if !self.path.is_absolute() || self.sha256.len() != 64 || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ToolError::invalid("A shared target needs an absolute path and a SHA-256 hash."));
        }
        let mut target = if clean_room {
            Target {
                id: format!("t_{}", self.sha256.get(..12).unwrap_or_default()),
                path: self.path.clone(),
                name: self.path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                sha256: self.sha256.clone(),
                kind: self.kind,
                bytes: Arc::from([]),
                raw: self.raw.clone(),
            }
        } else {
            let target = Target::load(&self.path)?;
            if target.sha256 != self.sha256 {
                return Err(ToolError::invalid("The shared target changed after it was opened.")
                    .with_remediation("Reopen the target in Craft Anything, then start a new conversation."));
            }
            target
        };
        if self.raw.is_some() || self.kind == TargetKind::DosCom {
            target.kind = self.kind;
            target.raw = self.raw.clone();
        }
        Ok(target)
    }
}

/// What kind of thing a target is. Detected from content, never from the extension alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TargetKind {
    Elf,
    Pe,
    MachO,
    /// A fat/universal Mach-O.
    MachOFat,
    /// A .NET assembly (PE with a CLI header).
    Managed,
    Wasm,
    Apk,
    Dex,
    Jar,
    /// A single Java `.class` file.
    JavaClass,
    Zip,
    Asar,
    /// A folder: an app bundle, an unpacked Electron app, a project.
    Directory,
    /// A JavaScript or HTML file.
    Script,
    /// A HAR or other network capture.
    NetworkCapture,
    Plist,
    /// EVM bytecode.
    Evm,
    /// A SQLite 3 database file.
    Sqlite,
    /// A DOS .COM program.
    DosCom,
    /// Raw bytes with a user-declared architecture (firmware, shellcode).
    RawBinary,
    /// Plain text, such as notes, source files and settings.
    Text,
    Unknown,
}

impl TargetKind {
    /// Plain-English description for Beginner mode.
    pub fn plain(self) -> &'static str {
        match self {
            TargetKind::Elf => "A Linux (or Unix) program",
            TargetKind::Pe => "A Windows program",
            TargetKind::MachO | TargetKind::MachOFat => "A Mac or iPhone program",
            TargetKind::Managed => "A .NET program (C#, F# or VB)",
            TargetKind::Wasm => "A WebAssembly module (code that runs in a browser)",
            TargetKind::Apk => "An Android app",
            TargetKind::Dex => "Android app code (DEX)",
            TargetKind::Jar => "A Java program",
            TargetKind::JavaClass => "Java program code (one class)",
            TargetKind::Zip => "A ZIP archive",
            TargetKind::Asar => "The packed contents of an Electron app (a website inside a desktop app)",
            TargetKind::Directory => "A folder",
            TargetKind::Script => "A script or web page",
            TargetKind::NetworkCapture => "A recording of network traffic",
            TargetKind::Plist => "An Apple settings file (plist)",
            TargetKind::Evm => "A smart contract (EVM bytecode)",
            TargetKind::Sqlite => "A SQLite database (how an app stores its data)",
            TargetKind::DosCom => "A DOS program",
            TargetKind::RawBinary => "Raw machine code",
            TargetKind::Text => "A text file (notes, source code or settings)",
            TargetKind::Unknown => "A file we don't recognise yet",
        }
    }

    pub fn id(self) -> String {
        serde_json::to_value(self).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "unknown".into())
    }
}

/// Whether an otherwise unknown file looks like text. Inspect at most 8 KiB:
/// a byte-order mark counts as text, otherwise require valid UTF-8, no NULs
/// and few control bytes. Empty input is not text.
pub fn looks_like_text(bytes: &[u8]) -> bool {
    const SNIFF: usize = 8 * 1024;
    if bytes.starts_with(b"\xEF\xBB\xBF") || bytes.starts_with(b"\xFF\xFE") || bytes.starts_with(b"\xFE\xFF") {
        return true;
    }
    let head = bytes.get(..bytes.len().min(SNIFF)).unwrap_or_default();
    if head.is_empty() || head.contains(&0) {
        return false;
    }
    let cut_at_end = bytes.len() > SNIFF;
    let mut chunks = head.utf8_chunks().peekable();
    while let Some(chunk) = chunks.next() {
        // The window may end inside a character; only that last sequence may be incomplete.
        let last = chunks.peek().is_none();
        if !chunk.invalid().is_empty() && !(last && cut_at_end && chunk.invalid().len() < 4) {
            return false;
        }
    }
    let control = head.iter().filter(|b| b.is_ascii_control() && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0c | 0x1b)).count();
    control.saturating_mul(100) <= head.len()
}

/// Sniff the kind from the first bytes (and the name, only to break ties).
///
/// Signatures come from the shared `craft_sniff` table; the tie-breaks that need our own parsers
/// (managed PE, Java class or universal Mach-O, APK by its manifest) and the kinds with no
/// signature (ASAR, XML plists, captures, scripts, text) stay here.
pub fn sniff(bytes: &[u8], name: &str) -> TargetKind {
    use craft_sniff::Kind;
    let lower = name.to_ascii_lowercase();
    match craft_sniff::identify(bytes) {
        Some(Kind::Sqlite) => TargetKind::Sqlite,
        Some(Kind::Elf) => TargetKind::Elf,
        Some(Kind::Pe | Kind::DosExecutable) => {
            if craft_anything_formats::pe_is_managed(bytes) {
                TargetKind::Managed
            } else {
                TargetKind::Pe
            }
        }
        Some(Kind::MachO) => TargetKind::MachO,
        // Both a Java class and a universal Mach-O start 0xCAFEBABE. A class has a plausible
        // major version where a fat binary has a small big-endian architecture count, so prefer
        // the .class extension, then the version check.
        Some(Kind::MachOFat | Kind::JavaClass) => {
            if lower.ends_with(".class") || craft_anything_formats::java_class::is_class(bytes) {
                TargetKind::JavaClass
            } else {
                TargetKind::MachOFat
            }
        }
        Some(Kind::Wasm) => TargetKind::Wasm,
        Some(Kind::Dex) => TargetKind::Dex,
        Some(Kind::Zip) => sniff_zip(bytes, &lower),
        Some(Kind::BinaryPlist) => TargetKind::Plist,
        Some(_) => TargetKind::Unknown,
        None => sniff_unsigned(bytes, &lower),
    }
}

/// A ZIP is an APK or a JAR by its name, then an APK by its manifest.
fn sniff_zip(bytes: &[u8], lower: &str) -> TargetKind {
    if lower.ends_with(".apk") || lower.ends_with(".aab") || lower.ends_with(".xapk") {
        TargetKind::Apk
    } else if lower.ends_with(".jar") || lower.ends_with(".war") {
        TargetKind::Jar
    } else if craft_anything_formats::zip_has_entry(bytes, "AndroidManifest.xml") {
        TargetKind::Apk
    } else {
        TargetKind::Zip
    }
}

/// The kinds the shared table has no signature for, told apart by name and content.
fn sniff_unsigned(bytes: &[u8], lower: &str) -> TargetKind {
    if lower.ends_with(".asar") || craft_anything_formats::looks_like_asar(bytes) {
        TargetKind::Asar
    } else if lower.ends_with(".plist") && bytes.windows(6).take(512).any(|w| w == b"<plist") {
        TargetKind::Plist
    } else if lower.ends_with(".har") {
        TargetKind::NetworkCapture
    } else if lower.ends_with(".com") && bytes.len() <= 0xff00 {
        TargetKind::DosCom
    } else if [".js", ".mjs", ".cjs", ".html", ".htm", ".ts"].iter().any(|e| lower.ends_with(e)) {
        TargetKind::Script
    } else if looks_like_text(bytes) {
        TargetKind::Text
    } else {
        TargetKind::Unknown
    }
}

/// A loaded target.
#[derive(Debug, Clone)]
pub struct Target {
    /// `t_` + first 12 hex of the sha256 (+ a suffix if the same file is opened twice).
    pub id: TargetId,
    pub path: PathBuf,
    pub name: String,
    pub sha256: String,
    pub kind: TargetKind,
    /// File contents (empty for directories).
    pub bytes: Arc<[u8]>,
    /// For raw binaries: declared architecture and entry point.
    pub raw: Option<RawSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RawSpec {
    /// `x86`, `x86_64`, `arm`, `arm64`.
    pub architecture: String,
    /// Load and entry address.
    pub entry_point: u64,
}

impl Target {
    /// Load a file or directory.
    pub fn load(path: &Path) -> Result<Target, ToolError> {
        let meta = std::fs::metadata(path).map_err(|e| ToolError::io(path.display(), e))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        if meta.is_dir() {
            let digest = sha256_hex(path.to_string_lossy().as_bytes());
            return Ok(Target {
                id: format!("t_{}", digest.get(..12).unwrap_or(&digest)),
                path: path.to_path_buf(),
                name,
                sha256: digest,
                kind: TargetKind::Directory,
                bytes: Arc::from(Vec::new()),
                raw: None,
            });
        }
        let bytes = craft_anything_platform::fs::read_file_capped(path, MAX_TARGET_BYTES)?;
        Ok(Target::from_bytes(path.to_path_buf(), name, bytes))
    }

    pub fn from_bytes(path: PathBuf, name: String, bytes: Vec<u8>) -> Target {
        let sha256 = sha256_hex(&bytes);
        let kind = sniff(&bytes, &name);
        Target { id: format!("t_{}", sha256.get(..12).unwrap_or(&sha256)), path, name, sha256, kind, bytes: Arc::from(bytes), raw: None }
    }

    pub fn subject(&self) -> Subject {
        Subject { kind: self.kind.id(), sha256: Some(self.sha256.clone()), name: Some(self.name.clone()) }
    }

    /// Fail if the file on disk no longer matches what we loaded.
    pub fn check_unchanged(&self) -> Result<(), ToolError> {
        if self.kind == TargetKind::Directory {
            return Ok(());
        }
        match craft_anything_platform::fs::read_file_capped(&self.path, MAX_TARGET_BYTES) {
            Ok(b) if sha256_hex(&b) == self.sha256 => Ok(()),
            Ok(_) => Err(ToolError::new(ErrorCode::FileChanged, format!("{} changed on disk since it was opened", self.path.display()))
                .with_remediation("Open it again to analyse the new version.")),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_common_magics() {
        assert_eq!(sniff(b"\x7fELF\x02\x01", "a"), TargetKind::Elf);
        assert_eq!(sniff(b"\0asm\x01\0\0\0", "m"), TargetKind::Wasm);
        assert_eq!(sniff(b"dex\n035\0", "classes.dex"), TargetKind::Dex);
        assert_eq!(sniff(b"PK\x03\x04", "app.apk"), TargetKind::Apk);
        assert_eq!(sniff(b"bplist00", "Info.plist"), TargetKind::Plist);
        assert_eq!(sniff(b"SQLite format 3\0\x10\x00", "app.db"), TargetKind::Sqlite);
        assert_eq!(sniff(b"\xcf\xfa\xed\xfe\x07\0\0\x01", "tool"), TargetKind::MachO);
        assert_eq!(sniff(b"\xca\xfe\xba\xbe\0\0\0\x02", "tool"), TargetKind::MachOFat);
        assert_eq!(sniff(b"\xca\xfe\xba\xbf\0\0\0\x02", "tool"), TargetKind::MachOFat);
        assert_eq!(sniff(b"\xca\xfe\xba\xbe\0\0\0\x41", "Main"), TargetKind::JavaClass);
        assert_eq!(sniff(b"\xca\xfe\xba\xbe\0\0\0\x02", "Main.class"), TargetKind::JavaClass);
        assert_eq!(sniff(b"MZ\x90\0", "setup.exe"), TargetKind::Pe);
        assert_eq!(sniff(b"PK\x03\x04", "lib.jar"), TargetKind::Jar);
        assert_eq!(sniff(b"PK\x05\x06\0\0\0\0", "empty.zip"), TargetKind::Zip);
        assert_eq!(sniff(b"<?xml?><plist version=\"1.0\"/>", "Info.plist"), TargetKind::Plist);
        // Media and archives the shared table names are not targets we inspect by kind.
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR", "icon.png"), TargetKind::Unknown);
        assert_eq!(sniff(b"070701", "archive.cpio"), TargetKind::Unknown);
        assert_eq!(sniff(b"", "x"), TargetKind::Unknown);
        assert_eq!(sniff(b"console.log(1)", "main.js"), TargetKind::Script);
        assert_eq!(sniff(b"# Notes\n", "notes.md"), TargetKind::Text);
        assert_eq!(sniff(b"key = true\n", "config.toml"), TargetKind::Text);
        assert_eq!(sniff(b"\xEF\xBB\xBFhello", "notes.txt"), TargetKind::Text);
        assert_eq!(sniff(b"\xFF\xFEh\0i\0", "notes.txt"), TargetKind::Text);
        assert_eq!(sniff(b"\0\xFF\x02", "notes.md"), TargetKind::Unknown);
        // The bytes decide, not the name: a gzip stream called `.txt` is not text.
        assert_eq!(sniff(b"\x1f\x8b\x08\x00\x00\x00", "notes.txt"), TargetKind::Unknown);
        assert_eq!(sniff(b"", "empty.txt"), TargetKind::Unknown);
        assert_eq!(TargetKind::Text.id(), "text");
        assert!(TargetKind::Text.plain().contains("text file"));
    }
}
