//! A lightweight C# declaration scanner (namespaces, types, methods, constructors, fields,
//! properties, events; nested types and generic arity) and a reconciliation of scanned
//! declarations against metadata members. It reads declarations only: method bodies are skipped,
//! so it says nothing about behaviour.

use super::model::Members;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

const MAX_TOKENS: usize = 4_000_000;
const MAX_DEPTH: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Ident { name: String, verbatim: bool },
    Punct(char),
    Arrow,
    Lit,
}

#[derive(Debug, Clone)]
struct T {
    tok: Tok,
    line: u32,
}

fn tokenize(src: &str) -> Vec<T> {
    let b: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut line = 1u32;
    let mut line_start = true;
    let at = |i: usize| b.get(i).copied().unwrap_or('\0');
    while i < b.len() && out.len() < MAX_TOKENS {
        let c = at(i);
        if c == '\n' {
            line += 1;
            line_start = true;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if line_start && c == '#' {
            while i < b.len() && at(i) != '\n' {
                i += 1;
            }
            continue;
        }
        line_start = false;
        if c == '/' && at(i + 1) == '/' {
            while i < b.len() && at(i) != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && at(i + 1) == '*' {
            i += 2;
            while i < b.len() && !(at(i) == '*' && at(i + 1) == '/') {
                if at(i) == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
            continue;
        }
        // Strings: raw ("""…"""), verbatim (@"…"), interpolated ($"…"), regular.
        let mut j = i;
        let mut verbatim = false;
        while matches!(at(j), '@' | '$') && j < i + 3 {
            if at(j) == '@' {
                verbatim = true;
            }
            j += 1;
        }
        if at(j) == '"' {
            let line0 = line;
            let mut quotes = 0;
            while at(j + quotes) == '"' {
                quotes += 1;
            }
            if quotes >= 3 {
                let mut k = j + quotes;
                loop {
                    if k >= b.len() {
                        break;
                    }
                    let mut n = 0;
                    while at(k + n) == '"' {
                        n += 1;
                    }
                    if n >= quotes {
                        k += n;
                        break;
                    }
                    if at(k) == '\n' {
                        line += 1;
                    }
                    k += n.max(1);
                }
                i = k;
            } else {
                let mut k = j + 1;
                while k < b.len() {
                    let ch = at(k);
                    if ch == '\n' {
                        line += 1;
                    }
                    if verbatim {
                        if ch == '"' {
                            if at(k + 1) == '"' {
                                k += 2;
                                continue;
                            }
                            break;
                        }
                    } else if ch == '\\' {
                        k += 2;
                        continue;
                    } else if ch == '"' || ch == '\n' {
                        break;
                    }
                    k += 1;
                }
                i = k + 1;
            }
            out.push(T { tok: Tok::Lit, line: line0 });
            continue;
        }
        if c == '\'' {
            let mut k = i + 1;
            while k < b.len() && k < i + 12 {
                if at(k) == '\\' {
                    k += 2;
                    continue;
                }
                if at(k) == '\'' || at(k) == '\n' {
                    break;
                }
                k += 1;
            }
            i = k + 1;
            out.push(T { tok: Tok::Lit, line });
            continue;
        }
        if c == '@' {
            let next = at(i + 1);
            if (next.is_alphanumeric() && !next.is_ascii_digit()) || next == '_' {
                let line0 = line;
                let start = i + 1;
                i += 2;
                while i < b.len() && (at(i).is_alphanumeric() || at(i) == '_') {
                    i += 1;
                }
                let word: String = b.get(start..i).map(|s| s.iter().collect()).unwrap_or_default();
                out.push(T { tok: Tok::Ident { name: word, verbatim: true }, line: line0 });
                continue;
            }
        }
        if c.is_alphanumeric() || c == '_' {
            let start = i;
            i += 1;
            while i < b.len() && (at(i).is_alphanumeric() || at(i) == '_') {
                i += 1;
            }
            let word: String = b.get(start..i).map(|s| s.iter().collect()).unwrap_or_default();
            if word.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
                out.push(T { tok: Tok::Lit, line });
            } else {
                out.push(T { tok: Tok::Ident { name: word, verbatim: false }, line });
            }
            continue;
        }
        if c == '=' && at(i + 1) == '>' {
            out.push(T { tok: Tok::Arrow, line });
            i += 2;
            continue;
        }
        out.push(T { tok: Tok::Punct(c), line });
        i += 1;
    }
    out
}

/// One scanned declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CsMember {
    /// method, constructor, field, property, event
    pub kind: &'static str,
    /// Declaring type, dotted (`Ns.Outer.Inner`).
    pub type_name: String,
    /// Metadata-style name (`.ctor`, `.cctor`, `Finalize`, `Item` for indexers).
    pub name: String,
    pub generic_arity: usize,
    pub params: Vec<String>,
    #[serde(rename = "type")]
    pub ty: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CsType {
    pub full_name: String,
    pub kind: String,
    pub generic_arity: usize,
    pub line: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CsScan {
    pub types: Vec<CsType>,
    pub members: Vec<CsMember>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
enum Scope {
    Namespace(String),
    /// `positional` holds a record's positional properties, added when the body closes unless the body
    /// declares a member of the same name; `start` is the member count when the body opened.
    Type {
        full: String,
        simple: String,
        is_enum: bool,
        positional: Vec<CsMember>,
        start: usize,
    },
    Block,
}

const MODIFIERS: &[&str] = &[
    "public",
    "private",
    "protected",
    "internal",
    "static",
    "readonly",
    "const",
    "volatile",
    "virtual",
    "override",
    "abstract",
    "sealed",
    "extern",
    "unsafe",
    "new",
    "async",
    "partial",
    "fixed",
    "required",
    "file",
    "implicit",
    "explicit",
];
const TYPE_KW: &[&str] = &["class", "struct", "interface", "enum", "record", "delegate"];

/// Identifier text of `t`, whether written plain (`foo`) or verbatim (`@class`).
fn ident(t: &T) -> Option<&str> {
    match &t.tok {
        Tok::Ident { name, .. } => Some(name),
        _ => None,
    }
}

/// Non-verbatim identifier text of `t`, used when matching C# keywords and modifiers.
fn kw(t: &T) -> Option<&str> {
    match &t.tok {
        Tok::Ident { name, verbatim: false } => Some(name),
        _ => None,
    }
}

fn is_p(t: &T, c: char) -> bool {
    t.tok == Tok::Punct(c)
}

/// Join tokens into a type string, preserving `@` on verbatim predefined type keywords (`@string`).
fn join_type(ts: &[T]) -> String {
    let mut s = String::new();
    for t in ts {
        match &t.tok {
            Tok::Ident { name: w, verbatim } => {
                if s.chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    s.push(' ');
                }
                if *verbatim && is_predefined_type_keyword(w) {
                    s.push('@');
                }
                s.push_str(w);
            }
            Tok::Punct(',') => s.push_str(", "),
            Tok::Punct(c) => s.push(*c),
            Tok::Arrow => s.push_str("=>"),
            Tok::Lit => s.push('…'),
        }
    }
    s
}

/// Join tokens into a type string.
fn join(ts: &[T]) -> String {
    let mut s = String::new();
    for t in ts {
        match &t.tok {
            Tok::Ident { name: w, .. } => {
                if s.chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    s.push(' ');
                }
                s.push_str(w);
            }
            Tok::Punct(',') => s.push_str(", "),
            Tok::Punct(c) => s.push(*c),
            Tok::Arrow => s.push_str("=>"),
            Tok::Lit => s.push('…'),
        }
    }
    s
}

/// The CLR name of a C# operator, from the symbol after `operator` (and optional `checked` keyword).
/// Checked `++`, `--`, `*` and `/` have their own names; `+` and `-` depend on how many parameters
/// they take, so the caller names those, and `implicit`/`explicit` conversions are named from the modifiers.
fn operator_name(symbol: &str, checked: bool) -> &'static str {
    if checked {
        return match symbol {
            "++" => "op_CheckedIncrement",
            "--" => "op_CheckedDecrement",
            "*" => "op_CheckedMultiply",
            "/" => "op_CheckedDivision",
            _ => "operator",
        };
    }
    match symbol {
        "==" => "op_Equality",
        "!=" => "op_Inequality",
        "*" => "op_Multiply",
        "/" => "op_Division",
        "%" => "op_Modulus",
        "&" => "op_BitwiseAnd",
        "|" => "op_BitwiseOr",
        "^" => "op_ExclusiveOr",
        "!" => "op_LogicalNot",
        "~" => "op_OnesComplement",
        "++" => "op_Increment",
        "--" => "op_Decrement",
        "<<" => "op_LeftShift",
        ">>" => "op_RightShift",
        ">>>" => "op_UnsignedRightShift",
        "<" => "op_LessThan",
        ">" => "op_GreaterThan",
        "<=" => "op_LessThanOrEqual",
        ">=" => "op_GreaterThanOrEqual",
        "true" => "op_True",
        "false" => "op_False",
        _ => "operator",
    }
}

/// Split at top-level commas.
fn split_commas(ts: &[T]) -> Vec<&[T]> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, t) in ts.iter().enumerate() {
        match t.tok {
            Tok::Punct('(' | '<' | '[' | '{') => depth += 1,
            Tok::Punct(')' | '>' | ']' | '}') => depth -= 1,
            Tok::Punct(',') if depth == 0 => {
                out.push(ts.get(start..i).unwrap_or(&[]));
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < ts.len() {
        out.push(ts.get(start..).unwrap_or(&[]));
    }
    out
}

/// Index of the token matching the opener at `open` (same nesting kind), if any.
fn matching(ts: &[T], open: usize, o: char, c: char) -> Option<usize> {
    let mut depth = 0i32;
    for (i, t) in ts.iter().enumerate().skip(open) {
        if is_p(t, o) {
            depth += 1;
        } else if is_p(t, c) {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Drop leading attributes (`[…]`) and modifiers.
fn strip_prefix(ts: &[T]) -> (&[T], Vec<String>) {
    let mut i = 0;
    let mut mods = Vec::new();
    loop {
        match ts.get(i) {
            Some(t) if is_p(t, '[') => match matching(ts, i, '[', ']') {
                Some(e) => i = e + 1,
                None => break,
            },
            Some(t) if kw(t).is_some_and(|w| MODIFIERS.contains(&w)) => {
                mods.push(kw(t).unwrap_or_default().to_string());
                i += 1;
            }
            _ => break,
        }
    }
    (ts.get(i..).unwrap_or(&[]), mods)
}

fn param_type(p: &[T]) -> String {
    let (p, _) = strip_prefix(p);
    // Default values.
    let p = match p.iter().position(|t| is_p(t, '=')) {
        Some(e) => p.get(..e).unwrap_or(p),
        None => p,
    };
    let mut byref = false;
    let mut start = 0;
    while let Some(w) = p.get(start).and_then(kw) {
        match w {
            "ref" | "out" | "in" => {
                byref = true;
                start += 1;
            }
            "this" | "params" | "scoped" | "readonly" => start += 1,
            _ => break,
        }
    }
    let body = p.get(start..).unwrap_or(&[]);
    // The last identifier is the parameter name.
    let ty = if body.len() >= 2 && body.last().and_then(ident).is_some() { body.get(..body.len() - 1).unwrap_or(&[]) } else { body };
    let s = join(ty);
    if byref { format!("ref {s}") } else { s }
}

/// Properties a positional record declares through its parameter list `params`.
fn positional_properties(kind: &str, params: &[T], type_name: &str, line: u32) -> Vec<CsMember> {
    if !kind.starts_with("record") {
        return Vec::new();
    }
    split_commas(params)
        .into_iter()
        .filter_map(|p| {
            let (p, _) = strip_prefix(p);
            let p = p.get(..p.iter().position(|t| is_p(t, '=')).unwrap_or(p.len())).unwrap_or(&[]);
            let name = p.last().and_then(ident)?;
            if p.len() < 2 {
                return None;
            }
            Some(CsMember {
                kind: "property",
                type_name: type_name.to_string(),
                name: name.to_string(),
                generic_arity: 0,
                params: vec![],
                ty: param_type(p),
                line,
            })
        })
        .collect()
}

struct Scanner {
    scopes: Vec<Scope>,
    file_ns: String,
    out: CsScan,
}

impl Scanner {
    fn prefix(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.file_ns.is_empty() {
            parts.push(self.file_ns.clone());
        }
        for s in &self.scopes {
            match s {
                Scope::Namespace(n) => parts.push(n.clone()),
                Scope::Type { simple, .. } => parts.push(simple.clone()),
                Scope::Block => {}
            }
        }
        parts.join(".")
    }

    fn current_type(&self) -> Option<(&str, &str, bool)> {
        match self.scopes.last()? {
            Scope::Type { full, simple, is_enum, .. } => Some((full, simple, *is_enum)),
            _ => None,
        }
    }

    /// Parse a type declaration header into its name, kind, generic arity, and line number.
    fn type_decl(&self, h: &[T]) -> Option<(String, String, usize, u32)> {
        let paren = h.iter().position(|t| is_p(t, '(')).unwrap_or(h.len());
        let kw_at = h.iter().take(paren).position(|t| kw(t).is_some_and(|w| TYPE_KW.contains(&w)))?;
        // A declaration keyword cannot follow an assignment or arrow operator.
        if h.iter().take(kw_at).any(|t| is_p(t, '=') || t.tok == Tok::Arrow) {
            return None;
        }
        let kind = kw(h.get(kw_at)?)?;
        if kind == "delegate" {
            if h.get(kw_at + 1).is_some_and(|t| is_p(t, '*')) {
                return None;
            }
            let is_reserved = |tok: &T| kw(tok).is_some_and(|w| w == "delegate" || w == "ref" || w == "void" || MODIFIERS.contains(&w) || TYPE_KW.contains(&w));
            let mut target = None;
            for p in (kw_at + 1)..h.len() {
                if !is_p(&h[p], '(') {
                    continue;
                }
                if p > kw_at + 1 && is_p(&h[p - 1], '>') {
                    let mut depth = 0i32;
                    let mut open_lt = None;
                    for i in (kw_at + 1..p).rev() {
                        let t = &h[i];
                        if is_p(t, '>') {
                            depth += 1;
                        } else if is_p(t, '<') {
                            depth -= 1;
                            if depth == 0 {
                                open_lt = Some(i);
                                break;
                            }
                        }
                    }
                    if let Some(lt) = open_lt
                        && lt > kw_at + 1
                        && let Some(name_tok) = h.get(lt - 1)
                        && let Some(name) = ident(name_tok)
                        && !is_reserved(name_tok)
                    {
                        let arity = split_commas(h.get(lt + 1..p - 1).unwrap_or(&[])).len();
                        target = Some((name.to_string(), arity, name_tok.line));
                        break;
                    }
                } else if let Some(name_tok) = h.get(p - 1)
                    && let Some(name) = ident(name_tok)
                    && !is_reserved(name_tok)
                {
                    target = Some((name.to_string(), 0, name_tok.line));
                    break;
                }
            }
            let (name, arity, line) = target?;
            return Some((name, "delegate".into(), arity, line));
        }
        let mut k = kw_at + 1;
        let mut kind = kind.to_string();
        if kind == "record" && h.get(k).and_then(kw).is_some_and(|w| w == "struct" || w == "class") {
            kind = format!("record {}", kw(h.get(k)?)?);
            k += 1;
        }
        let name = ident(h.get(k)?)?.to_string();
        let arity = if h.get(k + 1).is_some_and(|t| is_p(t, '<')) {
            let e = matching(h, k + 1, '<', '>')?;
            split_commas(h.get(k + 2..e)?).len()
        } else {
            0
        };
        Some((name, kind, arity, h.get(k)?.line))
    }

    /// Record a member declared by header `h` inside the current type.
    fn member(&mut self, h: &[T], has_body: bool) {
        let Some((full, simple, is_enum)) = self.current_type().map(|(a, b, c)| (a.to_string(), b.to_string(), c)) else { return };
        let (rest, mods) = strip_prefix(h);
        if rest.is_empty() {
            return;
        }
        let line = rest.first().map(|t| t.line).unwrap_or(0);
        if is_enum {
            for part in split_commas(rest) {
                if let Some(n) = part.first().and_then(ident) {
                    self.push(CsMember { kind: "field", type_name: full.clone(), name: n.into(), generic_arity: 0, params: vec![], ty: simple.clone(), line });
                }
            }
            return;
        }
        // Top-level '=' or '=>' (default parameter values sit inside parentheses).
        let mut depth = 0i32;
        let mut eq = rest.len();
        let mut paren = None;
        for (i, t) in rest.iter().enumerate() {
            match t.tok {
                Tok::Punct('(') => {
                    if depth == 0 && paren.is_none() {
                        paren = Some(i);
                    }
                    depth += 1;
                }
                Tok::Punct(')') => depth -= 1,
                Tok::Punct('=') | Tok::Arrow if depth == 0 => {
                    eq = i;
                    break;
                }
                _ => {}
            }
        }
        let op_at = rest.iter().position(|t| kw(t) == Some("operator"));
        if let Some(op) = op_at {
            let p = rest.iter().skip(op).position(|t| is_p(t, '(')).map(|x| x + op);
            let params: Vec<String> = p
                .and_then(|p| matching(rest, p, '(', ')').map(|c| (p, c)))
                .map(|(p, c)| split_commas(rest.get(p + 1..c).unwrap_or(&[])).into_iter().map(param_type).collect())
                .unwrap_or_default();
            let ret = join(rest.get(..op).unwrap_or(&[]));
            let op_tokens = p.and_then(|p| rest.get(op + 1..p)).unwrap_or(&[]);
            let is_checked = op_tokens.first().is_some_and(|t| kw(t) == Some("checked"));
            let conversion = mods.iter().any(|m| matches!(m.as_str(), "implicit" | "explicit"));
            let op_tail = if is_checked { op_tokens.get(1..).unwrap_or(&[]) } else { op_tokens };
            let symbol = if conversion { join_type(op_tail) } else { join(op_tail) };
            let name = if mods.iter().any(|m| m.as_str() == "implicit") {
                "op_Implicit".to_string()
            } else if mods.iter().any(|m| m.as_str() == "explicit") {
                if is_checked { "op_CheckedExplicit".to_string() } else { "op_Explicit".to_string() }
            } else {
                match (symbol.as_str(), is_checked, params.len()) {
                    ("+", false, 1) => "op_UnaryPlus".to_string(),
                    ("+", false, _) => "op_Addition".to_string(),
                    // Checked unary `+` isn't a C# operator: only the binary form is checked.
                    ("+", true, 1) => "operator".to_string(),
                    ("+", true, _) => "op_CheckedAddition".to_string(),
                    ("-", false, 1) => "op_UnaryNegation".to_string(),
                    ("-", false, _) => "op_Subtraction".to_string(),
                    ("-", true, 1) => "op_CheckedUnaryNegation".to_string(),
                    ("-", true, _) => "op_CheckedSubtraction".to_string(),
                    _ => operator_name(&symbol, is_checked).to_string(),
                }
            };
            let ty = if conversion { symbol } else { ret };
            self.push(CsMember { kind: "method", type_name: full, name, generic_arity: 0, params, ty, line });
            return;
        }
        if let Some(p) = paren.filter(|&p| p < eq) {
            let decl = rest;
            let close = matching(decl, p, '(', ')').unwrap_or(decl.len());
            let params: Vec<String> = split_commas(decl.get(p + 1..close).unwrap_or(&[])).into_iter().map(param_type).filter(|s| !s.is_empty()).collect();
            let mut name_at = p.checked_sub(1);
            let mut arity = 0;
            if name_at.and_then(|i| decl.get(i)).is_some_and(|t| is_p(t, '>')) {
                // Walk back to the matching '<'.
                let mut depth = 0i32;
                for i in (0..p).rev() {
                    let Some(t) = decl.get(i) else { break };
                    if is_p(t, '>') {
                        depth += 1;
                    } else if is_p(t, '<') {
                        depth -= 1;
                        if depth == 0 {
                            arity = split_commas(decl.get(i + 1..p - 1).unwrap_or(&[])).len();
                            name_at = i.checked_sub(1);
                            break;
                        }
                    }
                }
            }
            let Some(ni) = name_at else { return };
            let Some(name) = decl.get(ni).and_then(ident) else { return };
            let is_finalizer = ni > 0 && decl.get(ni - 1).is_some_and(|t| is_p(t, '~'));
            let raw = join(decl.get(..ni).unwrap_or(&[]));
            let (kind, mname, ret) = if is_finalizer {
                ("method", "Finalize".to_string(), "void".to_string())
            } else if name == simple && raw.is_empty() {
                ("constructor", if mods.iter().any(|m| m == "static") { ".cctor".to_string() } else { ".ctor".to_string() }, "void".to_string())
            } else if let Some(q) = raw.strip_suffix('.') {
                // Explicit interface implementation: `void IFoo.Bar()`.
                let (ret, qual) = q.rsplit_once(' ').unwrap_or(("", q));
                ("method", format!("{qual}.{name}"), ret.to_string())
            } else {
                ("method", name.to_string(), raw)
            };
            self.push(CsMember { kind, type_name: full, name: mname, generic_arity: arity, params, ty: ret, line });
            return;
        }
        let decl = rest.get(..eq).unwrap_or(&[]);
        let is_event = decl.first().and_then(kw) == Some("event");
        let decl = if is_event { decl.get(1..).unwrap_or(&[]) } else { decl };
        // Indexers.
        if let Some(ti) = decl.iter().position(|t| kw(t) == Some("this"))
            && decl.get(ti + 1).is_some_and(|t| is_p(t, '['))
        {
            let close = matching(decl, ti + 1, '[', ']').unwrap_or(decl.len());
            let params = split_commas(decl.get(ti + 2..close).unwrap_or(&[])).into_iter().map(param_type).collect();
            self.push(CsMember {
                kind: "property",
                type_name: full,
                name: "Item".into(),
                generic_arity: 0,
                params,
                ty: join(decl.get(..ti).unwrap_or(&[])),
                line,
            });
            return;
        }
        // Properties (with an accessor body or `=>`) and fields/events (possibly several declarators).
        let is_property = !is_event && (has_body || rest.get(eq).is_some_and(|t| t.tok == Tok::Arrow));
        let whole = if is_event { rest.get(1..).unwrap_or(&[]) } else { rest };
        let parts: Vec<&[T]> = if is_property { vec![decl] } else { split_commas(whole) };
        let cut = |p: &'_ [T]| -> usize { p.iter().position(|t| is_p(t, '=')).unwrap_or(p.len()) };
        let first = parts.first().copied().map(|p| p.get(..cut(p)).unwrap_or(&[])).unwrap_or(&[]);
        if first.len() < 2 {
            return;
        }
        let ty = join(first.get(..first.len() - 1).unwrap_or(&[]));
        for (i, part) in parts.iter().enumerate() {
            let part = part.get(..cut(part)).unwrap_or(&[]);
            let name_tok = if i == 0 { part.last() } else { part.first() };
            let Some(name) = name_tok.and_then(ident) else { continue };
            let kind = if is_event {
                "event"
            } else if is_property {
                "property"
            } else {
                "field"
            };
            self.push(CsMember { kind, type_name: full.clone(), name: name.into(), generic_arity: 0, params: vec![], ty: ty.clone(), line });
            if is_property {
                break;
            }
        }
    }

    fn push(&mut self, m: CsMember) {
        if self.out.members.len() < 1_000_000 {
            self.out.members.push(m);
        }
    }

    /// Scan token stream into types, members, and namespace scopes.
    fn run(mut self, toks: &[T]) -> CsScan {
        let mut cur: Vec<T> = Vec::new();
        let mut expr_depth = 0i32;
        let mut i = 0usize;
        while let Some(t) = toks.get(i) {
            i += 1;
            // Inside a method/accessor block: only track braces.
            if matches!(self.scopes.last(), Some(Scope::Block)) {
                if is_p(t, '{') {
                    if self.scopes.len() < MAX_DEPTH {
                        self.scopes.push(Scope::Block);
                    }
                } else if is_p(t, '}') {
                    self.scopes.pop();
                }
                continue;
            }
            let in_expr = cur.iter().any(|x| x.tok == Tok::Arrow || is_p(x, '='));
            if in_expr && expr_depth > 0 {
                if is_p(t, '{') {
                    expr_depth += 1;
                } else if is_p(t, '}') {
                    expr_depth -= 1;
                }
                continue;
            }
            match t.tok {
                Tok::Punct('[') if cur.is_empty() || cur.last().is_some_and(|x| is_p(x, ']')) => {
                    // Attribute: keep it in cur (strip_prefix drops it) but skip its contents.
                    let mut depth = 1;
                    cur.push(t.clone());
                    while let Some(n) = toks.get(i) {
                        i += 1;
                        if is_p(n, '[') {
                            depth += 1;
                        } else if is_p(n, ']') {
                            depth -= 1;
                            if depth == 0 {
                                cur.push(n.clone());
                                break;
                            }
                        }
                    }
                }
                Tok::Punct('{') if in_expr => {
                    expr_depth = 1;
                }
                Tok::Punct('{') => {
                    let h = std::mem::take(&mut cur);
                    if self.scopes.len() >= MAX_DEPTH {
                        self.out.warnings.push("nesting too deep; the rest of the file was skipped".into());
                        break;
                    }
                    let (ns_head, _) = strip_prefix(&h);
                    if ns_head.first().and_then(kw) == Some("namespace") {
                        let name = join(ns_head.get(1..).unwrap_or(&[]));
                        self.scopes.push(Scope::Namespace(name));
                    } else if let Some((name, kind, arity, line)) = self.type_decl(&h) {
                        let prefix = self.prefix();
                        let full = if prefix.is_empty() { name.clone() } else { format!("{prefix}.{name}") };
                        if self.out.types.len() < 200_000 {
                            self.out.types.push(CsType { full_name: full.clone(), kind: kind.clone(), generic_arity: arity, line });
                        }
                        // Positional records declare a constructor and properties.
                        let mut positional = Vec::new();
                        if kind != "delegate"
                            && let Some(p) = h.iter().position(|x| is_p(x, '('))
                        {
                            let close = matching(&h, p, '(', ')').unwrap_or(h.len());
                            let list = h.get(p + 1..close).unwrap_or(&[]);
                            let params: Vec<String> = split_commas(list).into_iter().map(param_type).collect();
                            self.push(CsMember {
                                kind: "constructor",
                                type_name: full.clone(),
                                name: ".ctor".into(),
                                generic_arity: 0,
                                params,
                                ty: "void".into(),
                                line,
                            });
                            positional = positional_properties(&kind, list, &full, line);
                        }
                        let start = self.out.members.len();
                        self.scopes.push(Scope::Type { full, simple: name, is_enum: kind == "enum", positional, start });
                    } else if self.current_type().is_some() {
                        self.member(&h, true);
                        self.scopes.push(Scope::Block);
                    } else {
                        self.scopes.push(Scope::Block);
                    }
                }
                Tok::Punct('}') => {
                    if self.current_type().is_some_and(|(_, _, e)| e) && !cur.is_empty() {
                        let h = std::mem::take(&mut cur);
                        self.member(&h, false);
                    }
                    cur.clear();
                    expr_depth = 0;
                    if let Some(Scope::Type { full, positional, start, .. }) = self.scopes.pop() {
                        // A member of the same name replaces the synthesized property.
                        for m in positional {
                            let declared = self.out.members.get(start..).unwrap_or(&[]).iter().any(|d| d.type_name == full && d.name == m.name);
                            if !declared {
                                self.push(m);
                            }
                        }
                    }
                }
                Tok::Punct(';') => {
                    let h = std::mem::take(&mut cur);
                    expr_depth = 0;
                    let (ns_head, _) = strip_prefix(&h);
                    if ns_head.first().and_then(kw) == Some("namespace") {
                        if self.scopes.is_empty() {
                            self.file_ns = join(ns_head.get(1..).unwrap_or(&[]));
                        }
                    } else if let Some((name, kind, arity, line)) = self.type_decl(&h) {
                        let prefix = self.prefix();
                        let full = if prefix.is_empty() { name } else { format!("{prefix}.{name}") };
                        if kind != "delegate"
                            && let Some(p) = h.iter().position(|x| is_p(x, '('))
                        {
                            let close = matching(&h, p, '(', ')').unwrap_or(h.len());
                            let list = h.get(p + 1..close).unwrap_or(&[]);
                            let params: Vec<String> = split_commas(list).into_iter().map(param_type).collect();
                            self.push(CsMember {
                                kind: "constructor",
                                type_name: full.clone(),
                                name: ".ctor".into(),
                                generic_arity: 0,
                                params,
                                ty: "void".into(),
                                line,
                            });
                            for m in positional_properties(&kind, list, &full, line) {
                                self.push(m);
                            }
                        }
                        if self.out.types.len() < 200_000 {
                            self.out.types.push(CsType { full_name: full, kind, generic_arity: arity, line });
                        }
                    } else if self.current_type().is_some() {
                        self.member(&h, false);
                    }
                }
                _ => cur.push(t.clone()),
            }
        }
        self.out
    }
}

/// Scan C# source for declarations.
pub fn scan(src: &str) -> CsScan {
    let toks = tokenize(src);
    let mut s = Scanner { scopes: Vec::new(), file_ns: String::new(), out: CsScan::default() }.run(&toks);
    if toks.len() >= MAX_TOKENS {
        s.warnings.push(format!("stopped after {MAX_TOKENS} tokens"));
    }
    s
}

/// Simplify a type for comparison: drop namespaces, map system type names to C# keywords, remove
/// spaces.
pub fn norm_type(s: &str) -> String {
    norm_type_scoped(s, false, None).text
}

/// Simplify a type for comparison but keep namespaces, so `A.Result` and `B.Result` stay distinct.
/// `global::` is dropped and metadata's nested-type separator becomes a dot, so a qualified source
/// name matches the metadata name.
fn norm_type_qualified(s: &str) -> String {
    norm_type_scoped(s, true, None).text
}

/// Known type definitions and references indexed by `(dotted_full_name, own_generic_arity)`.
type KnownTypes = BTreeSet<(String, usize)>;

/// Normalized C# type together with resolution metadata that keeps explicit `System.*` aliases and
/// `global::` qualifiers distinct from bare contextual keywords during reconciliation.
struct ScopedNorm {
    text: String,
    outer_resolved: bool,
    outer_system_builtin: bool,
    word_allows_fallback: Vec<bool>,
}

/// Resolve a source type expression against `declaring` and `known_types`, optionally mapping
/// unshadowed unqualified CLR built-in names (`String` -> `string`) as under `using System;`.
fn resolve_source_type(s: &str, declaring: &str, known_types: &KnownTypes, expand_system_aliases: bool) -> ScopedNorm {
    norm_type_scoped(s, true, Some((declaring, known_types, expand_system_aliases)))
}

/// True when `name` is a C# reserved predefined type keyword (`string`, `int`, `object`, …) that
/// always denotes its built-in type unless escaped with `@`.
fn is_reserved_predefined_type_keyword(name: &str) -> bool {
    matches!(
        name,
        "void"
            | "bool"
            | "char"
            | "sbyte"
            | "byte"
            | "short"
            | "ushort"
            | "int"
            | "uint"
            | "long"
            | "ulong"
            | "float"
            | "double"
            | "string"
            | "object"
            | "decimal"
    )
}

/// True when `name` is a C# contextual type keyword (`nint`, `nuint`, `dynamic`) that resolves to
/// an in-scope declared type before falling back to its built-in meaning.
fn is_contextual_predefined_type_keyword(name: &str) -> bool {
    matches!(name, "nint" | "nuint" | "dynamic")
}

/// True when `name` is a C# predefined type keyword (`string`, `int`, `nint`, `dynamic`, …).
fn is_predefined_type_keyword(name: &str) -> bool {
    is_reserved_predefined_type_keyword(name) || is_contextual_predefined_type_keyword(name)
}

/// Count top-level generic arguments immediately following a type identifier (`<T, U>` -> `2`).
fn following_generic_arity(after: &str) -> usize {
    let s = after.trim_start();
    if !s.starts_with('<') {
        return 0;
    }
    let mut depth = 0usize;
    let mut brackets = 0usize;
    let mut commas = 0usize;
    for ch in s.chars() {
        match ch {
            '<' if brackets == 0 => depth += 1,
            '>' if brackets == 0 => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return commas + 1;
                }
            }
            '[' | '(' => brackets += 1,
            ']' | ')' => brackets = brackets.saturating_sub(1),
            ',' if depth == 1 && brackets == 0 => commas += 1,
            _ => {}
        }
    }
    0
}

/// Extract the trailing backtick arity of the final segment in `word` (`App.String`1` -> `Some(1)`).
fn explicit_backtick_arity(word: &str) -> Option<usize> {
    let last = word.rsplit('.').next().unwrap_or(word);
    let (_, digits) = last.rsplit_once('`')?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then(|| digits.parse().ok()).flatten()
}

/// Resolve `(name, arity)` against `declaring` and its enclosing types/namespaces if a matching
/// declaration exists in `known_types`.
fn resolve_in_declaring_scope(name: &str, arity: usize, declaring: &str, known_types: &KnownTypes) -> Option<String> {
    if !declaring.is_empty() {
        let mut scope = declaring;
        loop {
            let candidate = format!("{scope}.{name}");
            if known_types.contains(&(candidate.clone(), arity)) {
                return Some(candidate);
            }
            match scope.rsplit_once('.') {
                Some((parent, _)) => scope = parent,
                None => break,
            }
        }
    }
    if !name.contains('.') && known_types.contains(&(name.to_string(), arity)) {
        return Some(name.to_string());
    }
    None
}

/// Collect qualified `(type_name, arity)` pairs referenced inside a metadata type signature so
/// external `TypeRef`s (for example `App.String` inside `List<App.String>`) participate in lookup.
fn collect_signature_types(sig: &str, out: &mut KnownTypes) {
    let cleaned = sig.replace('/', ".");
    let bytes = cleaned.as_bytes();
    let mut i = 0usize;
    while let Some(tail) = cleaned.get(i..) {
        let Some(c) = tail.chars().next() else { break };
        if c.is_alphanumeric() || matches!(c, '_' | '`') || c == '@' {
            let start = i;
            i += c.len_utf8();
            while let Some(next) = cleaned.get(i..).and_then(|r| r.chars().next()) {
                if next.is_alphanumeric() || matches!(next, '_' | '.' | '`' | '@') {
                    i += next.len_utf8();
                } else {
                    break;
                }
            }
            let word = cleaned.get(start..i).unwrap_or("");
            let after = cleaned.get(i..).unwrap_or("");
            if word == "global" && after.trim_start().starts_with("::") {
                let ws = after.len() - after.trim_start().len();
                i += ws + 2;
                continue;
            }
            if word == "ref" || bytes.get(start).is_some_and(u8::is_ascii_digit) {
                continue;
            }
            let dequoted = word.replace('@', "");
            let arity = explicit_backtick_arity(&dequoted).unwrap_or_else(|| following_generic_arity(after));
            let name = strip_arity(&dequoted);
            if name.contains('.') {
                out.insert((name, arity));
            }
        } else {
            i += c.len_utf8();
        }
    }
}

/// Collapse CLI multi-dimensional zero-lower-bound array shapes (`[0...,0...]`) to C# rank commas
/// (`[,]`), while keeping rank-1 `[0...]` and non-zero or sized bounds (`[1...,0...]`, `[0...3]`).
fn collapse_zero_lower_bound_ranks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        out.push_str(rest.get(..open).unwrap_or(""));
        let after_open = rest.get(open + 1..).unwrap_or("");
        if let Some(close) = after_open.find(']') {
            let inside = after_open.get(..close).unwrap_or("");
            let dims: Vec<&str> = inside.split(',').collect();
            if dims.len() >= 2 && dims.iter().all(|d| *d == "0...") {
                out.push('[');
                for _ in 1..dims.len() {
                    out.push(',');
                }
                out.push(']');
                rest = after_open.get(close + 1..).unwrap_or("");
                continue;
            }
        }
        out.push('[');
        rest = after_open;
    }
    out.push_str(rest);
    out
}

/// Normalize a C# or CLI metadata type signature, optionally retaining namespace prefixes and
/// resolving unqualified type names against an active declaring scope.
fn norm_type_scoped(s: &str, keep_namespace: bool, scope_ctx: Option<(&str, &KnownTypes, bool)>) -> ScopedNorm {
    let cleaned = s.replace('/', ".");
    let bytes = cleaned.as_bytes();
    let mut out = String::with_capacity(cleaned.len());
    let mut global_qualified = false;
    let mut after_member_dot = false;
    let mut nesting = 0usize;
    let mut outer_resolved = false;
    let mut outer_system_builtin = false;
    let mut word_allows_fallback = Vec::new();
    let mut i = 0usize;
    while let Some(tail) = cleaned.get(i..) {
        let Some(c) = tail.chars().next() else { break };
        if c == '.' && out.ends_with('>') {
            out.push('.');
            after_member_dot = true;
            global_qualified = false;
            i += 1;
            continue;
        }
        if c.is_alphanumeric() || matches!(c, '_' | '`') || c == '@' {
            let start = i;
            i += c.len_utf8();
            while let Some(next) = cleaned.get(i..).and_then(|r| r.chars().next()) {
                if next.is_alphanumeric() || matches!(next, '_' | '.' | '`' | '@') {
                    i += next.len_utf8();
                } else {
                    break;
                }
            }
            let word = cleaned.get(start..i).unwrap_or("");
            let after = cleaned.get(i..).unwrap_or("");
            let trimmed_after = after.trim_start();
            if word == "global" && trimmed_after.starts_with("::") {
                i += (after.len() - trimmed_after.len()) + 2;
                global_qualified = true;
                continue;
            }
            let had_ws = trimmed_after.len() < after.len();
            let starts_type = trimmed_after.chars().next().is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '(' | '@'));
            // `ref` and `out`/`in` both decode to a leading `ref ` modifier before a type.
            if !global_qualified && !word.starts_with('@') && word == "ref" && had_ws && starts_type {
                out.push_str("ref ");
                word_allows_fallback.push(false);
                continue;
            }
            if bytes.get(start).is_some_and(u8::is_ascii_digit) {
                out.push_str(word);
                global_qualified = false;
                after_member_dot = false;
                continue;
            }
            let verbatim_ident = word.contains('@');
            let dequoted = if verbatim_ident { word.replace('@', "") } else { word.to_string() };
            let word_arity = explicit_backtick_arity(&dequoted).unwrap_or_else(|| following_generic_arity(after));
            let raw_name = strip_arity(&dequoted);
            if !verbatim_ident && !global_qualified && !after_member_dot && !raw_name.contains('.') && is_reserved_predefined_type_keyword(&raw_name) {
                out.push_str(raw_name.as_str());
                word_allows_fallback.push(false);
                global_qualified = false;
                after_member_dot = false;
                continue;
            }
            let mut resolved = raw_name.clone();
            let mut shadowed = false;
            if let Some((declaring, known_types, _)) = scope_ctx
                && !global_qualified
                && !after_member_dot
                && let Some(scoped) = resolve_in_declaring_scope(&raw_name, word_arity, declaring, known_types)
            {
                resolved = scoped;
                shadowed = true;
                if nesting == 0 {
                    outer_resolved = true;
                }
            }
            let simple = resolved.rsplit('.').next().unwrap_or(&resolved);
            let is_system_builtin = !verbatim_ident && !after_member_dot && resolved.strip_prefix("System.").is_some_and(|rest| !rest.contains('.'));
            if nesting == 0 && is_system_builtin && cs_alias(simple).is_some() {
                outer_system_builtin = true;
            }
            let allow_unqualified_system_alias = scope_ctx.is_some_and(|(_, _, expand)| expand)
                && !global_qualified
                && !verbatim_ident
                && !after_member_dot
                && !shadowed
                && !raw_name.contains('.')
                && word_arity == 0;
            let alias = if !keep_namespace {
                if verbatim_ident { None } else { cs_alias(simple) }
            } else if is_system_builtin || allow_unqualified_system_alias {
                cs_alias(simple)
            } else {
                None
            };
            let word_out = alias.unwrap_or(if keep_namespace { resolved.as_str() } else { simple });
            out.push_str(word_out);
            if word_out.chars().next().is_some_and(|ch| !ch.is_ascii_digit() && (ch.is_alphanumeric() || matches!(ch, '_' | '.'))) {
                let allow_fallback = !global_qualified
                    && !after_member_dot
                    && !shadowed
                    && !is_system_builtin
                    && alias.is_none()
                    && !word_out.contains('.')
                    && !is_reserved_predefined_type_keyword(word_out);
                word_allows_fallback.push(allow_fallback);
            }
            global_qualified = false;
            after_member_dot = false;
        } else {
            if !c.is_whitespace() {
                match c {
                    '<' | '[' | '(' => nesting = nesting.saturating_add(1),
                    '>' | ']' | ')' => nesting = nesting.saturating_sub(1),
                    _ => {}
                }
                out.push(c);
                global_qualified = false;
                after_member_dot = false;
            }
            i += c.len_utf8();
        }
    }
    ScopedNorm { text: collapse_zero_lower_bound_ranks(&out), outer_resolved, outer_system_builtin, word_allows_fallback }
}

