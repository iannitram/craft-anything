//! What a model server says its context window is, read before the conversation needs it (R5.4).
//!
//! Without it, a model on this computer or its network is budgeted at
//! [`LOCAL_CONTEXT_TOKENS`](crate::context::LOCAL_CONTEXT_TOKENS) whatever it can hold, and the
//! real size is only learned when a request overflows, which never happens once we trim first.
//! Each kind of server says it in its own place, so each is a [`WindowProbe`]; [`probes`] lists
//! the built-in ones in the order they're asked, and a fork can add its own. A probe reads only
//! the window the server has *loaded*: a model's trained maximum is no use when the server runs
//! it with less (Ollama and LM Studio cut a prompt that doesn't fit their loaded size).

use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Duration;

/// Smallest and largest window we believe (anything else is a misread field).
const MIN_WINDOW: u64 = 512;
const MAX_WINDOW: u64 = 1 << 31;
/// How long one question to the server may take. A probe never waits for a model to load.
const ASK_TIMEOUT: Duration = Duration::from_secs(3);

/// Read JSON from the server; `None` for any failure (a probe that can't read just says nothing).
pub trait Fetch {
    fn get(&self, url: &str) -> Option<Value>;
}

/// The model a probe asks about, on the server it's served from.
#[derive(Debug, Clone, Copy)]
pub struct Server<'a> {
    /// The OpenAI-style base the chats go to, usually ending in `/v1`.
    pub base: &'a str,
    pub model: &'a str,
}

impl Server<'_> {
    /// The server's own root, for the endpoints outside `/v1`.
    pub fn root(&self) -> &str {
        let base = self.base.trim_end_matches('/');
        base.strip_suffix("/v1").unwrap_or(base)
    }

    /// This model's entry in the OpenAI-style model list.
    fn listed(&self, fetch: &dyn Fetch) -> Option<Value> {
        let list = fetch.get(&format!("{}/models", self.base.trim_end_matches('/')))?;
        let entries = list.get("data").or_else(|| list.get("models")).and_then(Value::as_array)?;
        entries.iter().find(|m| m.get("id").and_then(Value::as_str) == Some(self.model)).cloned()
    }
}

/// A window and which probe read it (for the journal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub tokens: u64,
    pub source: &'static str,
}

/// One way a server says how many tokens its model has room for.
pub trait WindowProbe: Send + Sync {
    /// A short name for the journal ("llama.cpp").
    fn source(&self) -> &'static str;
    fn window(&self, server: &Server<'_>, fetch: &dyn Fetch) -> Option<u64>;
}

/// The built-in probes, in the order they're asked. `on_this_network` is false for a remote
/// service, which is asked only through its model list.
pub fn probes(on_this_network: bool) -> Vec<Box<dyn WindowProbe>> {
    let mut all: Vec<Box<dyn WindowProbe>> = vec![Box::new(ModelList)];
    if on_this_network {
        all.push(Box::new(LlamaSwap));
        all.push(Box::new(LlamaCpp));
        all.push(Box::new(LmStudio));
        all.push(Box::new(Ollama));
    }
    all
}

/// The first believable window any probe reads.
pub fn read_window(server: &Server<'_>, fetch: &dyn Fetch, probes: &[Box<dyn WindowProbe>]) -> Option<Window> {
    probes.iter().find_map(|p| p.window(server, fetch).filter(|n| (MIN_WINDOW..=MAX_WINDOW).contains(n)).map(|tokens| Window { tokens, source: p.source() }))
}

/// Ask with the built-in probes. A model llama-swap lists but hasn't loaded is asked about only
/// through the list: llama-swap starts a model for almost any request naming it (`/props?model=`
/// too), unloading the one in use.
pub fn ask(server: &Server<'_>, fetch: &dyn Fetch, on_this_network: bool) -> Option<Window> {
    read_window(server, fetch, &probes(on_this_network && !would_start_it(server, fetch)))
}

/// Ask the server with `headers` (its key, when it has one). Each address is asked once.
pub fn read_from_server(server: &Server<'_>, headers: &[(&'static str, String)], on_this_network: bool) -> Option<Window> {
    let fetch = HttpFetch { agent: crate::http::quick_agent(ASK_TIMEOUT), headers, seen: RefCell::new(BTreeMap::new()) };
    ask(server, &fetch, on_this_network)
}

