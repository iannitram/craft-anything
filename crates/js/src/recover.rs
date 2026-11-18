//! Source recovery: pretty-printing, a few meaning-preserving unminify rewrites, and cutting
//! bundles into one file per module.
//!
//! Every rewrite is a text edit on exact syntax-tree spans, and the edited text must parse again
//! or the edit set is dropped. Rewrites at the `standard` level:
//! - `!0` → `true`, `!1` → `false`;
//! - `void 0` → `undefined` (only without a binding named `undefined` or a dynamic `with` scope);
//! - `a(), b(), c();` statement sequences → separate statements (only in statement lists, and only
//!   when no later part would change meaning at the start of a statement);
//! - webpack module parameters `(e, t, n)` → `(module, exports, __webpack_require__)`, using scope
//!   analysis so every reference is renamed and nothing is captured; skipped for possible direct eval.

use crate::lex::{TokKind, tokenize};
use crate::model::BundleModule;
use crate::util::slice;
use crate::{JsError, Limits, admit, guarded, parse_options, source_type_for};
use oxc_allocator::Allocator;
use oxc_ast::AstKind;
use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::{GetSpan, SourceType};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewriteLevel {
    /// Pretty-print only.
    None,
    /// Pretty-print plus the safe rewrites listed in the module docs.
    #[default]
    Standard,
}

/// A text edit: replace `start..end` with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Edit {
    start: u32,
    end: u32,
    text: String,
}

/// Apply non-overlapping edits (overlaps are dropped). `None` if any range is invalid.
fn apply_edits(src: &str, mut edits: Vec<Edit>) -> Option<String> {
    edits.sort_by_key(|e| (e.start, e.end));
    let mut out = String::with_capacity(src.len() + edits.len() * 8);
    let mut pos = 0usize;
    for e in edits {
        let (s, z) = (e.start as usize, e.end as usize);
        if s < pos || z < s || !src.is_char_boundary(s) || !src.is_char_boundary(z) || z > src.len() {
            continue;
        }
        out.push_str(src.get(pos..s)?);
        out.push_str(&e.text);
        pos = z;
    }
    out.push_str(src.get(pos..)?);
    Some(out)
}

struct RewriteVisitor<'s> {
    src: &'s str,
    edits: Vec<Edit>,
    voids: Vec<Edit>,
    binds_undefined: bool,
    has_with: bool,
}

fn starts_statement_safely(text: &str) -> bool {
    let t = text.trim_start();
    !(t.starts_with('{') || t.starts_with("function") || t.starts_with("class") || t.starts_with("let") || t.starts_with("async") || t.is_empty())
}

impl<'a> Visit<'a> for RewriteVisitor<'_> {
    fn visit_unary_expression(&mut self, it: &UnaryExpression<'a>) {
        match (&it.operator, &it.argument) {
            (UnaryOperator::LogicalNot, Expression::NumericLiteral(n)) if n.value == 0.0 || n.value == 1.0 => {
                let text = if n.value == 0.0 { " true " } else { " false " };
                self.edits.push(Edit { start: it.span.start, end: it.span.end, text: text.into() });
            }
            (UnaryOperator::Void, Expression::NumericLiteral(n)) if n.value == 0.0 => {
                self.voids.push(Edit { start: it.span.start, end: it.span.end, text: " undefined ".into() });
            }
            _ => walk::walk_unary_expression(self, it),
        }
    }

    /// A with object can supply undefined even inside a closure created in its body.
    fn visit_with_statement(&mut self, it: &WithStatement<'a>) {
        self.has_with = true;
        walk::walk_with_statement(self, it);
    }

    fn visit_binding_identifier(&mut self, it: &BindingIdentifier<'a>) {
        if it.name == "undefined" {
            self.binds_undefined = true;
        }
    }

    fn visit_statements(&mut self, it: &oxc_allocator::ArenaVec<'a, Statement<'a>>) {
        for st in it {
            if let Statement::ExpressionStatement(es) = st
                && let Expression::SequenceExpression(seq) = &es.expression
                && seq.expressions.len() >= 2
                && !matches!(seq.expressions.first(), Some(Expression::StringLiteral(_)))
                && seq.expressions.iter().skip(1).all(|e| starts_statement_safely(slice(self.src, e.span().start, e.span().end)))
            {
                for pair in seq.expressions.windows(2) {
                    if let [a, b] = pair {
                        self.edits.push(Edit { start: a.span().end, end: b.span().start, text: ";\n".into() });
                    }
                }
            }
        }
        walk::walk_statements(self, it);
    }
}

