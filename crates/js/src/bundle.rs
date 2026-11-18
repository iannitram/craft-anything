//! Bundle splitting: find the module tables bundlers emit and cut them into modules, from the
//! syntax tree only (nothing runs). Handles webpack (bootstrap tables and chunk `push`es),
//! esbuild (`__commonJS` / `__esm` wrappers, minified or not) and browserify. Scope-hoisted
//! bundles (Rollup, Vite production chunks) have no module boundaries left to recover.

use crate::facts::{Key, static_str};
use crate::fingerprint::Fingerprint;
use crate::model::{BundleInfo, BundleModule};
use crate::util::{LineIndex, sha256_hex, slice};
use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::Scoping;
use oxc_syntax::number::ToJsString;
use oxc_syntax::scope::ScopeFlags;
use std::collections::{HashMap, HashSet};

pub const MAX_MODULES: usize = 50_000;

pub(crate) struct BundleVisitor<'s> {
    src: &'s str,
    lines: &'s LineIndex,
    scoping: Option<&'s Scoping>,
    webpack_hint: bool,
    pub info: BundleInfo,
    /// esbuild wrapper binding → module id, for dependency resolution.
    esbuild_vars: HashMap<Key, String>,
}

/// A function-ish table value.
enum Func<'b, 'a> {
    Function(&'b Function<'a>),
    Arrow(&'b ArrowFunctionExpression<'a>),
}

fn as_func<'b, 'a>(e: &'b Expression<'a>) -> Option<Func<'b, 'a>> {
    match e.get_inner_expression() {
        Expression::FunctionExpression(f) => Some(Func::Function(f)),
        Expression::ArrowFunctionExpression(a) => Some(Func::Arrow(a)),
        _ => None,
    }
}

impl Func<'_, '_> {
    fn params(&self) -> Vec<String> {
        let p = match self {
            Func::Function(f) => &f.params,
            Func::Arrow(a) => &a.params,
        };
        p.items.iter().map(|i| i.pattern.get_identifier_name().map_or_else(|| "_".to_string(), |n| n.to_string())).collect()
    }

    fn param_count(&self) -> usize {
        match self {
            Func::Function(f) => f.params.items.len(),
            Func::Arrow(a) => a.params.items.len(),
        }
    }

    fn param_binding(&self, index: usize, scoping: Option<&Scoping>) -> Option<Key> {
        let params = match self {
            Func::Function(f) => &f.params,
            Func::Arrow(a) => &a.params,
        };
        let BindingPattern::BindingIdentifier(id) = &params.items.get(index)?.pattern else { return None };
        match scoping {
            Some(_) => id.symbol_id.get().map(Key::Sym),
            None => Some(Key::Name(id.name.to_string())),
        }
    }

    fn span(&self) -> oxc_span::Span {
        match self {
            Func::Function(f) => f.span,
            Func::Arrow(a) => a.span,
        }
    }

    fn body(&self) -> Option<(u32, u32)> {
        let b = match self {
            Func::Function(f) => f.body.as_ref()?.span,
            Func::Arrow(a) => match &a.body {
                ArrowFunctionBody::FunctionBody(b) => b.span,
                _ => return None,
            },
        };
        (b.end > b.start + 1).then(|| (b.start + 1, b.end - 1))
    }
}

/// Collects calls to the module's `require` parameter and esbuild wrapper calls.
struct DepVisitor<'n> {
    require_binding: Option<Key>,
    scoping: Option<&'n Scoping>,
    esbuild_vars: Option<&'n HashMap<Key, String>>,
    deps: Vec<String>,
    esbuild: Vec<Key>,
    esbuild_seen: HashSet<Key>,
}

