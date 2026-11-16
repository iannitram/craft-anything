//! Historical network captures (HAR): requests, responses, timings, hosts and an API surface
//! grouped by path template, with privacy built in.
//!
//! What is never kept: cookie values, `Authorization`/`Proxy-Authorization`/API-key style header
//! values, URL user-info, credential-looking query values, and any body value — JSON bodies are
//! reduced to their outline (keys and types). Secrets the caller names are redacted wherever
//! they appear in kept text, in raw, percent-encoded and JSON-escaped spellings; a sensitive value
//! used as a JSON property name drops that property.

use craft_anything_formats::har::{self, HarEntry};
use craft_anything_formats::sourcemap::base64_decode;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const CREDENTIAL_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "api-key",
    "x-auth-token",
    "x-access-token",
    "x-csrf-token",
    "x-xsrf-token",
    "x-amz-security-token",
    "x-goog-api-key",
    "x-session-id",
];

const CREDENTIAL_PARAMS: &[&str] = &[
    "token",
    "access_token",
    "id_token",
    "refresh_token",
    "auth",
    "authorization",
    "key",
    "api_key",
    "apikey",
    "secret",
    "client_secret",
    "password",
    "passwd",
    "pwd",
    "session",
    "sessionid",
    "sid",
    "sig",
    "signature",
    "code",
    "x-amz-signature",
    "x-amz-credential",
    "x-amz-security-token",
];

#[derive(Debug, Clone, Default)]
pub struct HarOptions {
    pub hidden_values: Vec<String>,
    pub offset: usize,
    pub limit: usize,
    /// Only these entries (0-based), in full detail.
    pub record_numbers: Option<Vec<usize>>,
    pub max_entries: usize,
}

pub(crate) struct Redactor {
    /// Every spelling of every declared value, longest first.
    pub(crate) needles: Vec<String>,
    log: Vec<Value>,
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl Redactor {
    pub(crate) fn new(values: &[String]) -> Self {
        let mut needles: Vec<String> = Vec::new();
        for v in values.iter().filter(|v| !v.is_empty()) {
            let json_escaped = serde_json::to_string(v).ok().and_then(|q| q.get(1..q.len().saturating_sub(1)).map(String::from)).unwrap_or_default();
            for form in [v.clone(), v.to_ascii_lowercase(), percent_encode(v), percent_encode(v).to_ascii_lowercase(), json_escaped, v.replace(' ', "+")] {
                if !form.is_empty() && !needles.contains(&form) {
                    needles.push(form);
                }
            }
        }
        needles.sort_by_key(|n| std::cmp::Reverse(n.len()));
        Redactor { needles, log: Vec::new() }
    }

    fn contains(&self, s: &str) -> bool {
        self.needles.iter().any(|n| s.contains(n.as_str()))
    }

    /// Replace declared values in kept text.
    fn text(&mut self, pointer: &str, s: &str) -> String {
        if !self.contains(s) {
            return s.to_string();
        }
        let mut out = s.to_string();
        for n in &self.needles {
            out = out.replace(n.as_str(), "[redacted]");
        }
        self.log.push(json!({"pointer": pointer, "reason": "secret-value"}));
        out
    }
}

/// Split a URL, dropping user-info. Returns (scheme, host, path, query pairs, had_userinfo).
pub(crate) fn split_url(url: &str) -> (String, String, String, Vec<(String, String)>, bool) {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let rest = rest.split('#').next().unwrap_or(rest);
    let (authority, path_q) = match rest.find(['/', '?', '#']) {
        Some(i) => (rest.get(..i).unwrap_or(""), rest.get(i..).unwrap_or("/")),
        None => (rest, "/"),
    };
    let (userinfo, host) = match authority.rsplit_once('@') {
        Some((_, h)) => (true, h),
        None => (false, authority),
    };
    let (path, query) = path_q.split_once('?').unwrap_or((path_q, ""));
    let pairs = query
        .split('&')
        .filter(|p| !p.is_empty())
        .take(200)
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (k.to_string(), v.to_string())
        })
        .collect();
    (scheme.to_ascii_lowercase(), host.to_ascii_lowercase(), if path.is_empty() { "/".into() } else { path.to_string() }, pairs, userinfo)
}

/// Decode a query name once for privacy checks; malformed escapes stay literal.
fn decode_query_name(name: &str) -> String {
    let mut decoded = Vec::with_capacity(name.len());
    let mut bytes = name.as_bytes().iter().copied().peekable();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let mut next = bytes.clone();
            let hex = next.next().and_then(|hi| next.next().map(|lo| [hi, lo]));
            if let Some(value) =
                hex.filter(|h| h.iter().all(u8::is_ascii_hexdigit)).and_then(|h| std::str::from_utf8(&h).ok().and_then(|s| u8::from_str_radix(s, 16).ok()))
            {
                decoded.push(value);
                bytes = next;
                continue;
            }
        }
        decoded.push(if b == b'+' { b' ' } else { b });
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