fn parse_ok(src: &str, st: SourceType) -> bool {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, st).with_options(parse_options()).parse();
    !ret.fatal_error && ret.diagnostics.errors().next().is_none()
}

/// Rewrite then print. Runs on the caller's thread: call through [`guarded`].
fn rewrite_and_print(src: &str, st: SourceType, level: RewriteLevel) -> Result<(String, usize), String> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, st).with_options(parse_options()).parse();
    if ret.fatal_error {
        return Err(ret.diagnostics.errors().next().map_or_else(|| "syntax error".to_string(), |e| e.to_string()));
    }
    let mut edits_applied = 0usize;
    if level == RewriteLevel::Standard {
        let mut v = RewriteVisitor { src, edits: Vec::new(), voids: Vec::new(), binds_undefined: false, has_with: false };
        v.visit_program(&ret.program);
        let mut edits = v.edits;
        if !v.binds_undefined && !v.has_with {
            edits.extend(v.voids);
        }
        let n = edits.len();
        if n > 0
            && let Some(edited) = apply_edits(src, edits)
            && parse_ok(&edited, st)
        {
            let alloc2 = Allocator::default();
            let ret2 = Parser::new(&alloc2, &edited, st).with_options(parse_options()).parse();
            if !ret2.fatal_error {
                return Ok((oxc_codegen::Codegen::new().build(&ret2.program).code, n));
            }
        }
        edits_applied = 0;
    }
    Ok((oxc_codegen::Codegen::new().build(&ret.program).code, edits_applied))
}

/// Pretty-print a source (with rewrites at `level`). Returns the text and how many rewrites applied.
pub fn pretty_print(path: &str, src: &str, level: RewriteLevel, limits: &Limits) -> Result<(String, usize), JsError> {
    admit(src, limits).map_err(JsError::Refused)?;
    let owned: Arc<str> = Arc::from(src);
    let st = source_type_for(path);
    guarded(move || rewrite_and_print(&owned, st, level))?.map_err(JsError::Parser)
}

const WEBPACK_NAMES: [&str; 3] = ["module", "exports", "__webpack_require__"];

/// Rename a module function's parameters inside `body` using scope analysis.
/// Returns the new body and the renames applied.
fn rename_params(params: &[String], body: &str, targets: &[&str]) -> Option<(String, Vec<(String, String)>)> {
    if params.iter().any(|p| p == "_" || p.contains(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))) {
        return None;
    }
    let wrapper = format!("(function({}){{\n{}\n}})", params.join(","), body);
    let header_len = wrapper.find("){\n")?;
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, &wrapper, SourceType::cjs()).with_options(parse_options()).parse();
    if ret.fatal_error || ret.diagnostics.errors().next().is_some() {
        return None;
    }
    let sem = SemanticBuilder::new().with_build_nodes(true).build(&ret.program).semantic;
    // Direct eval can read or create bindings by name, including from a nested function.
    // Treat a call spelled `eval(...)` conservatively even when the name is locally bound.
    if sem.nodes().iter().any(|node| {
        matches!(node.kind(), AstKind::CallExpression(call)
            if !call.optional && matches!(call.callee.get_inner_expression(), Expression::Identifier(id) if id.name == "eval"))
    }) {
        return None;
    }
    let scoping = sem.scoping();
    let Some(Statement::ExpressionStatement(es)) = ret.program.body.first() else { return None };
    let Expression::FunctionExpression(f) = es.expression.get_inner_expression() else { return None };
    // Names already used as identifiers anywhere in the body can't be taken.
    let (tokens, _) = tokenize(body, 5_000_000);
    // Property names after `.`/`?.` are not references and don't count.
    let mut used: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut prev_dot = false;
    for t in tokens.iter().filter(|t| t.kind != TokKind::Comment) {
        if t.kind == TokKind::Ident && !prev_dot {
            used.insert(t.text);
        }
        prev_dot = t.kind == TokKind::Punct && (t.text == "." || t.text == "?.");
    }
    let mut edits = Vec::new();
    let mut renames = Vec::new();
    for (i, item) in f.params.items.iter().enumerate() {
        let (Some(new), BindingPattern::BindingIdentifier(bid)) = (targets.get(i), &item.pattern) else { continue };
        let old = bid.name.as_str();
        if old == *new || used.contains(new) || params.iter().any(|p| p == new) {
            continue;
        }
        let Some(sym) = bid.symbol_id.get() else { continue };
        edits.push(Edit { start: bid.span.start, end: bid.span.end, text: (*new).to_string() });
        for rid in scoping.get_resolved_reference_ids(sym) {
            let node_id = scoping.get_reference(*rid).node_id();
            let span = sem.nodes().kind(node_id).span();
            let text = match sem.nodes().parent_kind(node_id) {
                AstKind::ObjectProperty(p) if p.shorthand => format!("{old}: {new}"),
                AstKind::AssignmentTargetPropertyIdentifier(_) => format!("{old}: {new}"),
                _ => (*new).to_string(),
            };
            edits.push(Edit { start: span.start, end: span.end, text });
        }
        renames.push((old.to_string(), (*new).to_string()));
    }
    if renames.is_empty() {
        return None;
    }
    let edited = apply_edits(&wrapper, edits)?;
    let new_header = edited.find("){\n")?;
    let _ = header_len;
    let inner = edited.get(new_header + 3..edited.len().checked_sub(3)?)?.to_string();
    Some((inner, renames))
}