/// The C# keyword for a CLR built-in type name, if it has one.
fn cs_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "Void" => "void",
        "Boolean" => "bool",
        "Char" => "char",
        "SByte" => "sbyte",
        "Byte" => "byte",
        "Int16" => "short",
        "UInt16" => "ushort",
        "Int32" => "int",
        "UInt32" => "uint",
        "Int64" => "long",
        "UInt64" => "ulong",
        "Single" => "float",
        "Double" => "double",
        "String" => "string",
        "Object" => "object",
        "IntPtr" => "nint",
        "UIntPtr" => "nuint",
        "Decimal" => "decimal",
        "dynamic" => "object",
        _ => return None,
    })
}

/// Remove generic-arity markers (`` `1 ``) from a metadata type name.
fn strip_arity(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut skip_digits = false;
    for c in s.chars() {
        if c == '`' {
            skip_digits = true;
        } else if skip_digits && c.is_ascii_digit() {
            // Part of the arity marker.
        } else {
            skip_digits = false;
            out.push(c);
        }
    }
    out
}

/// Extract the type path without balanced `<...>` generic argument blocks or array/nullable/pointer suffixes.
fn type_path_without_generics(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut angle_depth = 0usize;
    for ch in s.chars() {
        match ch {
            '<' => angle_depth += 1,
            '>' => angle_depth = angle_depth.saturating_sub(1),
            '[' | '?' | '*' if angle_depth == 0 => break,
            _ if angle_depth == 0 && !ch.is_whitespace() => out.push(ch),
            _ => {}
        }
    }
    out
}