pub(crate) fn is_credential_param(k: &str) -> bool {
    let l = decode_query_name(k).to_ascii_lowercase();
    CREDENTIAL_PARAMS.contains(&l.as_str()) || l.ends_with("_token") || l.ends_with("token") || l.contains("password") || l.contains("secret")
}

/// Rebuild a safe URL: no user-info, credential query values replaced.
fn safe_url(url: &str, pointer: &str, red: &mut Redactor) -> (String, Vec<String>) {
    let (scheme, host, path, pairs, userinfo) = split_url(url);
    if userinfo {
        red.log.push(json!({"pointer": pointer, "reason": "url-userinfo"}));
    }
    let mut q = Vec::new();
    let mut names = Vec::new();
    for (k, v) in pairs {
        if red.contains(&k) || red.contains(&decode_query_name(&k)) {
            red.log.push(json!({"pointer": pointer, "reason": "secret-value", "scope": "query-name"}));
            continue;
        }
        names.push(k.clone());
        if is_credential_param(&k) && !v.is_empty() {
            red.log.push(json!({"pointer": pointer, "reason": "credential-parameter", "parameter": k}));
            q.push(format!("{k}=[redacted]"));
        } else {
            q.push(if v.is_empty() { k } else { format!("{k}={v}") });
        }
    }
    let base = if scheme.is_empty() { format!("{host}{path}") } else { format!("{scheme}://{host}{path}") };
    let full = if q.is_empty() { base } else { format!("{base}?{}", q.join("&")) };
    (red.text(pointer, &full), names)
}

/// Path template: numeric → {n}, uuid → {uuid}, long hex → {hex}, ISO date → {date}.
pub fn path_template(path: &str) -> String {
    path.split('/')
        .map(|seg| {
            let s = seg;
            if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
                "{n}".to_string()
            } else if is_uuid(s) {
                "{uuid}".to_string()
            } else if s.len() >= 32 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
                "{hex}".to_string()
            } else if is_iso_date(s) {
                "{date}".to_string()
            } else {
                s.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5 && parts.iter().zip([8, 4, 4, 4, 12]).all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return false;
    }
    let number = |start, end| s.get(start..end).filter(|v| v.bytes().all(|b| b.is_ascii_digit())).and_then(|v| v.parse::<u32>().ok());
    let (Some(year), Some(month), Some(day)) = (number(0, 4), number(5, 7), number(8, 10)) else { return false };
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => 28 + u32::from(year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))),
        _ => return false,
    };
    (1..=days).contains(&day)
}

/// The outline of a JSON value: types only, never values.
pub fn json_outline(v: &Value, depth: usize, red: &mut OutlineRedactions<'_>, pointer: &str) -> Value {
    if depth > 8 {
        return json!("…");
    }
    match v {
        Value::Null => json!("null"),
        Value::Bool(_) => json!("boolean"),
        Value::Number(n) => json!(if n.is_i64() || n.is_u64() { "integer" } else { "number" }),
        Value::String(_) => json!("string"),
        Value::Array(a) => {
            let mut items: Option<Value> = None;
            for (i, x) in a.iter().take(20).enumerate() {
                let s = json_outline(x, depth + 1, red, &format!("{pointer}/{i}"));
                items = Some(match items {
                    None => s,
                    Some(prev) => merge_outlines(prev, s),
                });
            }
            json!({"type": "array", "items": items.unwrap_or(json!("unknown")), "length": a.len()})
        }
        Value::Object(o) => {
            let mut props = Map::new();
            for (k, x) in o.iter().take(200) {
                if red.names_hit(k) {
                    red.log.push(json!({"pointer": pointer, "reason": "secret-value", "scope": "property-name"}));
                    continue;
                }
                let child = format!("{pointer}/{}", k.replace('~', "~0").replace('/', "~1"));
                props.insert(k.clone(), json_outline(x, depth + 1, red, &child));
            }
            json!({"type": "object", "properties": props})
        }
    }
}

/// Redaction context for outlines (only property names can leak).
pub struct OutlineRedactions<'r> {
    needles: &'r [String],
    pub log: Vec<Value>,
}

impl OutlineRedactions<'_> {
    fn names_hit(&self, k: &str) -> bool {
        self.needles.iter().any(|n| k.contains(n.as_str()))
    }
}