/// One recovered module file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RecoveredModule {
    pub id: String,
    pub format: String,
    /// Safe relative file name for it.
    pub file_name: String,
    pub text: String,
    /// sha256 of the module's original bytes in the bundle.
    pub source_sha256: String,
    pub start: u32,
    pub end: u32,
    pub renamed: Vec<(String, String)>,
    pub rewrites: usize,
    pub printed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Whether `path`'s final segment already ends with a known script, stylesheet, component, or data
/// extension (case-insensitively, with a non-empty stem).
fn has_known_module_ext(path: &str) -> bool {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    leaf.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && ["js", "mjs", "cjs", "ts", "tsx", "jsx", "json", "css", "vue", "svelte"].iter().any(|known| ext.eq_ignore_ascii_case(known))
    })
}

/// A safe relative path for a module id: no `..`, no absolute roots, no leading or trailing segment
/// dots, no odd characters, and case-insensitive known-extension preservation.
pub fn safe_module_path(id: &str, fallback_ext: &str) -> String {
    let trimmed = id.strip_prefix("webpack://").or_else(|| id.strip_prefix("webpack:")).or_else(|| id.strip_prefix("file://")).unwrap_or(id);
    let mut parts = Vec::new();
    for p in trimmed.split(['/', '\\']) {
        let p = p.trim();
        let is_drive = matches!(p.as_bytes(), [c, b':'] if c.is_ascii_alphabetic());
        if p.is_empty() || p == "." || p == ".." || is_drive {
            continue;
        }
        let clean: String = p.chars().map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | '+') { c } else { '_' }).take(120).collect();
        let clean = clean.trim_matches('.').to_string();
        if !clean.is_empty() {
            parts.push(clean);
        }
        if parts.len() >= 16 {
            break;
        }
    }
    if parts.is_empty() {
        parts.push("module".into());
    }
    let mut path = parts.join("/");
    if !has_known_module_ext(&path) {
        path.push_str(fallback_ext);
    }
    path
}

/// Decompose a precomposed Hangul syllable (`U+AC00..=U+D7A3`) into canonical Jamo (`L`, `V`, optional `T`).
fn decompose_hangul(c: char) -> [Option<char>; 3] {
    let u = u32::from(c);
    if !(0xAC00..=0xD7A3).contains(&u) {
        return [Some(c), None, None];
    }
    let s = u - 0xAC00;
    let l = char::from_u32(0x1100 + s / 588);
    let v = char::from_u32(0x1161 + (s % 588) / 28);
    let t = match s % 28 {
        0 => None,
        rem => char::from_u32(0x11A7 + rem),
    };
    [l, v, t]
}

/// Build a case-folded deduplication key for relative output paths across case-insensitive
/// filesystems (Windows NTFS and macOS APFS).
///
/// Decomposes Hangul syllables (`가` U+AC00 → `가` U+1100 U+1161) and chains `char::to_lowercase`
/// into `char::to_uppercase` so Greek final and medial sigma (`ς`/`σ` → `Σ`), singleton
/// decompositions (`K` U+212A → `K`, `Å` U+212B → `Å`, `Ω` U+2126 → `Ω`), Turkish `İ`/`ı`, and
/// APFS multi-character case foldings (`ß`/`ẞ` → `SS`, `ﬁ` → `FI`) map to one key while callers
/// preserve the first file's original spelling on disk.
pub fn module_path_key(path: &str) -> String {
    path.chars().flat_map(decompose_hangul).flatten().flat_map(char::to_lowercase).flat_map(char::to_uppercase).collect()
}