impl<'a> Visit<'a> for DepVisitor<'_> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Expression::Identifier(id) = &it.callee {
            let n = id.name.as_str();
            let is_loader = self.require_binding.as_ref().is_some_and(|binding| match (binding, self.scoping) {
                (Key::Sym(symbol), Some(sc)) => id.reference_id.get().and_then(|rid| sc.get_reference(rid).symbol_id()) == Some(*symbol),
                (Key::Name(name), None) => name == n,
                _ => false,
            });
            if is_loader && it.arguments.len() == 1 {
                let dep = it.arguments.first().and_then(|a| a.as_expression()).and_then(|e| match e.get_inner_expression() {
                    Expression::NumericLiteral(x) => Some(x.value.to_js_string()),
                    other => static_str(other).map(str::to_string),
                });
                if let Some(d) = dep
                    && !self.deps.contains(&d)
                    && self.deps.len() < 4096
                {
                    self.deps.push(d);
                }
            } else if it.arguments.is_empty()
                && self.esbuild.len() < 4096
                && let Some(wrappers) = self.esbuild_vars
            {
                // Minifiers rename the wrapper. Resolve by binding, not by its spelling, so a
                // short name also works without confusing a local parameter with the wrapper.
                let key = match self.scoping {
                    Some(sc) => id.reference_id.get().and_then(|rid| sc.get_reference(rid).symbol_id()).map(Key::Sym),
                    None => Some(Key::Name(n.to_string())),
                };
                if let Some(key) = key
                    && wrappers.contains_key(&key)
                    && self.esbuild_seen.insert(key.clone())
                {
                    self.esbuild.push(key);
                }
            }
        }
        walk::walk_call_expression(self, it);
    }
}

/// Revisit only discovered module bodies once all wrapper bindings, including forward
/// declarations, are known. Unrelated calls never occupy the bounded dependency buffer.
struct EsbuildDeps<'s> {
    scoping: Option<&'s Scoping>,
    wrappers: &'s HashMap<Key, String>,
    modules: &'s mut [BundleModule],
    module_indices: HashMap<(u32, u32), usize>,
}

impl EsbuildDeps<'_> {
    fn collect(&mut self, f: &Func<'_, '_>) -> bool {
        let span = f.span();
        let Some(&idx) = self.module_indices.get(&(span.start, span.end)) else { return false };
        let mut dv = DepVisitor {
            require_binding: None,
            scoping: self.scoping,
            esbuild_vars: Some(self.wrappers),
            deps: Vec::new(),
            esbuild: Vec::new(),
            esbuild_seen: HashSet::new(),
        };
        match f {
            Func::Function(func) => dv.visit_function(func, ScopeFlags::Function),
            Func::Arrow(a) => dv.visit_arrow_function_expression(a),
        }
        if let Some(module) = self.modules.get_mut(idx) {
            for key in dv.esbuild {
                if let Some(dep) = self.wrappers.get(&key)
                    && !module.deps.contains(dep)
                {
                    module.deps.push(dep.clone());
                }
            }
        }
        true
    }
}

impl<'a> Visit<'a> for EsbuildDeps<'_> {
    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        if !self.collect(&Func::Function(it)) {
            walk::walk_function(self, it, flags);
        }
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        if !self.collect(&Func::Arrow(it)) {
            walk::walk_arrow_function_expression(self, it);
        }
    }
}

impl<'s> BundleVisitor<'s> {
    pub fn new(src: &'s str, lines: &'s LineIndex, webpack_hint: bool, scoping: Option<&'s Scoping>) -> Self {
        BundleVisitor { src, lines, scoping, webpack_hint, info: BundleInfo::default(), esbuild_vars: HashMap::new() }
    }

    /// Resolve esbuild wrapper calls to module ids once every wrapper is known.
    pub fn finish(mut self, program: &Program<'_>) -> BundleInfo {
        if !self.esbuild_vars.is_empty() {
            let module_indices = self.info.modules.iter().enumerate().map(|(idx, m)| ((m.start, m.end), idx)).collect();
            EsbuildDeps { scoping: self.scoping, wrappers: &self.esbuild_vars, modules: &mut self.info.modules, module_indices }.visit_program(program);
        }
        self.info
    }