fn merge_outlines(a: Value, b: Value) -> Value {
    if a == b {
        return a;
    }
    match (a, b) {
        (Value::Object(mut x), Value::Object(y)) if x.get("type") == Some(&json!("object")) && y.get("type") == Some(&json!("object")) => {
            let mut props = x.remove("properties").and_then(|p| if let Value::Object(m) = p { Some(m) } else { None }).unwrap_or_default();
            if let Some(Value::Object(yp)) = y.get("properties") {
                for (k, v) in yp {
                    let merged = match props.remove(k) {
                        Some(old) => merge_outlines(old, v.clone()),
                        None => v.clone(),
                    };
                    props.insert(k.clone(), merged);
                }
            }
            json!({"type": "object", "properties": props})
        }
        (Value::Object(x), other) | (other, Value::Object(x)) if x.contains_key("one_of") => {
            let mut list = x.get("one_of").and_then(Value::as_array).cloned().unwrap_or_default();
            if !list.contains(&other) && list.len() < 6 {
                list.push(other);
            }
            json!({"one_of": list})
        }
        (a, b) => json!({"one_of": [a, b]}),
    }
}

fn body_outline(
    mime: Option<&str>,
    text: Option<&str>,
    encoding: Option<&str>,
    params: &[(String, Option<String>)],
    red: &mut Redactor,
    pointer: &str,
) -> Value {
    let mime_l = mime.unwrap_or("").to_ascii_lowercase();
    let decoded_bytes = match (text, encoding) {
        (Some(t), Some("base64")) => base64_decode(t),
        (Some(t), _) => Some(t.as_bytes().to_vec()),
        _ => None,
    };
    let bytes = decoded_bytes.as_ref().map_or(0, Vec::len);
    let decoded = decoded_bytes.as_deref().map(|b| String::from_utf8_lossy(b).into_owned());
    let json_like = mime_l.contains("json") || decoded.as_deref().is_some_and(|t| t.trim_start().starts_with('{') || t.trim_start().starts_with('['));
    if json_like && let Some(v) = decoded.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
        let mut sr = OutlineRedactions { needles: &red.needles, log: Vec::new() };
        let outline = json_outline(&v, 0, &mut sr, pointer);
        red.log.extend(sr.log);
        return json!({"kind": "json", "bytes": bytes, "outline": outline});
    }
    if mime_l.contains("x-www-form-urlencoded") {
        let names: Vec<String> = if params.is_empty() {
            decoded.as_deref().unwrap_or("").split('&').filter_map(|p| p.split('=').next()).filter(|k| !k.is_empty()).map(String::from).take(200).collect()
        } else {
            params.iter().map(|(k, _)| k.clone()).take(200).collect()
        };
        let names: Vec<String> = names.into_iter().filter(|n| !red.contains(n)).collect();
        return json!({"kind": "form", "bytes": bytes, "fields": names});
    }
    if mime_l.contains("multipart") {
        return json!({"kind": "multipart", "bytes": bytes, "fields": params.iter().map(|(k, _)| k.clone()).filter(|n| !red.contains(n)).take(200).collect::<Vec<_>>()});
    }
    if decoded.is_none() {
        return Value::Null;
    }
    json!({"kind": if mime_l.is_empty() { "unknown".to_string() } else { red.text(&format!("{pointer}/mime"), &mime_l) }, "bytes": bytes})
}

fn headers_view(h: &[(String, String)], pointer: &str, red: &mut Redactor) -> Vec<Value> {
    h.iter()
        .take(200)
        .enumerate()
        .filter_map(|(i, (n, v))| {
            if red.contains(n) {
                red.log.push(json!({"pointer": pointer, "reason": "secret-value", "scope": "header-name"}));
                return None;
            }
            let l = n.to_ascii_lowercase();
            let p = format!("{pointer}/{i}/value");
            if CREDENTIAL_HEADERS.contains(&l.as_str()) {
                red.log.push(json!({"pointer": p, "reason": "auth-header"}));
                Some(json!({"name": n, "value": null, "redacted": "auth-header"}))
            } else if matches!(l.as_str(), "referer" | "origin" | "location") {
                let (u, _) = safe_url(v, &p, red);
                Some(json!({"name": n, "value": u}))
            } else {
                Some(json!({"name": n, "value": red.text(&p, v)}))
            }
        })
        .collect()
}