/// The innermost type name of a normalized type path, ignoring generic arguments and suffixes.
fn terminal_name(s: &str) -> String {
    let path = type_path_without_generics(s);
    path.rsplit('.').next().unwrap_or(&path).to_string()
}

/// True when the type path contains a `.` qualifier outside generic arguments and suffixes.
fn is_qualified_type(s: &str) -> bool {
    type_path_without_generics(s).contains('.')
}

/// Scope and sibling-overload context for matching a C# conversion operator's target type.
struct ConversionCtx<'a> {
    declaring: &'a str,
    known_types: &'a KnownTypes,
    targets: &'a [(&'a str, &'a str, usize)],
    op_name: &'a str,
    arity: usize,
}

/// Qualified type path without generic arguments or array/nullable/pointer suffixes.
fn outer_type_name(s: &str) -> String {
    type_path_without_generics(s)
}

/// Tokenize a normalized type signature into words and punctuation for structural comparison.
fn type_units(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(tail) = s.get(i..) {
        let Some(c) = tail.chars().next() else { break };
        if c.is_alphanumeric() || matches!(c, '_' | '.') {
            let start = i;
            i += c.len_utf8();
            while let Some(next) = s.get(i..).and_then(|r| r.chars().next()) {
                if next.is_alphanumeric() || matches!(next, '_' | '.') {
                    i += next.len_utf8();
                } else {
                    break;
                }
            }
            out.push(s.get(start..i).unwrap_or(""));
        } else {
            if !c.is_whitespace() {
                out.push(s.get(i..i + c.len_utf8()).unwrap_or(""));
            }
            i += c.len_utf8();
        }
    }
    out
}