    fn add(&mut self, id: String, format: &str, chunk_ids: &[String], f: &Func<'_, '_>) -> Option<usize> {
        if self.info.modules.len() >= MAX_MODULES {
            self.info.truncated = true;
            return None;
        }
        let span = f.span();
        let params = f.params();
        let require_binding = match format {
            "webpack" | "webpack-chunk" => f.param_binding(2, self.scoping),
            "browserify" => f.param_binding(0, self.scoping),
            _ => None,
        };
        let mut dv =
            DepVisitor { require_binding, scoping: self.scoping, esbuild_vars: None, deps: Vec::new(), esbuild: Vec::new(), esbuild_seen: HashSet::new() };
        let mut fp = Fingerprint::new();
        match f {
            Func::Function(func) => {
                if dv.require_binding.is_some() {
                    dv.visit_function(func, ScopeFlags::Function);
                }
                fp.visit_function(func, ScopeFlags::Function);
            }
            Func::Arrow(a) => {
                if dv.require_binding.is_some() {
                    dv.visit_arrow_function_expression(a);
                }
                fp.visit_arrow_function_expression(a);
            }
        }
        let text = slice(self.src, span.start, span.end);
        self.info.modules.push(BundleModule {
            id,
            format: format.into(),
            chunk_ids: chunk_ids.to_vec(),
            start: span.start,
            end: span.end,
            body: f.body(),
            line: self.lines.line(span.start),
            params,
            deps: dv.deps,
            sha256: sha256_hex(text.as_bytes()),
            fingerprint: fp.finish(),
        });
        Some(self.info.modules.len() - 1)
    }

    fn key_id(&self, key: &PropertyKey<'_>) -> Option<String> {
        match key {
            PropertyKey::NumericLiteral(n) => Some(n.value.to_js_string()),
            PropertyKey::StringLiteral(s) => Some(s.value.to_string()),
            PropertyKey::StaticIdentifier(i) => Some(i.name.to_string()),
            _ => None,
        }
    }

    /// An object whose values are (almost) all small-arity functions: a webpack module map.
    fn object_is_table(o: &ObjectExpression<'_>) -> bool {
        let total = o.properties.len();
        if total < 2 {
            return false;
        }
        let good = o
            .properties
            .iter()
            .filter(|p| matches!(p, ObjectPropertyKind::ObjectProperty(p) if !p.computed && as_func(&p.value).is_some_and(|f| f.param_count() <= 3)))
            .count();
        good * 10 >= total * 9
    }

    /// Every property is a module factory: a small-arity function under a plain key.
    fn object_has_factories(o: &ObjectExpression<'_>) -> bool {
        o.properties
            .iter()
            .all(|p| matches!(p, ObjectPropertyKind::ObjectProperty(p) if !p.computed && as_func(&p.value).is_some_and(|f| f.param_count() <= 3)))
    }

    /// `(g = g || [])` or `(g ||= [])`: how webpack's chunk loading reaches its global array,
    /// whatever the global is called.
    fn chunk_global(e: &Expression<'_>) -> bool {
        let Expression::AssignmentExpression(a) = e.get_inner_expression() else { return false };
        let empty = |e: &Expression<'_>| matches!(e.get_inner_expression(), Expression::ArrayExpression(x) if x.elements.is_empty());
        match a.operator {
            AssignmentOperator::Assign => {
                matches!(a.right.get_inner_expression(), Expression::LogicalExpression(l) if l.operator == LogicalOperator::Or && empty(&l.right))
            }
            AssignmentOperator::LogicalOr => empty(&a.right),
            _ => false,
        }
    }

    fn array_is_table(a: &ArrayExpression<'_>) -> bool {
        let funcs = a.elements.iter().filter(|e| e.as_expression().and_then(as_func).is_some_and(|f| f.param_count() <= 3)).count();
        let holes = a.elements.iter().filter(|e| matches!(e, ArrayExpressionElement::Elision(_))).count();
        funcs >= 2 && (funcs + holes) * 10 >= a.elements.len() * 9
    }

    fn take_object(&mut self, o: &ObjectExpression<'_>, format: &str, chunk_ids: &[String]) {
        for p in &o.properties {
            let ObjectPropertyKind::ObjectProperty(p) = p else { continue };
            let (Some(id), Some(f)) = (self.key_id(&p.key), as_func(&p.value)) else { continue };
            if self.add(id, format, chunk_ids, &f).is_none() {
                return;
            }
        }
    }