fn entry_view(e: &HarEntry, red: &mut Redactor) -> Value {
    let base = format!("/log/entries/{}", e.ordinal);
    let (url, query_names) = safe_url(&e.request.url, &format!("{base}/request/url"), red);
    let (_, host, path, _, _) = split_url(&e.request.url);
    let req_body =
        e.request.post.as_ref().map(|p| body_outline(p.mime.as_deref(), p.text.as_deref(), None, &p.params, red, &format!("{base}/request/postData")));
    let res_body =
        body_outline(e.response.mime.as_deref(), e.response.text.as_deref(), e.response.encoding.as_deref(), &[], red, &format!("{base}/response/content"));
    let redirect = e.response.redirect_url.as_deref().map(|u| safe_url(u, &format!("{base}/response/redirectURL"), red).0);
    if !e.request.cookie_names.is_empty() || !e.response.cookie_names.is_empty() {
        red.log.push(json!({"pointer": format!("{base}/cookies"), "reason": "auth-header"}));
    }
    json!({
        "ordinal": e.ordinal,
        "started": e.started.as_deref().map(|value| red.text(&format!("{base}/started"), value)),
        "time_ms": e.time_ms,
        "resource_type": e.resource_type.as_deref().map(|value| red.text(&format!("{base}/resource_type"), value)),
        "method": red.text(&format!("{base}/request/method"), &e.request.method),
        "url": url,
        "host": red.text(&format!("{base}/request/host"), &host),
        "path_template": red.text(&format!("{base}/request/path_template"), &path_template(&path)),
        "query_parameters": query_names,
        "http_version": e.request.http_version.as_deref().map(|value| red.text(&format!("{base}/http_version"), value)),
        "status": e.response.status,
        "status_text": e.response.status_text.as_deref().map(|value| red.text(&format!("{base}/status_text"), value)),
        "request_headers": headers_view(&e.request.headers, &format!("{base}/request/headers"), red),
        "response_headers": headers_view(&e.response.headers, &format!("{base}/response/headers"), red),
        "request_cookie_names": e.request.cookie_names.iter().filter(|n| !red.contains(n)).collect::<Vec<_>>(),
        "response_cookie_names": e.response.cookie_names.iter().filter(|n| !red.contains(n)).collect::<Vec<_>>(),
        "request_body": req_body,
        "response_body": res_body,
        "response_mime": e.response.mime.as_deref().map(|value| red.text(&format!("{base}/response_mime"), value)),
        "response_bytes": e.response.content_size,
        "redirect_url": redirect,
        "timings": e.timings.iter().filter(|(k, _)| !red.contains(k)).map(|(k, v)| (k.clone(), json!(v))).collect::<Map<String, Value>>(),
    })
}

/// HAR uses -1 for an unknown size. A known zero (for example on a 304) still counts.
fn response_sizes(entries: &[HarEntry], field: impl Fn(&har::HarResponse) -> Option<i64>) -> (u64, usize) {
    let mut bytes = 0u64;
    let mut missing = 0;
    for entry in entries {
        match field(&entry.response).and_then(|n| u64::try_from(n).ok()) {
            Some(n) => bytes = bytes.saturating_add(n),
            None => missing += 1,
        }
    }
    (bytes, missing)
}

