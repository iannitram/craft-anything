//! Source policy checks, not Rust name resolution: inspect syntax and explicit imports without
//! expanding macros or following re-exports. Only cfg predicates that cannot hold outside tests
//! exempt an item. Comments, strings and attribute tokens aren't executable paths.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{Attribute, Meta, Token};

pub fn read(path: &Path) -> Result<syn::File, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    syn::parse_file(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

pub fn rust_files(dir: &Path, skip: &[&str]) -> Result<Vec<PathBuf>, String> {
    fn walk(dir: &Path, skip: &[&str], in_source: bool, files: &mut Vec<PathBuf>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let kind = entry.file_type().map_err(|e| format!("cannot inspect {}: {e}", path.display()))?;
            if name == ".git" {
                continue;
            }
            // Below src/, these are module names. Elsewhere, exempt only crate boundaries.
            let exempt = !in_source && skip.contains(&name.as_ref()) && (dir.join("Cargo.toml").is_file() || path.join("Cargo.toml").is_file());
            if exempt {
                continue;
            }
            let linked_dir = if kind.is_symlink() {
                std::fs::metadata(&path).map_err(|e| format!("cannot inspect source link {}: {e}", path.display()))?.is_dir()
            } else {
                false
            };
            if kind.is_dir() || linked_dir {
                // Do not silently omit linked modules, or follow links into a directory cycle.
                if kind.is_symlink() {
                    return Err(format!("cannot scan symlinked source directory {}; use a real directory so source policies can inspect it", path.display()));
                }
                walk(&path, skip, in_source || name == "src", files)?;
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(dir, skip, dir.file_name().is_some_and(|n| n == "src"), &mut files)?;
    Ok(files)
}

/// Evaluate only what is known in a non-test build. Other cfg atoms are unknown, so e.g.
/// `any(test, unix)` stays in scope, while `all(unix, test)` cannot be production code.
fn outside_tests(meta: &Meta) -> Option<bool> {
    match meta {
        Meta::Path(path) if path.is_ident("test") => Some(false),
        Meta::List(list) => {
            let args = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated).ok()?;
            if list.path.is_ident("not") && args.len() == 1 {
                return outside_tests(args.first()?).map(|v| !v);
            }
            let all = list.path.is_ident("all");
            if !all && !list.path.is_ident("any") {
                return None;
            }
            let mut unknown = false;
            for arg in args {
                match outside_tests(&arg) {
                    Some(v) if v != all => return Some(v),
                    None => unknown = true,
                    _ => {}
                }
            }
            (!unknown).then_some(all)
        }
        _ => None,
    }
}

pub fn test_only(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("test") || (a.path().is_ident("cfg") && a.parse_args::<Meta>().is_ok_and(|m| outside_tests(&m) == Some(false))))
}

pub fn attrs(item: &syn::Item) -> &[Attribute] {
    match item {
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::ExternCrate(i) => &i.attrs,
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::ForeignMod(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::TraitAlias(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Union(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

#[derive(Clone, Copy)]
pub enum Policy {
    Process,
    Ui,
    Session,
}

impl Policy {
    fn forbids(self, path: &[String]) -> bool {
        let parts: Vec<&str> = path.iter().map(String::as_str).collect();
        let pair = |a, b| parts.windows(2).any(|p| p == [a, b]);
        match self {
            Self::Process => parts.starts_with(&["std", "process", "Command"]),
            Self::Ui => {
                pair("std", "process")
                    || pair("File", "create")
                    || parts.iter().any(|p| matches!(*p, "OpenOptions" | "write_atomic"))
                    || parts.iter().take(parts.len().saturating_sub(1)).any(|p| matches!(*p, "Launch" | "Process"))
                    || parts.windows(2).any(|p| {
                        p.first() == Some(&"fs")
                            && p.get(1).is_some_and(|p| *p == "write" || p.starts_with("create_dir") || p.starts_with("remove_") || *p == "rename")
                    })
            }
            Self::Session => pair("Session", "default"),
        }
    }
}

/// (full imported path, local name). Globs and cross-file re-exports are intentionally not
/// resolved. Explicit module and type aliases in the current lexical scope are followed.
fn imports(tree: &syn::UseTree, prefix: &[String], out: &mut Vec<(Vec<String>, String)>) {
    match tree {
        syn::UseTree::Path(p) => {
            let mut path = prefix.to_vec();
            path.push(p.ident.to_string());
            imports(&p.tree, &path, out);
        }
        syn::UseTree::Group(g) => {
            for tree in &g.items {
                imports(tree, prefix, out);
            }
        }
        syn::UseTree::Name(n) => {
            let mut path = prefix.to_vec();
            if n.ident != "self" {
                path.push(n.ident.to_string());
            }
            if let Some(name) = path.last() {
                out.push((path.clone(), name.clone()));
            }
        }
        syn::UseTree::Rename(r) => {
            let mut path = prefix.to_vec();
            if r.ident != "self" {
                path.push(r.ident.to_string());
            }
            out.push((path, r.rename.to_string()));
        }
        syn::UseTree::Glob(_) => {}
    }
}

struct Finder {
    policy: Policy,
    aliases: BTreeMap<String, Vec<String>>,
    found: Vec<(usize, String)>,
}

impl Finder {
    fn resolved(&self, mut path: Vec<String>) -> Vec<String> {
        // Each substitution consumes a distinct alias; cycles cannot resolve to a std path.
        let mut seen = std::collections::BTreeSet::new();
        while let Some(first) = path.first() {
            if !seen.insert(first.clone()) {
                break;
            }
            let Some(replacement) = self.aliases.get(first) else { break };
            let mut next = replacement.clone();
            next.extend(path.into_iter().skip(1));
            path = next;
        }
        path
    }

    fn record(&mut self, path: Vec<String>, line: usize) {
        let path = self.resolved(path);
        if self.policy.forbids(&path) {
            self.found.push((line, path.join("::")));
        }
    }

    fn bind<'a>(&mut self, items: impl Iterator<Item = &'a syn::Item>) {
        for item in items {
            if let syn::Item::Use(u) = item
                && (matches!(self.policy, Policy::Session) || !test_only(&u.attrs))
            {
                let mut bindings = Vec::new();
                imports(&u.tree, &[], &mut bindings);
                for (path, name) in bindings {
                    if path != [name.clone()] {
                        self.aliases.insert(name, path);
                    }
                }
            }
        }
    }
}

impl<'ast> Visit<'ast> for Finder {
    fn visit_file(&mut self, file: &'ast syn::File) {
        if !matches!(self.policy, Policy::Session) && test_only(&file.attrs) {
            return;
        }
        self.bind(file.items.iter());
        visit::visit_file(self, file);
    }

    fn visit_item(&mut self, item: &'ast syn::Item) {
        if matches!(self.policy, Policy::Session) || !test_only(attrs(item)) {
            visit::visit_item(self, item);
        }
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if matches!(self.policy, Policy::Session) || !test_only(&item.attrs) {
            visit::visit_impl_item_fn(self, item);
        }
    }

    fn visit_trait_item_fn(&mut self, item: &'ast syn::TraitItemFn) {
        if matches!(self.policy, Policy::Session) || !test_only(&item.attrs) {
            visit::visit_trait_item_fn(self, item);
        }
    }

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        // A module does not inherit its parent's imports.
        let parent = std::mem::take(&mut self.aliases);
        if let Some((_, items)) = &item.content {
            self.bind(items.iter());
        }
        visit::visit_item_mod(self, item);
        self.aliases = parent;
    }

    fn visit_block(&mut self, block: &'ast syn::Block) {
        let parent = self.aliases.clone();
        self.bind(block.stmts.iter().filter_map(|s| if let syn::Stmt::Item(i) = s { Some(i) } else { None }));
        visit::visit_block(self, block);
        self.aliases = parent;
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        let mut paths = Vec::new();
        imports(&item.tree, &[], &mut paths);
        for (path, _) in paths {
            self.record(path, item.use_token.span.start().line);
        }
    }

    fn visit_attribute(&mut self, _: &'ast Attribute) {}

    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.record(path.segments.iter().map(|s| s.ident.to_string()).collect(), path.segments.first().map_or(0, |s| s.ident.span().start().line));
        visit::visit_path(self, path);
    }
}

pub fn problems(file: &syn::File, policy: Policy) -> Vec<(usize, String)> {
    let mut finder = Finder { policy, aliases: BTreeMap::new(), found: Vec::new() };
    finder.visit_file(file);
    finder.found.sort();
    finder.found.dedup();
    finder.found
}

/// Resolve only module-level conventional files and literal `#[path]` declarations. Missing files
/// may belong to an inactive cfg; the compiler decides that. Other inspection errors fail.
fn modules(items: &[syn::Item], dir: &Path, attr_dir: &Path, test: bool, out: &mut Vec<(PathBuf, bool)>) -> Result<(), String> {
    for item in items {
        let syn::Item::Mod(module) = item else { continue };
        let test = test || test_only(&module.attrs);
        let name = module.ident.to_string();
        if let Some((_, items)) = &module.content {
            modules(items, &dir.join(&name), &attr_dir.join(&name), test, out)?;
        } else {
            let custom = module.attrs.iter().find_map(|a| {
                if !a.path().is_ident("path") {
                    return None;
                }
                let Meta::NameValue(value) = &a.meta else { return None };
                let syn::Expr::Lit(value) = &value.value else { return None };
                let syn::Lit::Str(value) = &value.lit else { return None };
                Some(attr_dir.join(value.value()))
            });
            let candidates = custom.map_or_else(|| vec![dir.join(format!("{name}.rs")), dir.join(name).join("mod.rs")], |path| vec![path]);
            for path in candidates {
                match path.canonicalize() {
                    Ok(path) => {
                        out.push((path, test));
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(format!("cannot inspect module {}: {e}", path.display())),
                }
            }
        }
    }
    Ok(())
}

fn references(path: &Path, file: &syn::File) -> Result<Vec<(PathBuf, bool)>, String> {
    let parent = path.parent().unwrap_or(Path::new(""));
    let dir = if path.file_stem().is_some_and(|n| n == "lib" || n == "main" || n == "mod") {
        parent.to_path_buf()
    } else {
        parent.join(path.file_stem().unwrap_or_default())
    };
    let mut refs = Vec::new();
    modules(&file.items, &dir, parent, false, &mut refs)?;
    Ok(refs)
}

/// Track explicit module declarations so an out-of-line test module has the same exemption as
/// an inline one. A file also used by production stays in scope. Unreferenced files are checked
/// conservatively; macro-generated modules and cross-crate imports aren't inferred.
pub fn test_files(files: &[(PathBuf, syn::File)]) -> Result<std::collections::BTreeSet<PathBuf>, String> {
    let mut incoming: BTreeMap<PathBuf, Vec<(PathBuf, bool)>> = BTreeMap::new();
    let mut tests = std::collections::BTreeSet::new();
    let mut roots = std::collections::BTreeSet::new();
    for (path, file) in files {
        let path = path.canonicalize().map_err(|e| format!("cannot inspect {}: {e}", path.display()))?;
        if test_only(&file.attrs) {
            tests.insert(path.clone());
        }
        let parent = path.parent().unwrap_or(Path::new(""));
        // A reference from a test module does not make a conventional crate entry test-only.
        let entry = parent.file_name().is_some_and(|n| n == "src") && path.file_stem().is_some_and(|n| n == "lib" || n == "main");
        let bin = parent.file_name().is_some_and(|n| n == "bin") && parent.parent().and_then(Path::file_name).is_some_and(|n| n == "src")
            || (path.file_stem().is_some_and(|n| n == "main")
                && parent.parent().and_then(Path::file_name).is_some_and(|n| n == "bin")
                && parent.parent().and_then(Path::parent).and_then(Path::file_name).is_some_and(|n| n == "src"));
        if entry || bin {
            roots.insert(path.clone());
        }
        for (target, test) in references(&path, file)? {
            incoming.entry(target).or_default().push((path.clone(), test));
        }
    }
    loop {
        let before = tests.len();
        for (path, refs) in &incoming {
            if !roots.contains(path) && refs.iter().all(|(source, test)| *test || tests.contains(source)) {
                tests.insert(path.clone());
            }
        }
        if tests.len() == before {
            return Ok(tests);
        }
    }
}

/// Include explicit module files even beneath otherwise-exempt directories. Canonical paths
/// bound discovery to the workspace and stop cyclic or shared module references repeating.
pub fn read_all(mut paths: Vec<PathBuf>, root: &Path) -> Result<Vec<(PathBuf, syn::File)>, String> {
    let workspace = root.canonicalize().map_err(|e| format!("cannot inspect workspace {}: {e}", root.display()))?;
    let mut seen = std::collections::BTreeSet::new();
    let mut files = Vec::new();
    while let Some(path) = paths.pop() {
        let canonical = path.canonicalize().map_err(|e| format!("cannot inspect source {}: {e}", path.display()))?;
        let relative =
            canonical.strip_prefix(&workspace).map_err(|_| format!("source {} is outside the workspace; source policies cannot inspect it", path.display()))?;
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let path = root.join(relative);
        let file = read(&path)?;
        paths.extend(references(&canonical, &file)?.into_iter().map(|(path, _)| path));
        files.push((path, file));
    }
    Ok(files)
}