/// Return [`module_path_key`] for every strict parent directory prefix of a `/`-separated
/// relative file path so callers can prevent file/directory collisions in either input order.
pub fn ancestor_dir_keys(path: &str) -> Vec<String> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    let mut out = Vec::new();
    let mut prefix = String::new();
    for seg in segs.iter().take(segs.len().saturating_sub(1)) {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(seg);
        out.push(module_path_key(&prefix));
    }
    out
}

/// Cut the modules found in `src` into files, pretty-printing each.
pub fn recover_modules(src: &str, modules: &[BundleModule], level: RewriteLevel, limits: &Limits) -> Vec<RecoveredModule> {
    let mut out = Vec::new();
    let initial_paths: Vec<String> = modules.iter().map(|m| safe_module_path(&m.id, ".js")).collect();
    let mut names: std::collections::HashSet<String> = initial_paths.iter().flat_map(|p| ancestor_dir_keys(p)).collect();
    for (module_index, (m, mut file_name)) in modules.iter().zip(initial_paths).enumerate() {
        let original = slice(src, m.start, m.end);
        let mut renamed = Vec::new();
        let mut note = None;
        let body = match m.body {
            Some((s, e)) => slice(src, s, e).to_string(),
            None => format!("module.exports = ({original});"),
        };
        let body = if level == RewriteLevel::Standard && m.format.starts_with("webpack") && m.body.is_some() {
            let params = m.params.clone();
            let b = body.clone();
            match guarded(move || rename_params(&params, &b, &WEBPACK_NAMES)) {
                Ok(Some((nb, r))) => {
                    renamed = r;
                    nb
                }
                _ => body,
            }
        } else {
            body
        };
        let (text, rewrites, printed) = match pretty_print("module.js", &body, level, limits) {
            Ok((t, n)) => (t, n, true),
            Err(e) => {
                note = Some(format!("kept as written: {e}"));
                (body, 0, false)
            }
        };
        if !names.insert(module_path_key(&file_name)) {
            let (stem, ext) = match file_name.rsplit_once('.') {
                Some((stem, ext)) if !stem.is_empty() && !stem.ends_with('/') && !ext.is_empty() => (stem.to_string(), format!(".{ext}")),
                _ => (file_name.clone(), ".js".to_string()),
            };
            let hash = m.sha256.get(..8).unwrap_or("dup");
            file_name = format!("{stem}.{hash}{ext}");
            let mut occurrence = module_index;
            while !names.insert(module_path_key(&file_name)) {
                file_name = format!("{stem}.{hash}.{occurrence}{ext}");
                occurrence += 1;
            }
        }
        let header = format!("// Recovered from a {} bundle: module {:?} (bytes {}..{} of the original file).\n", m.format, m.id, m.start, m.end);
        let header = if renamed.is_empty() {
            header
        } else {
            format!("{header}// Renamed: {}\n", renamed.iter().map(|(a, b)| format!("{a} -> {b}")).collect::<Vec<_>>().join(", "))
        };
        out.push(RecoveredModule {
            id: m.id.clone(),
            format: m.format.clone(),
            file_name,
            text: format!("{header}{text}"),
            source_sha256: m.sha256.clone(),
            start: m.start,
            end: m.end,
            renamed,
            rewrites,
            printed,
            note,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_testkit::electron as fx;

    #[test]
    fn rewrites_are_safe_and_parse() {
        let (out, n) = pretty_print("a.js", "function f(){return a(),b(),!0}x=!1,y=void 0;", RewriteLevel::Standard, &Limits::default()).unwrap();
        assert!(n >= 3, "{out}");
        assert!(out.contains("true") && out.contains("false") && out.contains("undefined"), "{out}");
        assert!(out.contains("x = false;\ny = undefined;") || out.contains("x = false;"), "{out}");
        // A local binding named undefined blocks the void rewrite.
        let (out, _) = pretty_print("b.js", "var undefined=1;x=void 0;", RewriteLevel::Standard, &Limits::default()).unwrap();
        assert!(out.contains("void 0"), "{out}");
        // Splitting would turn an object literal into a block: left alone.
        let (out, _) = pretty_print("c.js", "a(),{b:1}.b;", RewriteLevel::Standard, &Limits::default()).unwrap();
        assert!(out.contains(','), "{out}");
        let (plain, n) = pretty_print("d.js", "x=!0", RewriteLevel::None, &Limits::default()).unwrap();
        assert_eq!(n, 0);
        assert!(plain.contains("!0"));
    }

    #[test]
    fn webpack_modules_become_files_with_renamed_params() {
        let a = crate::analyze("179.js", fx::WEBPACK_CHUNK, &Limits::default());
        let rec = recover_modules(fx::WEBPACK_CHUNK, &a.bundle.modules, RewriteLevel::Standard, &Limits::default());
        let m = rec.iter().find(|m| m.id == "512").unwrap();
        assert_eq!(m.file_name, "512.js");
        assert!(m.text.contains("__webpack_require__(87)"), "{}", m.text);
        assert!(m.text.contains("module.exports = function"), "{}", m.text);
        assert_eq!(m.renamed.len(), 3);
        let b = rec.iter().find(|m| m.id == "87").unwrap();
        assert!(b.text.contains("exports.a = 41"), "{}", b.text);
    }

    #[test]
    fn renames_respect_shorthand_and_collisions() {
        let (body, r) = rename_params(&["e".into(), "t".into()], "var o={e};t.x=o;", &WEBPACK_NAMES).unwrap();
        assert!(body.contains("{e: module}") || body.contains("{ e: module }"), "{body}");
        assert!(body.contains("exports.x"));
        assert_eq!(r.len(), 2);
        // `module` already used in the body: that rename is skipped.
        let got = rename_params(&["e".into()], "module.hot; e.exports=1", &WEBPACK_NAMES);
        assert!(got.is_none());
    }

    #[test]
    fn safe_paths() {
        assert_eq!(safe_module_path("./src/../../etc/passwd", ".js"), "src/etc/passwd.js");
        assert_eq!(safe_module_path("/abs/x.js", ".js"), "abs/x.js");
        assert_eq!(safe_module_path("webpack:///./src/a.ts", ".js"), "src/a.ts");
        assert_eq!(safe_module_path("C:\\x\\y", ".js"), "x/y.js");
        assert_eq!(safe_module_path("..", ".js"), "module.js");
        assert_eq!(safe_module_path("12", ".js"), "12.js");
        assert_eq!(safe_module_path(".hidden", ".js"), "hidden.js");
        assert!(!has_known_module_ext(".JS"));
        assert!(!has_known_module_ext("src/.TS"));
        assert!(has_known_module_ext("src/a.TS"));
    }
}

#[cfg(test)]
mod sequence_directives_regression {
    use super::*;

    #[test]
    fn splitting_does_not_introduce_use_strict() {
        for source in [r#"function f(){ "hello", "use strict"; return this; }"#, r#""hello", "use strict"; globalThis.answer = this;"#] {
            let (out, _) = pretty_print("a.cjs", source, RewriteLevel::Standard, &Limits::default()).unwrap();
            assert!(out.contains(','), "{out}");
        }
    }

    #[test]
    fn ordinary_sequences_still_split() {
        let (out, count) = pretty_print("a.cjs", "a(), b();", RewriteLevel::Standard, &Limits::default()).unwrap();
        assert_eq!(count, 1);
        assert!(!out.contains(','), "{out}");
    }
}

#[cfg(test)]
mod module_collisions_regression {
    use super::*;

    /// Repeated webpack registrations retain every recovered module under a distinct path.
    #[test]
    fn repeated_webpack_chunk_modules_have_distinct_paths() {
        let chunk = r#"(self.webpackChunknotes=self.webpackChunknotes||[]).push([[179],{512:function(e){e.exports=1}}]);"#;
        let source = chunk.repeat(3);
        let analysis = crate::analyze("bundle.js", &source, &crate::Limits::default());
        assert_eq!(analysis.bundle.modules.len(), 3);
        let recovered = super::recover_modules(&source, &analysis.bundle.modules, super::RewriteLevel::None, &crate::Limits::default());
        let names: std::collections::HashSet<_> = recovered.iter().map(|m| &m.file_name).collect();
        assert_eq!(names.len(), 3);
    }

    /// Hash collisions and module ids matching generated names never share an output path.
    #[test]
    fn repeated_hashes_have_distinct_output_paths() {
        let source = "globalThis.answer = 1;";
        let module = BundleModule {
            id: "same.js".into(),
            format: "webpack".into(),
            chunk_ids: vec![],
            start: 0,
            end: source.len() as u32,
            body: Some((0, source.len() as u32)),
            line: 1,
            params: vec![],
            deps: vec![],
            sha256: "0123456789abcdef".into(),
            fingerprint: String::new(),
        };
        let mut modules = vec![module.clone(); 4];
        modules[3].id = "same.01234567.js".into();
        let recovered = recover_modules(source, &modules, RewriteLevel::None, &Limits::default());
        let names: std::collections::HashSet<_> = recovered.iter().map(|m| &m.file_name).collect();
        assert_eq!(names.len(), recovered.len());
        assert_eq!(recovered[0].file_name, "same.js");
        assert_eq!(recovered[1].file_name, "same.01234567.js");
    }
}