/// Inspect a HAR capture.
pub fn inspect_har(bytes: &[u8], opts: &HarOptions) -> Result<Value, String> {
    let har = har::parse(bytes, opts.max_entries.max(1)).map_err(|e| e.to_string())?;
    let mut red = Redactor::new(&opts.hidden_values);
    let mut methods: BTreeMap<String, usize> = BTreeMap::new();
    let mut statuses: BTreeMap<String, usize> = BTreeMap::new();
    let mut hosts: BTreeMap<String, usize> = BTreeMap::new();
    let mut mimes: BTreeMap<String, usize> = BTreeMap::new();
    let (body_bytes, missing_body) = response_sizes(&har.entries, |r| r.body_size);
    let (content_bytes, missing_content) = response_sizes(&har.entries, |r| r.content_size);
    let mut first: Option<String> = None;
    let mut last: Option<String> = None;
    let mut slowest: Vec<(f64, usize)> = Vec::new();
    let mut surface: BTreeMap<String, Value> = BTreeMap::new();
    let mut views = Vec::new();
    for e in &har.entries {
        let view = entry_view(e, &mut red);
        *methods.entry(view["method"].as_str().unwrap_or_default().to_ascii_uppercase()).or_default() += 1;
        *statuses.entry(format!("{}xx", e.response.status / 100)).or_default() += 1;
        let host = view["host"].as_str().unwrap_or("").to_string();
        *hosts.entry(red.text("/hosts", &host)).or_default() += 1;
        let mime = view["response_mime"].as_str().unwrap_or("").split(';').next().unwrap_or("").trim().to_string();
        *mimes.entry(mime.clone()).or_default() += 1;
        if let Some(s) = &e.started {
            if first.as_ref().is_none_or(|f| s < f) {
                first = Some(s.clone());
            }
            if last.as_ref().is_none_or(|l| s > l) {
                last = Some(s.clone());
            }
        }
        if let Some(t) = e.time_ms {
            slowest.push((t, e.ordinal));
        }
        // API surface.
        let (scheme, _, _, _, _) = split_url(&e.request.url);
        let key = format!(
            "{} {}://{}{}",
            view["method"].as_str().unwrap_or_default().to_ascii_uppercase(),
            red.text("/scheme", &scheme),
            host,
            view["path_template"].as_str().unwrap_or("")
        );
        let key = red.text("/api_surface", &key);
        let g = surface.entry(key.clone()).or_insert_with(|| json!({"operation": key, "count": 0, "statuses": [], "query_parameters": [], "request_outline": null, "response_outline": null, "response_mimes": []}));
        if let Some(o) = g.as_object_mut() {
            let c = o.get("count").and_then(Value::as_u64).unwrap_or(0) + 1;
            o.insert("count".into(), json!(c));
            add_unique(o, "statuses", json!(e.response.status));
            for q in view["query_parameters"].as_array().into_iter().flatten() {
                add_unique(o, "query_parameters", q.clone());
            }
            add_unique(o, "response_mimes", json!(mime));
            for (field, src) in [("request_outline", &view["request_body"]), ("response_outline", &view["response_body"])] {
                if let Some(outline) = src.get("outline") {
                    let merged = match o.get(field).cloned() {
                        Some(Value::Null) | None => outline.clone(),
                        Some(prev) => merge_outlines(prev, outline.clone()),
                    };
                    o.insert(field.into(), merged);
                }
            }
        }
        views.push(view);
    }
    slowest.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let slowest: Vec<Value> = slowest
        .iter()
        .take(5)
        .filter_map(|(t, i)| views.get(*i).map(|v| json!({"ordinal": i, "time_ms": t, "method": v["method"], "url": v["url"]})))
        .collect();
    let selected: Vec<Value> = match &opts.record_numbers {
        Some(ords) => views.iter().filter(|v| v["ordinal"].as_u64().is_some_and(|o| ords.contains(&(o as usize)))).cloned().collect(),
        None => views.iter().skip(opts.offset).take(opts.limit.max(1)).cloned().collect(),
    };
    let ops: Vec<Value> = surface.into_values().collect();
    let version = har.version.as_deref().map(|v| red.text("/har_version", v));
    let creator = har.creator.as_deref().map(|v| red.text("/creator", v));
    let first = first.as_deref().map(|v| red.text("/summary/first_request", v));
    let last = last.as_deref().map(|v| red.text("/summary/last_request", v));
    let mut redactions = red.log;
    redactions.truncate(5000);
    Ok(json!({
        "format": "har",
        "har_version": version,
        "creator": creator,
        "pages": har.pages,
        "summary": {
            "entries": har.entries.len(),
            "declared_entries": har.declared_entries,
            "methods": methods, "statuses": statuses, "hosts": hosts, "response_mimes": mimes,
            "response_body_bytes": body_bytes, "responses_without_body_size": missing_body,
            "response_content_bytes": content_bytes, "responses_without_content_size": missing_content,
            "first_request": first, "last_request": last, "slowest": slowest,
        },
        "api_surface": ops,
        "entries": selected,
        "entries_total": views.len(),
        "redactions": redactions,
        "sensitive_value_declarations": opts.hidden_values.len(),
        "truncated": har.truncated,
    }))
}

fn add_unique(o: &mut Map<String, Value>, key: &str, v: Value) {
    if let Some(Value::Array(a)) = o.get_mut(key)
        && !a.contains(&v)
        && a.len() < 64
    {
        a.push(v);
    }
}

/// Every set of distinct values seen for a key in a JSON tree — used by tests to prove nothing leaks.
pub fn all_strings(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::String(s) => {
            out.insert(s.clone());
        }
        Value::Array(a) => a.iter().for_each(|x| all_strings(x, out)),
        Value::Object(o) => o.iter().for_each(|(k, x)| {
            out.insert(k.clone());
            all_strings(x, out);
        }),
        _ => {}
    }
}

/// A HAR 1.2 recording of browser network entries, as the browser tools return them (method, url,
/// status, MIME type and any response body they kept). Nothing is invented: a header reduced to its
/// length, a body that was left out and a body that came back as bytes are all left out here too.
/// The result is what `web_standin_build` reads, so a rebuilt front end can be given recorded
/// answers.
pub fn har_from_network(requests: &[Value], started: &str) -> Value {
    let entries: Vec<Value> = requests.iter().map(|r| har_entry(r, started)).collect();
    json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "Craft Anything", "version": env!("CARGO_PKG_VERSION")},
            "entries": entries,
        }
    })
}

/// W3C-style headers from a name-to-value map the tools kept. The tools return
/// `{"kept": {...}, "credentials_present": [...]}` for an outline, or a flat map of every value;
/// both are read, and a value that isn't text (a length, a list) is dropped rather than guessed.
fn har_headers(value: Option<&Value>) -> Vec<Value> {
    let Some(Value::Object(o)) = value else { return Vec::new() };
    let kept = match o.get("kept") {
        Some(Value::Object(kept)) => kept,
        _ => o,
    };
    kept.iter().filter_map(|(name, v)| v.as_str().map(|v| json!({"name": name, "value": v}))).collect()
}