struct HttpFetch<'a> {
    agent: ureq::Agent,
    headers: &'a [(&'static str, String)],
    seen: RefCell<BTreeMap<String, Option<Value>>>,
}

impl Fetch for HttpFetch<'_> {
    fn get(&self, url: &str) -> Option<Value> {
        if let Some(v) = self.seen.borrow().get(url) {
            return v.clone();
        }
        let v = crate::http::get_json(&self.agent, "the model server", url, self.headers).ok();
        self.seen.borrow_mut().insert(url.to_string(), v.clone());
        v
    }
}

fn number(v: &Value, path: &[&str]) -> Option<u64> {
    path.iter().try_fold(v, |v, key| v.get(key))?.as_u64()
}

/// `context_length` in an OpenAI-style model list (OpenRouter, Together), `context_window` (Groq),
/// `max_model_len` (vLLM), or the loaded `meta.n_ctx` some llama.cpp-style servers list. Not `meta.n_ctx_train`: that's what the
/// model was trained with, not what the server loaded.
struct ModelList;

impl WindowProbe for ModelList {
    fn source(&self) -> &'static str {
        "model list"
    }
    fn window(&self, server: &Server<'_>, fetch: &dyn Fetch) -> Option<u64> {
        let m = server.listed(fetch)?;
        ["context_length", "context_window", "max_model_len"].iter().find_map(|k| number(&m, &[k])).or_else(|| number(&m, &["meta", "n_ctx"]))
    }
}

/// llama-swap lists a window only when its config gives one (the model list probe reads that),
/// but it passes `/upstream/<model>/…` to the model's own server. Asked only when the list says the model is loaded: asking an unloaded one would start it.
struct LlamaSwap;

impl WindowProbe for LlamaSwap {
    fn source(&self) -> &'static str {
        "llama-swap"
    }
    fn window(&self, server: &Server<'_>, fetch: &dyn Fetch) -> Option<u64> {
        let id = loaded_swap_model(server, fetch)?;
        let upstream = format!("{}/upstream/{id}", server.root());
        let props = fetch.get(&format!("{upstream}/props"));
        props.as_ref().and_then(props_window).or_else(|| {
            let inner = fetch.get(&format!("{upstream}/v1/models"))?;
            inner.get("data")?.as_array()?.iter().find_map(|e| number(e, &["meta", "n_ctx"]))
        })
    }
}

/// The model llama-swap serves `server.model` with (an alias is served by the model it names),
/// when its list says that one is loaded; `None` for any other server, or an unloaded model.
pub(crate) fn loaded_swap_model(server: &Server<'_>, fetch: &dyn Fetch) -> Option<String> {
    let m = server.listed(fetch)?;
    let swap = m.get("meta")?.get("llamaswap")?;
    let id = swap.get("modelID").and_then(Value::as_str).unwrap_or(server.model);
    if id != server.model {
        let list = fetch.get(&format!("{}/models", server.base.trim_end_matches('/')))?;
        let entries = list.get("data").and_then(Value::as_array)?;
        let target = entries.iter().find(|e| e.get("id").and_then(Value::as_str) == Some(id))?;
        if !loaded(target) {
            return None;
        }
    } else if !loaded(&m) {
        return None;
    }
    Some(id.to_string())
}

/// Whether llama-swap lists the model but hasn't loaded it, so asking its server anything about
/// the model would start it.
pub(crate) fn would_start_it(server: &Server<'_>, fetch: &dyn Fetch) -> bool {
    let is_swaps = server.listed(fetch).is_some_and(|m| m.get("meta").and_then(|meta| meta.get("llamaswap")).is_some());
    is_swaps && loaded_swap_model(server, fetch).is_none()
}

fn loaded(entry: &Value) -> bool {
    entry.get("status").and_then(|s| s.get("value")).and_then(Value::as_str) == Some("loaded")
}

/// llama.cpp's `/props`: the loaded window, at the top or in the default generation settings.
fn props_window(props: &Value) -> Option<u64> {
    number(props, &["default_generation_settings", "n_ctx"]).or_else(|| number(props, &["n_ctx"]))
}

/// llama.cpp's server, asked about the model by name (a router serving several needs it). A
/// router loads a model a request names unless told not to, and with `--models-max` that can
/// unload the one in use, so the question says `autoload=false` (a single-model server ignores it).
struct LlamaCpp;