/// Compare inner generic arguments and suffixes when the outer short name matched via `using`
/// fallback: already-qualified or built-in-keyword words must match `want` verbatim so `App.String`
/// never collapses to `string` and explicit `System.IntPtr` never matches `Other.nint`.
fn terminal_fallback_compatible(got: &ScopedNorm, want_qualified: &str) -> bool {
    let g_units = type_units(&got.text);
    let w_units = type_units(want_qualified);
    if g_units.len() != w_units.len() || g_units.is_empty() {
        return false;
    }
    let mut word_idx = 0usize;
    for (pos, (gw, ww)) in g_units.iter().zip(w_units.iter()).enumerate() {
        let is_non_digit_word = gw.chars().next().is_some_and(|c| !c.is_ascii_digit() && (c.is_alphanumeric() || matches!(c, '_' | '.')));
        let allow_fallback = if is_non_digit_word {
            let allow = got.word_allows_fallback.get(word_idx).copied().unwrap_or(false);
            word_idx += 1;
            allow
        } else {
            false
        };
        if pos == 0 {
            continue;
        }
        if !allow_fallback || gw.contains('.') || is_reserved_predefined_type_keyword(gw) {
            if gw != ww {
                return false;
            }
        } else if *gw != ww.rsplit('.').next().unwrap_or(ww) {
            return false;
        }
    }
    true
}