    fn take_array(&mut self, a: &ArrayExpression<'_>, format: &str, chunk_ids: &[String]) {
        for (i, e) in a.elements.iter().enumerate() {
            let Some(f) = e.as_expression().and_then(as_func) else { continue };
            if self.add(i.to_string(), format, chunk_ids, &f).is_none() {
                return;
            }
        }
    }

    /// browserify: `{1:[function(require,module,exports){…},{"./dep":2}], …}`
    fn take_browserify(&mut self, o: &ObjectExpression<'_>) -> bool {
        let entries: Vec<_> = o
            .properties
            .iter()
            .filter_map(|p| match p {
                ObjectPropertyKind::ObjectProperty(p) => match p.value.get_inner_expression() {
                    Expression::ArrayExpression(a) if a.elements.len() == 2 => {
                        let f = a.elements.first()?.as_expression().and_then(as_func)?;
                        let deps = match a.elements.get(1)?.as_expression()?.get_inner_expression() {
                            Expression::ObjectExpression(d) => d,
                            _ => return None,
                        };
                        Some((&p.key, f, deps))
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect();
        if entries.len() < 2 || entries.len() * 10 < o.properties.len() * 9 {
            return false;
        }
        for (key, f, deps) in entries {
            let Some(id) = self.key_id(key) else { continue };
            let Some(idx) = self.add(id, "browserify", &[], &f) else { return true };
            let mapped: Vec<String> = deps
                .properties
                .iter()
                .filter_map(|p| match p {
                    ObjectPropertyKind::ObjectProperty(p) => match p.value.get_inner_expression() {
                        Expression::NumericLiteral(n) => Some(n.value.to_js_string()),
                        other => static_str(other).map(str::to_string),
                    },
                    _ => None,
                })
                .collect();
            if let Some(m) = self.info.modules.get_mut(idx) {
                m.deps = mapped;
            }
        }
        true
    }

    /// esbuild wrapper: `__commonJS({"src/a.js"(exports, module) {…}})` (any callee name when minified).
    fn esbuild_wrapper<'b, 'a>(call: &'b CallExpression<'a>) -> Option<(String, Func<'b, 'a>, &'static str)> {
        if call.arguments.len() != 1 {
            return None;
        }
        let Expression::ObjectExpression(o) = call.arguments.first()?.as_expression()?.get_inner_expression() else { return None };
        if o.properties.len() != 1 {
            return None;
        }
        let ObjectPropertyKind::ObjectProperty(p) = o.properties.first()? else { return None };
        let key = match &p.key {
            PropertyKey::StringLiteral(s) => s.value.to_string(),
            _ => return None,
        };
        if !(key.contains('/')
            || key.ends_with(".js")
            || key.ends_with(".ts")
            || key.ends_with(".mjs")
            || key.ends_with(".cjs")
            || key.ends_with(".jsx")
            || key.ends_with(".tsx"))
        {
            return None;
        }
        let f = as_func(&p.value)?;
        let kind = match (&call.callee, f.param_count()) {
            (Expression::Identifier(id), _) if id.name == "__esm" => "esbuild-esm",
            (_, 0) => "esbuild-esm",
            _ => "esbuild-cjs",
        };
        Some((key, f, kind))
    }
}

impl<'a> Visit<'a> for BundleVisitor<'_> {
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some(Expression::CallExpression(call)) = it.init.as_ref().map(|e| e.get_inner_expression())
            && let Some((key, f, kind)) = Self::esbuild_wrapper(call)
        {
            if let BindingPattern::BindingIdentifier(var) = &it.id {
                let binding = match (self.scoping, var.symbol_id.get()) {
                    (Some(_), Some(symbol)) => Key::Sym(symbol),
                    _ => Key::Name(var.name.to_string()),
                };
                self.esbuild_vars.insert(binding, key.clone());
            }
            self.add(key, kind, &[], &f);
            return;
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        // (self.webpackChunkapp = self.webpackChunkapp || []).push([[ids], {modules}, runtime?])
        if let Expression::StaticMemberExpression(m) = it.callee.get_inner_expression()
            && m.property.name == "push"
            && it.arguments.len() == 1
            && let Some(Expression::ArrayExpression(payload)) = it.arguments.first().and_then(|a| a.as_expression()).map(|e| e.get_inner_expression())
            && payload.elements.len() >= 2
            && let Some(Expression::ArrayExpression(ids)) = payload.elements.first().and_then(|e| e.as_expression()).map(|e| e.get_inner_expression())
        {
            let chunk_ids: Vec<String> = ids
                .elements
                .iter()
                .filter_map(|e| match e.as_expression()?.get_inner_expression() {
                    Expression::NumericLiteral(n) => Some(n.value.to_js_string()),
                    other => static_str(other).map(str::to_string),
                })
                .collect();
            // A table of two or more factories is shape enough; a single one also needs webpack
            // named in the file or webpack's `(g = g || []).push`, since `queue.push([[1], { a }])`
            // is ordinary code too.
            let single_module = |o: &ObjectExpression<'_>| {
                Self::object_is_table(o) || (o.properties.len() == 1 && (self.webpack_hint || Self::chunk_global(&m.object)) && Self::object_has_factories(o))
            };
            match payload.elements.get(1).and_then(|e| e.as_expression()).map(|e| e.get_inner_expression()) {
                Some(Expression::ObjectExpression(o)) if single_module(o) => {
                    self.info.chunk_ids.extend(chunk_ids.iter().cloned());
                    self.take_object(o, "webpack-chunk", &chunk_ids);
                    return;
                }
                Some(Expression::ArrayExpression(a)) if Self::array_is_table(a) => {
                    self.info.chunk_ids.extend(chunk_ids.iter().cloned());
                    self.take_array(a, "webpack-chunk", &chunk_ids);
                    return;
                }
                _ => {}
            }
        }
        if let Some((key, f, kind)) = Self::esbuild_wrapper(it) {
            self.add(key, kind, &[], &f);
            return;
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_object_expression(&mut self, it: &ObjectExpression<'a>) {
        if self.webpack_hint && Self::object_is_table(it) {
            self.take_object(it, "webpack", &[]);
            return;
        }
        if self.take_browserify(it) {
            return;
        }
        walk::walk_object_expression(self, it);
    }

    fn visit_array_expression(&mut self, it: &ArrayExpression<'a>) {
        if self.webpack_hint && Self::array_is_table(it) {
            self.take_array(it, "webpack", &[]);
            return;
        }
        walk::walk_array_expression(self, it);
    }
}

/// Name the bundlers suggested by runtime markers and split results.
pub(crate) fn classify(markers: &std::collections::BTreeMap<String, u32>, info: &BundleInfo, strings: &[String]) -> Vec<String> {
    let has = |k: &str| markers.get(k).copied().unwrap_or(0) > 0;
    let fmt = |f: &str| info.modules.iter().any(|m| m.format.starts_with(f));
    let mut out = Vec::new();
    if has("__webpack_require__") || has("webpackChunk") || has("__webpack_modules__") || fmt("webpack") {
        out.push("webpack");
    }
    if has("__turbopack_require__") || has("__turbopack_context__") {
        out.push("turbopack");
    }
    if has("__vitePreload") || has("__vite__mapDeps") || has("__vite_ssr_import__") {
        out.push("vite");
    }
    if has("__commonJS") || has("__esm") || has("__toESM") || fmt("esbuild") {
        out.push("esbuild");
    }
    if has("parcelRequire") {
        out.push("parcel");
    }
    if fmt("browserify") {
        out.push("browserify");
    }
    if has("__swcpack_require__") {
        out.push("swcpack");
    }
    if strings.iter().any(|s| s == "System.register") {
        out.push("systemjs");
    }
    if has("__importDefault") || has("__createBinding") || has("__exportStar") {
        out.push("tsc");
    }
    if has("_interopRequireDefault") || has("_interopRequireWildcard") {
        out.push("babel");
    }
    out.into_iter().map(String::from).collect()
}