impl WindowProbe for LlamaCpp {
    fn source(&self) -> &'static str {
        "llama.cpp"
    }
    fn window(&self, server: &Server<'_>, fetch: &dyn Fetch) -> Option<u64> {
        props_window(&fetch.get(&format!("{}/props?model={}&autoload=false", server.root(), query_text(server.model)))?)
    }
}

/// LM Studio's own API: the window the model was loaded with (none until it's loaded).
struct LmStudio;

impl WindowProbe for LmStudio {
    fn source(&self) -> &'static str {
        "LM Studio"
    }
    fn window(&self, server: &Server<'_>, fetch: &dyn Fetch) -> Option<u64> {
        number(&fetch.get(&format!("{}/api/v0/models/{}", server.root(), server.model))?, &["loaded_context_length"])
    }
}

/// Ollama's running models: the window it loaded the model with (none until it's loaded).
struct Ollama;

impl WindowProbe for Ollama {
    fn source(&self) -> &'static str {
        "Ollama"
    }
    fn window(&self, server: &Server<'_>, fetch: &dyn Fetch) -> Option<u64> {
        let ps = fetch.get(&format!("{}/api/ps", server.root()))?;
        let same = |name: &str| name == server.model || name.strip_suffix(":latest") == Some(server.model);
        ps.get("models")?
            .as_array()?
            .iter()
            .find(|m| ["name", "model"].iter().any(|k| m.get(k).and_then(Value::as_str).is_some_and(same)))
            .and_then(|m| number(m, &["context_length"]))
    }
}