/// Match a conversion operator's source target `got` against metadata `want`, returning a priority
/// tier (`0` exact/lexical scope, `1` unshadowed `System` alias, `2` unique terminal-name import
/// with scope-resolved arguments, `3` unique terminal-name import with alias-expanded arguments).
///
/// Enclosing types and namespaces take precedence over `using System;` aliases, so `App.String`
/// shadows `System.String` inside `namespace App`, while an unrelated `Other.String` does not block
/// `String` or `System.Collections.Generic.List<String>` from resolving to `string`.
fn conversion_target_tier(want: &str, got: &str, ctx: &ConversionCtx<'_>) -> Option<u8> {
    let want_qualified = norm_type_qualified(want);
    let got_scoped = resolve_source_type(got, ctx.declaring, ctx.known_types, false);
    if want_qualified == got_scoped.text {
        return Some(0);
    }
    let got_with_aliases = resolve_source_type(got, ctx.declaring, ctx.known_types, true);
    // `global::`, explicit `System.<Builtin>` aliases, and non-verbatim reserved predefined keywords
    // (`string`, `int`, …) are fixed for the outermost type: never expand them against `declaring`
    // or terminal fallback.
    let trimmed_got = got.trim_start();
    let global_outer = trimmed_got.starts_with("global::");
    let outer_head = trimmed_got.split(['<', '[', '?', '*', ' ', '\t']).next().unwrap_or(trimmed_got);
    let keyword_outer = !outer_head.contains(['.', '@']) && is_reserved_predefined_type_keyword(outer_head);
    if !global_outer && !keyword_outer && !got_scoped.outer_resolved && !got_scoped.outer_system_builtin && !ctx.declaring.is_empty() {
        let mut scope = ctx.declaring;
        loop {
            if format!("{scope}.{}", got_scoped.text) == want_qualified {
                return Some(0);
            }
            match scope.rsplit_once('.') {
                Some((parent, _)) => scope = parent,
                None => break,
            }
        }
    }
    if want_qualified == got_with_aliases.text {
        return Some(1);
    }
    if global_outer || keyword_outer || got_scoped.outer_resolved || got_scoped.outer_system_builtin || is_qualified_type(&got_scoped.text) {
        return None;
    }
    let want_terminal = terminal_name(&want_qualified);
    let got_terminal = terminal_name(&got_scoped.text);
    if got_terminal != want_terminal {
        return None;
    }
    let mut same_outer: Vec<String> = ctx
        .targets
        .iter()
        .filter(|(name, _, params)| *name == ctx.op_name && *params == ctx.arity)
        .map(|(_, target, _)| outer_type_name(&norm_type_qualified(target)))
        .filter(|outer| outer.rsplit('.').next().unwrap_or(outer) == want_terminal)
        .collect();
    same_outer.sort();
    same_outer.dedup();
    if same_outer.len() != 1 {
        return None;
    }
    if terminal_fallback_compatible(&got_scoped, &want_qualified) {
        Some(2)
    } else if terminal_fallback_compatible(&got_with_aliases, &want_qualified) {
        Some(3)
    } else {
        None
    }
}

/// Normalise a metadata type name (`Ns.Outer/Inner`1`) to its dotted form and the innermost
/// segment's own generic arity.
fn norm_type_own_arity(s: &str) -> (String, usize) {
    let mut own_arity = 0;
    let parts: Vec<String> = s
        .replace('/', ".")
        .split('.')
        .map(|p| match p.rsplit_once('`') {
            Some((n, a)) => {
                own_arity = a.parse().unwrap_or(0);
                n.to_string()
            }
            None => {
                own_arity = 0;
                p.to_string()
            }
        })
        .collect();
    (parts.join("."), own_arity)
}

/// Normalise a metadata type name (`Ns.Outer/Inner`1`) to the dotted form without arity.
fn norm_type_name(s: &str) -> (String, usize) {
    let mut arity = 0;
    let parts: Vec<String> = s
        .replace('/', ".")
        .split('.')
        .map(|p| match p.rsplit_once('`') {
            Some((n, a)) => {
                arity = a.parse().unwrap_or(0);
                n.to_string()
            }
            None => p.to_string(),
        })
        .collect();
    (parts.join("."), arity)
}