/// The text of a body the tools kept, if it is whole: a body cut at the cap would be replayed as
/// an incomplete answer (invalid JSON, a file missing its end), so only a complete one is used.
fn har_body<'a>(r: &'a Value, key: &str) -> Option<&'a str> {
    let body = r.get(key)?;
    if body.get("state").and_then(Value::as_str) != Some("captured") || body.get("truncated").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    body.get("text").and_then(Value::as_str)
}

fn har_entry(r: &Value, started: &str) -> Value {
    let text = |key: &str| r.get(key).and_then(Value::as_str).unwrap_or_default();
    let mut request = json!({
        "method": if text("method").is_empty() { "GET" } else { text("method") },
        "url": text("url"),
        "httpVersion": "HTTP/1.1",
        "headers": har_headers(r.get("request_header_values").or_else(|| r.get("request_headers"))),
        "queryString": [],
        "cookies": [],
        "headersSize": -1,
        "bodySize": -1,
    });
    if let Some(body) = har_body(r, "request_body") {
        request["postData"] = json!({"mimeType": text("mime_type"), "text": body});
    }
    let mut content = json!({"size": 0, "mimeType": text("mime_type")});
    if let Some(body) = har_body(r, "response_body") {
        content["text"] = json!(body);
        content["size"] = json!(r.get("response_body").and_then(|b| b.get("bytes")).and_then(Value::as_u64).unwrap_or(body.len() as u64));
    }
    json!({
        "startedDateTime": started,
        "time": 0,
        "request": request,
        "response": {
            "status": r.get("status").and_then(Value::as_i64).unwrap_or(0),
            "statusText": "",
            "httpVersion": "HTTP/1.1",
            "headers": har_headers(r.get("response_header_values").or_else(|| r.get("response_headers"))),
            "cookies": [],
            "content": content,
            "redirectURL": "",
            "headersSize": -1,
            "bodySize": -1,
        },
        "cache": {},
        "timings": {"send": 0, "wait": 0, "receive": 0},
    })
}

/// Now, as an ISO 8601 UTC stamp (`2026-11-14T09:30:00Z`), for a recording's `startedDateTime`.
pub fn utc_iso_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    utc_iso(secs)
}