/// `text` for a URL query: unreserved characters as they are, every other byte as `%XX`.
fn query_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Answers from a table; anything else is "no answer". Records what was asked.
    struct Canned {
        answers: BTreeMap<String, Value>,
        asked: RefCell<Vec<String>>,
    }

    impl Canned {
        fn new(answers: &[(&str, Value)]) -> Self {
            Canned { answers: answers.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(), asked: RefCell::new(Vec::new()) }
        }
    }

    impl Fetch for Canned {
        fn get(&self, url: &str) -> Option<Value> {
            self.asked.borrow_mut().push(url.to_string());
            self.answers.get(url).cloned()
        }
    }

    fn read(base: &str, model: &str, fetch: &Canned, local: bool) -> Option<Window> {
        ask(&Server { base, model }, fetch, local)
    }

    #[test]
    fn the_root_is_the_base_without_v1() {
        assert_eq!(Server { base: "http://h:8080/v1/", model: "m" }.root(), "http://h:8080");
        assert_eq!(Server { base: "http://h:1234", model: "m" }.root(), "http://h:1234");
    }

    #[test]
    fn a_model_list_with_context_length_answers_first() {
        let f = Canned::new(&[("https://or/api/v1/models", json!({"data": [{"id": "a"}, {"id": "b", "context_length": 200000}]}))]);
        assert_eq!(read("https://or/api/v1", "b", &f, false), Some(Window { tokens: 200_000, source: "model list" }));
    }

    #[test]
    fn each_model_list_field_is_read() {
        for (field, n) in [("context_window", 131_072_u64), ("max_model_len", 32_768)] {
            let f = Canned::new(&[("https://svc/v1/models", json!({"data": [{"id": "m", field: n}]}))]);
            assert_eq!(read("https://svc/v1", "m", &f, false).map(|w| w.tokens), Some(n), "{field}");
        }
    }

    #[test]
    fn a_remote_service_is_asked_only_for_its_model_list() {
        let f = Canned::new(&[]);
        assert_eq!(read("https://api.example/v1", "m", &f, false), None);
        assert_eq!(*f.asked.borrow(), ["https://api.example/v1/models"]);
    }

    #[test]
    fn the_trained_window_is_not_the_loaded_one() {
        let f = Canned::new(&[("http://h/v1/models", json!({"data": [{"id": "m", "meta": {"n_ctx_train": 131072}}]}))]);
        assert_eq!(read("http://h/v1", "m", &f, true), None);
    }

    #[test]
    fn llama_swap_is_asked_through_its_upstream_route() {
        let f = Canned::new(&[
            ("http://h/v1/models", json!({"data": [{"id": "org/m:q8", "meta": {"llamaswap": {"type": "model"}}, "status": {"value": "loaded"}}]})),
            ("http://h/upstream/org/m:q8/props", json!({"default_generation_settings": {"n_ctx": 524288}})),
        ]);
        assert_eq!(read("http://h/v1", "org/m:q8", &f, true), Some(Window { tokens: 524_288, source: "llama-swap" }));
    }

    #[test]
    fn a_llama_swap_alias_is_read_from_the_model_it_names() {
        let f = Canned::new(&[
            (
                "http://h/v1/models",
                json!({"data": [
                    {"id": "m-high", "meta": {"llamaswap": {"type": "alias", "modelID": "m"}}, "status": {"value": "loaded"}},
                    {"id": "m", "meta": {"llamaswap": {"type": "model"}}, "status": {"value": "loaded"}},
                ]}),
            ),
            ("http://h/upstream/m/v1/models", json!({"data": [{"id": "inner", "meta": {"n_ctx": 262144}}]})),
        ]);
        assert_eq!(read("http://h/v1", "m-high", &f, true).map(|w| w.tokens), Some(262_144));
    }

    #[test]
    fn an_unloaded_llama_swap_model_is_not_started_by_asking() {
        let f = Canned::new(&[("http://h/v1/models", json!({"data": [{"id": "m", "meta": {"llamaswap": {}}, "status": {"value": "unloaded"}}]}))]);
        assert_eq!(read("http://h/v1", "m", &f, true), None);
        // Not through its upstream route, and not as llama.cpp's `/props?model=` either: llama-swap
        // starts a model for that too (seen live), so only the list is read.
        assert!(!f.asked.borrow().is_empty() && f.asked.borrow().iter().all(|u| u == "http://h/v1/models"), "{:?}", f.asked.borrow());
    }

    /// Every question to llama.cpp names the model and says not to load it: a router would load
    /// it otherwise, perhaps unloading the one in use (`Canned` answers only the exact URL).
    #[test]
    fn llama_cpp_props_name_the_model() {
        let f = Canned::new(&[("http://h:8080/props?model=org%2Fm%3Aq4&autoload=false", json!({"n_ctx": 32768}))]);
        assert_eq!(read("http://h:8080/v1", "org/m:q4", &f, true), Some(Window { tokens: 32_768, source: "llama.cpp" }));
    }

    #[test]
    fn lm_studio_says_the_loaded_window() {
        let f = Canned::new(&[("http://h:1234/api/v0/models/qwen", json!({"max_context_length": 131072, "loaded_context_length": 8192}))]);
        assert_eq!(read("http://h:1234/v1", "qwen", &f, true), Some(Window { tokens: 8192, source: "LM Studio" }));
        let unloaded = Canned::new(&[("http://h:1234/api/v0/models/qwen", json!({"max_context_length": 131072, "state": "not-loaded"}))]);
        assert_eq!(read("http://h:1234/v1", "qwen", &unloaded, true), None, "the maximum isn't what it loads");
    }

    #[test]
    fn ollama_says_the_window_of_a_running_model() {
        let f = Canned::new(&[(
            "http://h:11434/api/ps",
            json!({"models": [{"name": "other:7b", "context_length": 2048}, {"name": "qwen:latest", "context_length": 32768}]}),
        )]);
        assert_eq!(read("http://h:11434/v1", "qwen", &f, true), Some(Window { tokens: 32_768, source: "Ollama" }));
        assert_eq!(read("http://h:11434/v1", "missing", &f, true), None);
    }

    #[test]
    fn an_unbelievable_window_is_ignored() {
        for n in [0_u64, 100, u64::MAX] {
            let f = Canned::new(&[("http://h/props?model=m&autoload=false", json!({"n_ctx": n}))]);
            assert_eq!(read("http://h/v1", "m", &f, true), None, "{n}");
        }
    }

    #[test]
    fn hostile_shapes_read_as_nothing() {
        let f = Canned::new(&[
            ("http://h/v1/models", json!({"data": "nope"})),
            ("http://h/props?model=m&autoload=false", json!({"n_ctx": "big"})),
            ("http://h/api/ps", json!({"models": [{"name": 5}]})),
            ("http://h/api/v0/models/m", json!([1, 2])),
        ]);
        assert_eq!(read("http://h/v1", "m", &f, true), None);
    }

    #[test]
    fn query_text_escapes_everything_but_unreserved() {
        assert_eq!(query_text("a-b_c.d~e/f:g h"), "a-b_c.d~e%2Ff%3Ag%20h");
    }
}