#[derive(Debug, Clone, Serialize)]
pub struct MemberMatch {
    pub kind: &'static str,
    pub type_name: String,
    pub name: String,
    pub metadata: Option<String>,
    pub source: Option<String>,
    pub line: Option<u32>,
    pub token: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Reconciliation {
    pub matched: Vec<MemberMatch>,
    pub signature_mismatch: Vec<MemberMatch>,
    /// In metadata but not declared in the source.
    pub missing: Vec<MemberMatch>,
    /// Declared in the source but not in metadata.
    pub extra: Vec<MemberMatch>,
    /// Members the compiler emits without a declaration (accessors, default constructors…).
    pub implicit: Vec<MemberMatch>,
    pub missing_types: Vec<String>,
    pub extra_types: Vec<String>,
    pub matched_types: usize,
}

/// Explicit interface implementations are named `Iface.Member` (namespaced in metadata).
fn key_name(n: &str) -> &str {
    if n.starts_with('.') { n } else { n.rsplit('.').next().unwrap_or(n) }
}

fn compiler_generated(name: &str) -> bool {
    name.contains('<') || name.contains('$') || name.starts_with("CS$")
}

/// An accessor (`get_P`, `add_E`…) of a property or event in `props`. Explicit interface
/// implementations keep the qualifier in front: `Iface.get_P` belongs to `Iface.P`.
fn is_accessor(name: &str, props: &BTreeSet<&str>) -> bool {
    let (qualifier, local) = match name.rsplit_once('.') {
        Some((q, l)) => (Some(q), l),
        None => (None, name),
    };
    if !["get_", "set_", "add_", "remove_", "raise_"].iter().any(|p| local.starts_with(p)) {
        return false;
    }
    let Some((_, prop)) = local.split_once('_') else { return false };
    match qualifier {
        Some(q) => props.contains(format!("{q}.{prop}").as_str()),
        None => props.contains(prop),
    }
}

/// Check scanned declarations against metadata members.
pub fn reconcile(members: &Members, scan: &CsScan) -> Reconciliation {
    let mut r = Reconciliation::default();
    // Source members by (type, name).
    let mut src: BTreeMap<(String, String), Vec<(usize, &CsMember)>> = BTreeMap::new();
    for (i, m) in scan.members.iter().enumerate() {
        src.entry((m.type_name.clone(), key_name(&m.name).to_string())).or_default().push((i, m));
    }
    let mut used: BTreeSet<usize> = BTreeSet::new();
    let src_types: BTreeSet<String> = scan.types.iter().map(|t| t.full_name.clone()).collect();
    let mut known_types: KnownTypes = scan.types.iter().map(|t| (norm_type_name(&t.full_name).0, t.generic_arity)).collect();
    for ty in &members.types {
        if ty.kind == "module" || compiler_generated(&ty.full_name) {
            continue;
        }
        known_types.insert(norm_type_own_arity(&ty.full_name));
        for m in &ty.methods {
            collect_signature_types(&m.return_type, &mut known_types);
            for p in &m.params {
                collect_signature_types(&p.ty, &mut known_types);
            }
        }
        for f in &ty.fields {
            collect_signature_types(&f.ty, &mut known_types);
        }
        for p in &ty.properties {
            collect_signature_types(&p.ty, &mut known_types);
        }
        for e in &ty.events {
            collect_signature_types(&e.ty, &mut known_types);
        }
    }
    let mut meta_types = BTreeSet::new();
    let src_sig = |m: &CsMember| format!("{}({})", m.name, m.params.join(", "));
    for ty in &members.types {
        if ty.kind == "module" || compiler_generated(&ty.full_name) {
            continue;
        }
        let (tname, _) = norm_type_name(&ty.full_name);
        meta_types.insert(tname.clone());
        if src_types.contains(&tname) {
            r.matched_types += 1;
        } else {
            r.missing_types.push(ty.full_name.clone());
        }
        let props: BTreeSet<&str> = ty.properties.iter().map(|p| p.name.as_str()).chain(ty.events.iter().map(|e| e.name.as_str())).collect();
        let conversion_targets: Vec<(&str, &str, usize)> = ty
            .methods
            .iter()
            .filter(|m| matches!(m.name.as_str(), "op_Implicit" | "op_Explicit" | "op_CheckedExplicit"))
            .map(|m| (m.name.as_str(), m.return_type.as_str(), m.params.len()))
            .collect();
        let mut check = |kind: &'static str, name: &str, params: Vec<String>, ret: &str, meta_text: String, token: String, r: &mut Reconciliation| {
            let mm = |source: Option<&CsMember>, reason: Option<String>| MemberMatch {
                kind,
                type_name: ty.full_name.clone(),
                name: name.to_string(),
                metadata: Some(meta_text.clone()),
                source: source.map(src_sig),
                line: source.map(|s| s.line),
                token: Some(token.clone()),
                reason,
            };
            let cands = src.get(&(tname.clone(), key_name(name).to_string())).cloned().unwrap_or_default();
            let arity = params.len();
            let want: Vec<String> = params.iter().map(|p| norm_type(p)).collect();
            // A conversion's target type is part of its identity: two op_Implicit methods can
            // share their parameters and differ only in the type they convert to.
            let want_ret = matches!(name, "op_Implicit" | "op_Explicit" | "op_CheckedExplicit").then_some(ret);
            let ctx = ConversionCtx { declaring: &tname, known_types: &known_types, targets: &conversion_targets, op_name: name, arity };
            let pick = |exact_params: bool| -> Option<(usize, &CsMember)> {
                let matches_params = |s: &CsMember| {
                    if exact_params { s.params.iter().map(|p| norm_type(p)).collect::<Vec<_>>() == want } else { s.params.len() == want.len() }
                };
                match want_ret {
                    None => cands.iter().copied().find(|(i, s)| !used.contains(i) && matches_params(s)),
                    Some(want_ty) => (0..=3).find_map(|tier| {
                        cands
                            .iter()
                            .copied()
                            .find(|(i, s)| !used.contains(i) && conversion_target_tier(want_ty, &s.ty, &ctx) == Some(tier) && matches_params(s))
                    }),
                }
            };
            if let Some((i, s)) = pick(true) {
                used.insert(i);
                r.matched.push(mm(Some(s), None));
            } else if let Some((i, s)) = pick(false) {
                used.insert(i);
                let got: Vec<String> = s.params.iter().map(|p| norm_type(p)).collect();
                r.signature_mismatch.push(mm(Some(s), Some(format!("parameters ({}) vs ({})", want.join(", "), got.join(", ")))));
            } else if compiler_generated(name)
                || name == ".cctor"
                || (name == ".ctor" && params.is_empty())
                || is_accessor(name, &props)
                || (ty.kind == "delegate" && ["Invoke", "BeginInvoke", "EndInvoke", ".ctor"].contains(&name))
                || (ty.kind == "enum" && name == "value__")
            {
                r.implicit.push(mm(None, Some("emitted by the compiler without a declaration".into())));
            } else {
                r.missing.push(mm(None, None));
            }
        };
        for m in &ty.methods {
            let params: Vec<String> = m.params.iter().map(|p| p.ty.clone()).collect();
            check("method", &m.name, params, &m.return_type, m.signature.clone(), m.token.clone(), &mut r);
        }
        for f in &ty.fields {
            check("field", &f.name, vec![], "", format!("{} {}", f.ty, f.name), f.token.clone(), &mut r);
        }
        for p in &ty.properties {
            check("property", &p.name, vec![], "", format!("{} {}", p.ty, p.name), String::new(), &mut r);
        }
        for e in &ty.events {
            check("event", &e.name, vec![], "", format!("event {} {}", e.ty, e.name), String::new(), &mut r);
        }
    }
    for t in &scan.types {
        if !meta_types.contains(&t.full_name) {
            r.extra_types.push(t.full_name.clone());
        }
    }
    for (i, m) in scan.members.iter().enumerate() {
        if !used.contains(&i) {
            r.extra.push(MemberMatch {
                kind: m.kind,
                type_name: m.type_name.clone(),
                name: m.name.clone(),
                metadata: None,
                source: Some(src_sig(m)),
                line: Some(m.line),
                token: None,
                reason: None,
            });
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dotnet::model::{MethodInfo, Param, PropertyInfo, TypeInfo};

    #[test]
    fn scans_declarations() {
        let s = scan(
            r#"
namespace A.B;
/* comment { */
public sealed class Box<T> : IDisposable where T : class
{
    private readonly List<int> _items = new() { 1, 2 }, _other;
    public int Count { get; set; } = 3;
    public string Name => "x{";
    public T this[int i] => default;
    public event EventHandler Changed;
    public Box() : base() { var s = @"a""b"; }
    static Box() { }
    ~Box() { }
    public static Box<T> Create<U, V>(ref int x, out string y, params object[] rest) where U : struct { return null; }
    void IDisposable.Dispose() { }
    public static bool operator ==(Box<T> a, Box<T> b) => true;
    public static bool operator !=(Box<T> a, Box<T> b) => false;
    public enum Mode { A, B = 2, C }
    public record Point(int X, int Y);
}
"#,
        );
        let names: Vec<(&str, &str)> = s.members.iter().map(|m| (m.kind, m.name.as_str())).collect();
        assert!(names.contains(&("field", "_items")), "{names:?}");
        assert!(names.contains(&("field", "_other")));
        assert!(names.contains(&("property", "Count")));
        assert!(names.contains(&("property", "Name")));
        assert!(names.contains(&("property", "Item")));
        assert!(names.contains(&("event", "Changed")));
        assert!(names.contains(&("constructor", ".ctor")));
        assert!(names.contains(&("constructor", ".cctor")));
        assert!(names.contains(&("method", "Finalize")));
        assert!(names.contains(&("method", "IDisposable.Dispose")));
        assert!(names.contains(&("method", "op_Equality")), "{names:?}");
        assert!(names.contains(&("method", "op_Inequality")), "{names:?}");
        assert!(names.contains(&("field", "C")));
        let create = s.members.iter().find(|m| m.name == "Create").unwrap();
        assert_eq!(create.generic_arity, 2);
        assert_eq!(create.params, vec!["ref int", "ref string", "object[]"]);
        assert_eq!(create.ty, "Box<T>");
        assert_eq!(create.type_name, "A.B.Box");
        let types: Vec<&str> = s.types.iter().map(|t| t.full_name.as_str()).collect();
        assert_eq!(types, vec!["A.B.Box", "A.B.Box.Mode", "A.B.Box.Point"]);
        assert_eq!(s.types[0].generic_arity, 1);
    }

    #[test]
    fn normalises_types() {
        assert_eq!(norm_type("System.Collections.Generic.List<System.Int32>"), "List<int>");
        assert_eq!(norm_type("List<int>"), "List<int>");
        assert_eq!(norm_type("ref int"), "ref int");
        assert_eq!(norm_type_name("Ns.Outer/Inner`1"), ("Ns.Outer.Inner".into(), 1));
    }

    fn method(name: &str) -> MethodInfo {
        MethodInfo {
            token: String::new(),
            name: name.into(),
            key: String::new(),
            signature: name.into(),
            return_type: String::new(),
            params: vec![],
            generic_params: vec![],
            visibility: "private",
            modifiers: vec![],
            code_type: "il",
            internal_call: false,
            pinvoke: false,
            rva: 0,
            body: None,
        }
    }

    fn ty(name: &str, kind: &'static str, methods: &[&str], props: &[&str]) -> TypeInfo {
        TypeInfo {
            token: String::new(),
            full_name: name.into(),
            namespace: String::new(),
            name: name.into(),
            kind,
            visibility: "internal",
            modifiers: vec![],
            extends: None,
            enclosing_type: None,
            generic_params: vec![],
            interfaces: vec![],
            fields: vec![],
            methods: methods.iter().map(|m| method(m)).collect(),
            properties: props.iter().map(|p| PropertyInfo { name: (*p).into(), ty: "int".into() }).collect(),
            events: vec![],
        }
    }

    #[test]
    fn explicit_interface_property_accessors_are_implicit() {
        let members = Members {
            types: vec![
                ty("Interface", "interface", &["get_Property", "set_Property"], &["Property"]),
                ty("Class", "class", &["Interface.get_Property", "Interface.set_Property", ".ctor", "Other.get_X"], &["Interface.Property"]),
            ],
            total_types: 2,
            total_methods: 6,
            total_fields: 0,
            truncated: false,
            warnings: vec![],
        };
        let s = scan("interface Interface { int Property { get; set; } } class Class : Interface { int Interface.Property { get; set; } }");
        let r = reconcile(&members, &s);
        let missing: Vec<&str> = r.missing.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(missing, vec!["Other.get_X"]);
        assert!(r.extra.is_empty(), "{:?}", r.extra);
        assert_eq!(r.matched.iter().filter(|m| m.kind == "property").count(), 2);
        let implicit: Vec<&str> = r.implicit.iter().map(|m| m.name.as_str()).collect();
        assert!(implicit.contains(&"Interface.get_Property"), "{implicit:?}");
        assert!(implicit.contains(&"Interface.set_Property"), "{implicit:?}");
    }

    #[test]
    fn scans_and_reconciles_delegate_declarations() {
        let s = scan(
            r#"
namespace Newtonsoft.Json.Serialization;

public delegate void ExtensionDataSetter(object o, string key, object value);
public delegate TResult Func<out TResult>();
public delegate (int, int) Pair();
public delegate (string Name, int Age) GetUser<T>(T id);

public class Container {
    public delegate bool Filter(int item);
    public Action callback = delegate { };
    public Action<int> onCount = delegate(int x) { };
    public unsafe delegate*<int, void> fp;
}
"#,
        );
        let types: Vec<(&str, &str, usize)> = s.types.iter().map(|t| (t.full_name.as_str(), t.kind.as_str(), t.generic_arity)).collect();
        assert!(types.contains(&("Newtonsoft.Json.Serialization.ExtensionDataSetter", "delegate", 0)), "{types:?}");
        assert!(types.contains(&("Newtonsoft.Json.Serialization.Func", "delegate", 1)), "{types:?}");
        assert!(types.contains(&("Newtonsoft.Json.Serialization.Pair", "delegate", 0)), "{types:?}");
        assert!(types.contains(&("Newtonsoft.Json.Serialization.GetUser", "delegate", 1)), "{types:?}");
        assert!(types.contains(&("Newtonsoft.Json.Serialization.Container.Filter", "delegate", 0)), "{types:?}");
        assert!(!types.iter().any(|(name, _, _)| name.ends_with(".delegate")), "anonymous delegates must not register as types: {types:?}");

        let fields: Vec<(&str, &str)> = s.members.iter().filter(|m| m.kind == "field").map(|m| (m.name.as_str(), m.ty.as_str())).collect();
        assert!(fields.contains(&("callback", "Action")), "{fields:?}");
        assert!(fields.contains(&("onCount", "Action<int>")), "{fields:?}");
        assert!(fields.iter().any(|(name, _)| *name == "fp"), "{fields:?}");

        let members = Members {
            types: vec![ty("Newtonsoft.Json.Serialization.ExtensionDataSetter", "delegate", &[".ctor", "Invoke", "BeginInvoke", "EndInvoke"], &[])],
            total_types: 1,
            total_methods: 4,
            total_fields: 0,
            truncated: false,
            warnings: vec![],
        };
        let r = reconcile(&members, &s);
        assert_eq!(r.matched_types, 1);
        assert!(r.missing_types.is_empty(), "delegate is not missing: {:?}", r.missing_types);
        assert!(r.missing.is_empty(), "synthetic delegate methods are not missing: {:?}", r.missing);
    }

    #[test]
    fn equality_operators_match_their_clr_names() {
        let mut c = ty("C", "class", &["op_Equality", "op_Inequality"], &[]);
        for m in &mut c.methods {
            m.params = vec![Param { name: "a".into(), ty: "C".into() }, Param { name: "b".into(), ty: "C".into() }];
        }
        let members = Members { types: vec![c], total_types: 1, total_methods: 2, total_fields: 0, truncated: false, warnings: vec![] };
        let s = scan("class C { public static bool operator ==(C a, C b) => true; public static bool operator !=(C a, C b) => false; }");
        let r = reconcile(&members, &s);
        assert!(r.missing.is_empty(), "{:?}", r.missing);
        assert!(r.extra.is_empty(), "{:?}", r.extra);
        assert_eq!(r.matched.iter().filter(|m| m.kind == "method").count(), 2, "{:?}", r.matched);
    }

    #[test]
    fn other_operators_get_their_clr_names() {
        let s = scan(
            "class C {
                public static C operator +(C a, C b) => a;
                public static C operator -(C a) => a;
                public static implicit operator int(C c) => 0;
                public static explicit operator C(int i) => null;
                public static bool operator true(C c) => true;
                public static C operator checked *(C a, C b) => a;
                public static C operator checked +(C a, C b) => a;
                public static C operator checked -(C a) => a;
                public static C operator >>>(C a, int b) => a;
                public static explicit operator checked int(C c) => 0;
            }",
        );
        let names: Vec<&str> = s.members.iter().map(|m| m.name.as_str()).collect();
        for want in [
            "op_Addition",
            "op_UnaryNegation",
            "op_Implicit",
            "op_Explicit",
            "op_True",
            "op_CheckedMultiply",
            "op_CheckedAddition",
            "op_CheckedUnaryNegation",
            "op_UnsignedRightShift",
            "op_CheckedExplicit",
        ] {
            assert!(names.contains(&want), "{want} missing: {names:?}");
        }
    }

    #[test]
    fn a_checked_unary_plus_is_not_a_clr_operator() {
        // Not valid C#; the scanner must not invent op_CheckedAddition for it.
        let s = scan("class C { public static C operator checked +(C a) => a; }");
        assert_eq!(s.members.first().map(|m| m.name.as_str()), Some("operator"));
    }

    #[test]
    fn arithmetic_and_conversion_operators_reconcile() {
        let mut c = ty("C", "class", &["op_Addition", "op_Implicit", "op_Implicit"], &[]);
        for m in &mut c.methods {
            m.params = match m.name.as_str() {
                "op_Addition" => vec![Param { name: "a".into(), ty: "C".into() }, Param { name: "b".into(), ty: "C".into() }],
                _ => vec![Param { name: "c".into(), ty: "C".into() }],
            };
        }
        c.methods[1].return_type = "System.Int32".into();
        c.methods[2].return_type = "System.String".into();
        let members = Members { types: vec![c], total_types: 1, total_methods: 3, total_fields: 0, truncated: false, warnings: vec![] };
        let s = scan(
            "class C {
                public static C operator +(C a, C b) => a;
                public static implicit operator string(C c) => \"\";
                public static implicit operator int(C c) => 0;
            }",
        );
        let r = reconcile(&members, &s);
        assert!(r.missing.is_empty(), "{:?}", r.missing);
        assert!(r.extra.is_empty(), "{:?}", r.extra);
        assert_eq!(r.matched.iter().filter(|m| m.kind == "method").count(), 3, "{:?}", r.matched);
        // The int conversion matches the int declaration (line 4), not the first op_Implicit.
        assert_eq!(r.matched[1].line, Some(4), "{:?}", r.matched[1]);
        assert_eq!(r.matched[2].line, Some(3), "{:?}", r.matched[2]);
    }