/// A Unix time as an ISO 8601 UTC stamp, without a calendar crate: days since the epoch to a civil
/// date, by Howard Hinnant's algorithm.
fn utc_iso(secs: u64) -> String {
    let days = secs / 86_400;
    let rest = secs % 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 };
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, (rest % 3600) / 60, rest % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_url_authority_cases() {
        let mut failures = Vec::new();
        for (row, url, host, path, userinfo) in [
            ("at in path", "https://example.test/@profile", "example.test", "/@profile", false),
            ("at in file name", "https://cdn.example.test/icon@2x.png", "cdn.example.test", "/icon@2x.png", false),
            ("at in query", "https://example.test/?q=a@b", "example.test", "/", false),
            ("at in fragment", "https://example.test/#a@b", "example.test", "/", false),
            ("user-info and at in path", "https://user:pw@example.test/@x", "example.test", "/@x", true),
            ("IPv6 and port", "https://[::1]:8080/@x", "[::1]:8080", "/@x", false),
            ("query with slash, no path", "https://example.test?q=a/b@c", "example.test", "/", false),
            ("port and at in path", "http://example.test:8080/a@b", "example.test:8080", "/a@b", false),
            ("fragment, no path", "https://example.test#a@b", "example.test", "/", false),
            ("user-info only", "https://user@example.test/p", "example.test", "/p", true),
            ("plain", "https://example.test/path", "example.test", "/path", false),
            ("percent-encoded at in user-info", "https://u%40x:pw@example.test/", "example.test", "/", true),
            ("two at signs in user-info", "https://a@b@EXAMPLE.test/p", "example.test", "/p", true),
        ] {
            let (_, actual_host, actual_path, _, actual_userinfo) = split_url(url);
            let actual = (actual_host.as_str(), actual_path.as_str(), actual_userinfo);
            let expected = (host, path, userinfo);
            if actual != expected {
                failures.push(format!("{row}: expected {expected:?}, got {actual:?}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    const HAR: &str = r#"{"log":{"version":"1.2","creator":{"name":"t","version":"1"},"entries":[
      {"startedDateTime":"2026-01-01T00:00:00Z","time":120,
       "request":{"method":"GET","url":"https://bob:hunter2@api.example.test/v1/users/42?access_token=SECRET123&page=2","headers":[{"name":"Authorization","value":"Bearer SECRET123"},{"name":"Accept","value":"application/json"},{"name":"X-Trace","value":"trace-hunter2"}],"cookies":[{"name":"sid","value":"COOKIEVAL"}]},
       "response":{"status":200,"headers":[{"name":"Set-Cookie","value":"sid=COOKIEVAL"}],"content":{"size":40,"mimeType":"application/json","text":"{\"id\":42,\"name\":\"Ann\",\"tags\":[\"a\"],\"hunter2\":true}"}},"timings":{"wait":100}},
      {"startedDateTime":"2026-01-01T00:00:01Z","time":30,
       "request":{"method":"GET","url":"https://api.example.test/v1/users/7","headers":[]},
       "response":{"status":404,"headers":[],"content":{"size":2,"mimeType":"application/json","text":"eyJlcnJvciI6Im5vIn0=","encoding":"base64"}},"timings":{}},
      {"startedDateTime":"2026-01-01T00:00:02Z","time":5,
       "request":{"method":"POST","url":"https://api.example.test/v1/login","headers":[],"postData":{"mimeType":"application/x-www-form-urlencoded","text":"user=ann&password=hunter2"}},
       "response":{"status":302,"headers":[{"name":"Location","value":"https://u:p@api.example.test/home"}],"content":{"size":0,"mimeType":""},"redirectURL":"https://api.example.test/home?token=abc"},"timings":{}}
    ]}}"#;

    fn run(sensitive: &[&str]) -> Value {
        inspect_har(
            HAR.as_bytes(),
            &HarOptions { hidden_values: sensitive.iter().map(|s| s.to_string()).collect(), limit: 50, max_entries: 1000, ..Default::default() },
        )
        .unwrap()
    }

    #[test]
    fn nothing_secret_survives() {
        let v = run(&["hunter2"]);
        let mut all = BTreeSet::new();
        all_strings(&v, &mut all);
        for s in &all {
            for secret in ["SECRET123", "COOKIEVAL", "hunter2", "bob:", "u:p@", "Ann"] {
                assert!(!s.contains(secret), "{secret} leaked in {s:?}");
            }
        }
        assert_eq!(v["summary"]["entries"], 3);
        assert_eq!(v["summary"]["methods"]["GET"], 2);
        assert_eq!(v["entries"][0]["url"], "https://api.example.test/v1/users/42?access_token=[redacted]&page=2");
        assert_eq!(v["entries"][0]["request_headers"][0]["value"], Value::Null);
        assert_eq!(v["entries"][0]["request_headers"][2]["value"], "trace-[redacted]");
        assert_eq!(v["entries"][2]["request_body"]["fields"], json!(["user", "password"]));
        let outline = &v["entries"][0]["response_body"]["outline"];
        assert_eq!(outline["properties"]["id"], "integer");
        assert!(outline["properties"].get("hunter2").is_none(), "sensitive property name dropped");
        assert_eq!(v["entries"][1]["response_body"]["outline"]["properties"]["error"], "string");
    }

    #[test]
    fn api_surface_groups_by_template() {
        let v = run(&[]);
        let ops = v["api_surface"].as_array().unwrap();
        let users = ops.iter().find(|o| o["operation"] == "GET https://api.example.test/v1/users/{n}").unwrap();
        assert_eq!(users["count"], 2);
        assert_eq!(users["statuses"], json!([200, 404]));
        assert_eq!(path_template("/a/123e4567-e89b-12d3-a456-426614174000/2026-01-02/deadbeefdeadbeefdeadbeefdeadbeef"), "/a/{uuid}/{date}/{hex}");
    }

    #[test]
    fn encoded_forms_are_redacted() {
        let r = Redactor::new(&["a b/c\"".into()]);
        assert!(r.needles.iter().any(|n| n == "a%20b%2Fc%22"));
        assert!(r.needles.iter().any(|n| n == "a b/c\\\""));
        let mut r = r;
        assert_eq!(r.text("/x", "q=a%20b%2Fc%22"), "q=[redacted]");
        assert!(inspect_har(b"not json", &HarOptions::default()).is_err());
    }

    /// A recording written from what a browser run kept reads back as the same requests, and a
    /// stand-in built from it knows them.
    #[test]
    fn a_recording_of_kept_requests_reads_back() {
        let requests = vec![
            json!({"method": "GET", "url": "https://api.example.test/v1/items/42", "status": 200, "mime_type": "application/json",
                   "response_headers": {"content-type": "application/json"},
                   "response_body": {"state": "captured", "bytes": 11, "text": "{\"title\":1}"},
                   "request_headers": {"accept": "application/json", "authorization": {"value_length": 12}}}),
            json!({"method": "POST", "url": "https://api.example.test/v1/items", "status": 201, "mime_type": "application/json",
                   "request_body": {"state": "captured", "bytes": 9, "text": "{\"a\":true}"},
                   "response_body": {"state": "not_exposed"}}),
        ];
        let har = har_from_network(&requests, "2026-11-14T09:30:00Z");
        let bytes = serde_json::to_vec(&har).unwrap();
        let parsed = har::parse(&bytes, 10).unwrap();
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.entries[0].request.method, "GET");
        assert_eq!(parsed.entries[0].response.text.as_deref(), Some("{\"title\":1}"));
        assert_eq!(parsed.entries[1].request.post.as_ref().and_then(|p| p.text.as_deref()), Some("{\"a\":true}"));
        assert_eq!(parsed.entries[1].response.status, 201);
        assert_eq!(parsed.entries[1].response.text, None, "a body that wasn't kept isn't invented");
        // A header whose value was reduced to its length stays out.
        let headers = har["log"]["entries"][0]["request"]["headers"].as_array().unwrap();
        assert_eq!(headers.len(), 1);
        let standin = crate::standin::build(&parsed, &crate::standin::BuildOptions::default());
        assert_eq!(standin.routes.len(), 2, "{:?}", standin.routes.iter().map(|r| &r.template).collect::<Vec<_>>());
        assert_eq!(standin.hosts, vec!["api.example.test".to_string()]);
    }

    /// #1838: the browser keeps headers under `response_headers.kept`; a recording has to read
    /// that shape or a `Link` a front end paginates with disappears from replayed answers.
    #[test]
    fn a_recording_reads_the_nested_header_shape_the_browser_keeps() {
        let requests = vec![json!({
            "method": "GET", "url": "https://api.example.test/v1/items?page=2", "status": 200,
            "mime_type": "application/json",
            "response_headers": {
                "kept": {"content-type": "application/json", "link": "</v1/items?page=3>; rel=\"next\"", "x-long": {"value_length": 40}},
                "credentials_present": ["set-cookie"],
            },
            "response_body": {"state": "captured", "bytes": 2, "text": "{}"},
        })];
        let har = har_from_network(&requests, "2026-11-14T09:30:00Z");
        let headers = har["log"]["entries"][0]["response"]["headers"].as_array().unwrap();
        let link = headers.iter().find(|h| h["name"] == "link").expect("the kept Link header");
        assert_eq!(link["value"], "</v1/items?page=3>; rel=\"next\"");
        assert!(headers.iter().any(|h| h["name"] == "content-type"));
        assert!(!headers.iter().any(|h| h["name"] == "x-long"), "a length isn't a value");
        let parsed = har::parse(&serde_json::to_vec(&har).unwrap(), 10).unwrap();
        assert!(parsed.entries[0].response.headers.iter().any(|(name, _)| name == "link"), "{:?}", parsed.entries[0].response.headers);
    }

    /// #1838: a body cut at the cap would replay as an incomplete answer, so it isn't one.
    #[test]
    fn a_recording_leaves_out_a_truncated_body() {
        let requests = vec![json!({
            "method": "GET", "url": "https://api.example.test/v1/big", "status": 200, "mime_type": "application/json",
            "request_body": {"state": "captured", "bytes": 70_000, "truncated": true, "text": "{\"q\":"},
            "response_body": {"state": "captured", "bytes": 70_000, "truncated": true, "text": "{\"a\":"},
        })];
        let har = har_from_network(&requests, "2026-11-14T09:30:00Z");
        let entry = &har["log"]["entries"][0];
        assert!(entry["response"]["content"].get("text").is_none(), "{}", entry["response"]);
        assert!(entry["request"].get("postData").is_none(), "{}", entry["request"]);
    }

    /// #1838: two GraphQL operations to one endpoint are kept apart by the body they sent, so the
    /// stand-in answers each with its own.
    #[test]
    fn two_operations_to_one_endpoint_are_told_apart_by_their_bodies() {
        let op = |query: &str, title: &str| {
            let answer = format!("{{\"data\":{{\"{title}\":[]}}}}");
            json!({"method": "POST", "url": "https://api.example.test/graphql", "status": 200, "mime_type": "application/json",
                   "request_body": {"state": "captured", "bytes": query.len(), "text": query},
                   "response_body": {"state": "captured", "bytes": answer.len(), "text": answer}})
        };
        let requests = vec![op("{\"query\":\"query Items { items { id } }\"}", "items"), op("{\"query\":\"query Users { users { id } }\"}", "users")];
        let har = har_from_network(&requests, "2026-11-14T09:30:00Z");
        let parsed = har::parse(&serde_json::to_vec(&har).unwrap(), 10).unwrap();
        let standin = crate::standin::build(&parsed, &crate::standin::BuildOptions::default());
        let route = standin.routes.iter().find(|r| r.template == "/graphql").expect("one route");
        let keys: Vec<Option<String>> = route.samples.iter().map(|s| s.request_key.clone()).collect();
        assert_eq!(keys.len(), 2, "{keys:?}");
        assert!(keys.iter().all(Option::is_some), "both bodies are request keys: {keys:?}");
        assert_ne!(keys[0], keys[1], "the two operations are told apart");
    }

    #[test]
    fn a_unix_time_is_written_as_a_utc_stamp() {
        assert_eq!(utc_iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_iso(86_399), "1970-01-01T23:59:59Z");
        assert_eq!(utc_iso(951_782_400), "2000-02-29T00:00:00Z");
    }
}