    #[test]
    fn conversion_targets_in_different_namespaces_are_distinct() {
        let mut c = ty("C", "class", &["op_Implicit", "op_Implicit"], &[]);
        for m in &mut c.methods {
            m.params = vec![Param { name: "c".into(), ty: "C".into() }];
        }
        c.methods[0].return_type = "A.Result".into();
        c.methods[1].return_type = "B.Result".into();
        let members = Members { types: vec![c], total_types: 1, total_methods: 2, total_fields: 0, truncated: false, warnings: vec![] };
        let s = scan(
            "class C {
                public static implicit operator B.Result(C c) => null;
                public static implicit operator A.Result(C c) => null;
            }",
        );
        let r = reconcile(&members, &s);
        assert!(r.missing.is_empty(), "{:?}", r.missing);
        assert!(r.extra.is_empty(), "{:?}", r.extra);
        let lines: Vec<Option<u32>> = r.matched.iter().map(|m| m.line).collect();
        assert_eq!(lines, vec![Some(3), Some(2)], "A.Result must not match B.Result: {:?}", r.matched);
    }

    #[test]
    fn a_nested_conversion_target_matches_metadata_separators() {
        let mut c = ty("C", "class", &["op_Implicit"], &[]);
        c.methods[0].params = vec![Param { name: "c".into(), ty: "C".into() }];
        c.methods[0].return_type = "A.Outer/Inner".into();
        let members = Members { types: vec![c], total_types: 1, total_methods: 1, total_fields: 0, truncated: false, warnings: vec![] };
        let s = scan("class C { public static implicit operator A.Outer.Inner(C c) => null; }");
        let r = reconcile(&members, &s);
        assert!(r.missing.is_empty(), "{:?}", r.missing);
        assert!(r.extra.is_empty(), "{:?}", r.extra);
        assert_eq!(r.matched.first().and_then(|m| m.line), Some(1), "{:?}", r.matched);
    }

    #[test]
    fn a_short_nested_conversion_target_matches_metadata_separators() {
        let mut c = ty("C", "class", &["op_Implicit"], &[]);
        c.methods[0].params = vec![Param { name: "c".into(), ty: "C".into() }];
        c.methods[0].return_type = "N.Outer/Inner".into();
        let members = Members { types: vec![c], total_types: 1, total_methods: 1, total_fields: 0, truncated: false, warnings: vec![] };
        let s = scan("class C { public static implicit operator Inner(C c) => null; }");
        let r = reconcile(&members, &s);
        assert!(r.missing.is_empty(), "{:?}", r.missing);
        assert!(r.extra.is_empty(), "{:?}", r.extra);
        assert_eq!(r.matched.first().and_then(|m| m.line), Some(1), "{:?}", r.matched);
    }

    #[test]
    fn qualified_generic_conversion_targets_alias_builtin_arguments() {
        let mut c = ty("C", "class", &["op_Implicit"], &[]);
        c.methods[0].params = vec![Param { name: "c".into(), ty: "C".into() }];
        c.methods[0].return_type = "System.Collections.Generic.List<System.Int32>".into();
        let members = Members { types: vec![c], total_types: 1, total_methods: 1, total_fields: 0, truncated: false, warnings: vec![] };
        let s = scan("class C { public static implicit operator System.Collections.Generic.List<int>(C c) => null; }");
        let r = reconcile(&members, &s);
        assert!(r.missing.is_empty(), "{:?}", r.missing);
        assert!(r.extra.is_empty(), "{:?}", r.extra);
        assert_eq!(r.matched.first().and_then(|m| m.line), Some(1), "{:?}", r.matched);
    }

    #[test]
    fn a_short_conversion_target_matches_the_qualified_metadata_name() {
        let mut c = ty("C", "class", &["op_Implicit"], &[]);
        c.methods[0].params = vec![Param { name: "c".into(), ty: "C".into() }];
        c.methods[0].return_type = "A.Result".into();
        let members = Members { types: vec![c], total_types: 1, total_methods: 1, total_fields: 0, truncated: false, warnings: vec![] };
        let s = scan("class C { public static implicit operator Result(C c) => null; }");
        let r = reconcile(&members, &s);
        assert!(r.missing.is_empty(), "{:?}", r.missing);
        assert!(r.extra.is_empty(), "{:?}", r.extra);
        assert_eq!(r.matched.first().and_then(|m| m.line), Some(1), "{:?}", r.matched);
    }

    #[test]
    fn a_namespace_relative_conversion_target_matches() {
        let mut c = ty("N.C", "class", &["op_Implicit"], &[]);
        c.methods[0].params = vec![Param { name: "c".into(), ty: "N.C".into() }];
        c.methods[0].return_type = "N.Outer/Inner".into();
        let members = Members { types: vec![c], total_types: 1, total_methods: 1, total_fields: 0, truncated: false, warnings: vec![] };
        let s = scan("namespace N { class C { public static implicit operator Outer.Inner(C c) => null; } }");
        let r = reconcile(&members, &s);
        assert!(r.missing.is_empty(), "{:?}", r.missing);
        assert!(r.extra.is_empty(), "{:?}", r.extra);
        assert_eq!(r.matched.first().and_then(|m| m.line), Some(1), "{:?}", r.matched);
    }

    #[test]
    fn an_enclosing_type_relative_conversion_target_matches() {
        let mut c = ty("N.C", "class", &["op_Implicit"], &[]);
        c.methods[0].params = vec![Param { name: "c".into(), ty: "N.C".into() }];
        c.methods[0].return_type = "N.C/Outer/Inner".into();
        let members = Members { types: vec![c], total_types: 1, total_methods: 1, total_fields: 0, truncated: false, warnings: vec![] };
        let s = scan("namespace N { class C { public static implicit operator Outer.Inner(C c) => null; } }");
        let r = reconcile(&members, &s);
        assert!(r.missing.is_empty(), "{:?}", r.missing);
        assert!(r.extra.is_empty(), "{:?}", r.extra);
        assert_eq!(r.matched.first().and_then(|m| m.line), Some(1), "{:?}", r.matched);
    }

    #[test]
    fn an_unrelated_qualified_conversion_target_does_not_match() {
        let mut c = ty("N.C", "class", &["op_Implicit"], &[]);
        c.methods[0].params = vec![Param { name: "c".into(), ty: "N.C".into() }];
        c.methods[0].return_type = "N.Other/Inner".into();
        let members = Members { types: vec![c], total_types: 1, total_methods: 1, total_fields: 0, truncated: false, warnings: vec![] };
        let s = scan("namespace N { class C { public static implicit operator Outer.Inner(C c) => null; } }");
        let r = reconcile(&members, &s);
        assert_eq!(r.missing.len(), 1, "{:?}", r.missing);
        assert_eq!(r.extra.len(), 1, "{:?}", r.extra);
        assert!(r.matched.is_empty(), "{:?}", r.matched);
    }

    /// Convenience wrapper around `conversion_target_tier` for unit tests over synthetic target slices.
    fn conversion_targets_match(want: &str, got: &str, declaring: &str, targets: &[(&str, &str, usize)], op_name: &str, arity: usize) -> bool {
        let mut known_types: KnownTypes = BTreeSet::new();
        if !declaring.is_empty() {
            known_types.insert(norm_type_own_arity(declaring));
        }
        for (_, target, _) in targets {
            collect_signature_types(target, &mut known_types);
        }
        let ctx = ConversionCtx { declaring, known_types: &known_types, targets, op_name, arity };
        conversion_target_tier(want, got, &ctx).is_some()
    }

    #[test]
    fn a_short_name_does_not_equate_distinct_nested_targets() {
        let targets = [("op_Implicit", "N.Other/Inner", 1), ("op_Implicit", "N.C/Inner", 1)];
        assert!(conversion_targets_match("N.C/Inner", "Inner", "N.C", &targets, "op_Implicit", 1));
        assert!(!conversion_targets_match("N.Other/Inner", "Inner", "N.C", &targets, "op_Implicit", 1));
    }

    #[test]
    fn conversion_ambiguity_is_per_operator_kind_and_outer_name() {
        // An op_Explicit target with the same terminal name cannot make an op_Implicit ambiguous.
        let kinds = [("op_Implicit", "N.Foo", 1), ("op_Explicit", "M.Foo", 1)];
        assert!(conversion_targets_match("N.Foo", "Foo", "C", &kinds, "op_Implicit", 1));
        // Generic arguments share a terminal name; the outer names still decide.
        let generic = [("op_Implicit", "N.Box<Q.Item>", 1), ("op_Implicit", "M.Other<Q.Item>", 1)];
        assert!(conversion_targets_match("N.Box<Q.Item>", "Box<Q.Item>", "C", &generic, "op_Implicit", 1));
    }

    #[test]
    fn a_global_qualified_conversion_target_is_absolute() {
        let targets = [("op_Implicit", "A.Result", 1), ("op_Implicit", "Result", 1)];
        assert!(conversion_targets_match("A.Result", "global::A.Result", "N.C", &targets, "op_Implicit", 1));
        assert!(!conversion_targets_match("N.A.Result", "global::A.Result", "N.C", &targets, "op_Implicit", 1));
        assert!(conversion_targets_match("Result", "global::Result", "N.C", &targets, "op_Implicit", 1));
        assert!(!conversion_targets_match("A.Result", "global::Result", "N.C", &targets, "op_Implicit", 1));
    }

    #[test]
    fn junk_does_not_panic() {
        for src in ["", "{{{{", "}}}}", "class", "class X {", "\"unterminated", "'", "/*", "@\"", "$\"\"\"", "namespace ;", "class X { void (", "enum E { , }"]
        {
            let _ = scan(src);
        }
        let deep = "{".repeat(10_000);
        let _ = scan(&deep);
        let mut weird = String::new();
        for i in 0..2000u32 {
            weird.push(char::from_u32(i * 7 % 0x2fff).unwrap_or('x'));
        }
        let _ = scan(&weird);
    }

    #[test]
    fn positional_record_declares_properties() {
        use crate::dotnet::model::PropertyInfo;
        use crate::dotnet::{MethodInfo, Param, TypeInfo};
        let method = |name: &str, params: &[&str], ty: &str| MethodInfo {
            token: String::new(),
            name: name.into(),
            key: String::new(),
            signature: name.into(),
            return_type: ty.into(),
            params: params.iter().map(|p| Param { name: String::new(), ty: (*p).into() }).collect(),
            generic_params: vec![],
            visibility: "public",
            modifiers: vec![],
            code_type: "il",
            internal_call: false,
            pinvoke: false,
            rva: 0,
            body: None,
        };
        let members = Members {
            types: vec![TypeInfo {
                token: "0x02000002".into(),
                full_name: "N.Point".into(),
                namespace: "N".into(),
                name: "Point".into(),
                kind: "class",
                visibility: "public",
                modifiers: vec![],
                extends: Some("object".into()),
                enclosing_type: None,
                generic_params: vec![],
                interfaces: vec![],
                fields: vec![],
                methods: vec![
                    method(".ctor", &["int", "int"], "void"),
                    method("get_X", &[], "int"),
                    method("set_X", &["int"], "void"),
                    method("get_Y", &[], "int"),
                    method("set_Y", &["int"], "void"),
                ],
                properties: vec![PropertyInfo { name: "X".into(), ty: "int".into() }, PropertyInfo { name: "Y".into(), ty: "int".into() }],
                events: vec![],
            }],
            total_types: 1,
            total_methods: 5,
            total_fields: 0,
            truncated: false,
            warnings: vec![],
        };
        for src in [
            "namespace N { public record Point(int X, int Y); }",
            "namespace N { public record Point(int X, int Y) { } }",
            "namespace N { public record Point(int X, int Y) { public int X { get; init; } = X; } }",
            "namespace N { public record Point(int X, [property: Obsolete] int Y) { public int X { get; init; } = X; public int Y { get; init; } = Y; } }",
        ] {
            let s = scan(src);
            let props: Vec<(&str, &str, &str)> =
                s.members.iter().filter(|m| m.kind == "property").map(|m| (m.type_name.as_str(), m.name.as_str(), m.ty.as_str())).collect();
            assert_eq!(props.len(), 2, "{src}: {props:?}");
            assert!(props.contains(&("N.Point", "X", "int")), "{src}: {props:?}");
            assert!(props.contains(&("N.Point", "Y", "int")), "{src}: {props:?}");
            let r = reconcile(&members, &s);
            assert!(r.missing.is_empty(), "{src}: {:?}", r.missing);
            assert!(r.extra.is_empty(), "{src}: {:?}", r.extra);
            assert_eq!(r.matched.len(), 3, "{src}");
        }
        // A class with a primary constructor does not get properties.
        let s = scan("namespace N { public class C(int x) { } }");
        assert!(s.members.iter().all(|m| m.kind != "property"));
    }
}
