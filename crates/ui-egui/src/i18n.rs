//! UI localisation: English strings in code are the lookup keys; a per-language
//! TSV catalog (`key<TAB>translation` per line, `#` comments) maps them to display text at render
//! time. A string with no translation is shown in English. Tool ids, the CLI and MCP never see
//! translated text.
//!
//! Adding a language: put `xx.tsv` in `i18n/` and add one row to [`LANGUAGES`].

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// One supported UI language.
pub struct LangInfo {
    /// BCP 47 code, lowercase. Also the `settings.language` value.
    pub code: &'static str,
    /// The language's name in itself.
    pub name: &'static str,
    /// Catalog contents (empty for the built-in English).
    pub source: &'static str,
}

pub static LANGUAGES: &[LangInfo] = &[
    LangInfo { code: "en", name: "English", source: "" },
    LangInfo { code: "de", name: "Deutsch", source: include_str!("i18n/de.tsv") },
    LangInfo { code: "ka", name: "ქართული", source: include_str!("i18n/ka.tsv") },
    LangInfo { code: "fr", name: "Français", source: include_str!("fr.tsv") },
    LangInfo { code: "it", name: "Italiano", source: include_str!("i18n/it.tsv") },
    LangInfo { code: "ja", name: "日本語", source: include_str!("ja.tsv") },
    LangInfo { code: "zh-hans", name: "简体中文", source: include_str!("i18n/zh-hans.tsv") },
    LangInfo { code: "zh-hant", name: "繁體中文", source: include_str!("i18n/zh-hant.tsv") },
    LangInfo { code: "ru", name: "Русский", source: include_str!("i18n/ru.tsv") },
    LangInfo { code: "ko", name: "한국어", source: include_str!("i18n/ko.tsv") },
    LangInfo { code: "cs", name: "Čeština", source: include_str!("i18n/cs.tsv") },
    LangInfo { code: "es", name: "Español", source: include_str!("i18n/es.tsv") },
    LangInfo { code: "vi", name: "Tiếng Việt", source: include_str!("i18n/vi.tsv") },
    LangInfo { code: "pt-br", name: "Português (Brasil)", source: include_str!("i18n/pt-br.tsv") },
    LangInfo { code: "tr", name: "Türkçe", source: include_str!("i18n/tr.tsv") },
    LangInfo { code: "th", name: "ไทย", source: include_str!("i18n/th.tsv") },
    LangInfo { code: "et", name: "Eesti", source: include_str!("i18n/et.tsv") },
];

/// Match a locale to a registered UI language. An explicit script wins over the region.
pub fn language_from_tag(tag: &str) -> Option<&'static str> {
    let tag = tag.trim().split(['.', '@']).next().unwrap_or_default().replace('_', "-").to_ascii_lowercase();
    if let Some(lang) = LANGUAGES.iter().find(|l| l.code == tag) {
        return Some(lang.code);
    }
    let mut parts = tag.split('-');
    let primary = parts.next()?;
    match primary {
        "zh" => {
            let parts: Vec<_> = parts.take_while(|p| p.len() != 1).collect();
            if parts.contains(&"hant") {
                Some("zh-hant")
            } else if parts.contains(&"hans") {
                Some("zh-hans")
            } else if parts.iter().any(|p| matches!(*p, "tw" | "hk" | "mo")) {
                Some("zh-hant")
            } else {
                Some("zh-hans")
            }
        }
        "pt" => Some("pt-br"),
        _ => LANGUAGES.iter().find(|l| l.code == primary).map(|l| l.code),
    }
}

fn preferred_language(tags: impl IntoIterator<Item = impl AsRef<str>>) -> &'static str {
    tags.into_iter().find_map(|tag| language_from_tag(tag.as_ref())).unwrap_or("en")
}

fn system_language() -> &'static str {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static SYSTEM: OnceLock<&'static str> = OnceLock::new();
        SYSTEM.get_or_init(|| preferred_language(sys_locale::get_locales()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        preferred_language(std::iter::empty::<&str>())
    }
}

fn current() -> &'static RwLock<(String, HashMap<String, String>)> {
    static CUR: OnceLock<RwLock<(String, HashMap<String, String>)>> = OnceLock::new();
    CUR.get_or_init(|| RwLock::new(("en".into(), HashMap::new())))
}

fn parse(source: &str) -> HashMap<String, String> {
    source
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once('\t'))
        .filter(|(k, v)| !k.is_empty() && !v.trim().is_empty())
        .map(|(k, v)| (k.replace("\\n", "\n"), v.trim_end().replace("\\n", "\n")))
        .collect()
}

/// Switch language. Empty means the system's preferred language; unsupported codes mean English.
pub fn set_language(code: &str) {
    let code = if code.trim().is_empty() { system_language() } else { language_from_tag(code).unwrap_or("en") };
    let lang = LANGUAGES.iter().find(|l| l.code == code);
    let map = lang.map(|l| parse(l.source)).unwrap_or_default();
    if let Ok(mut g) = current().write() {
        *g = (lang.map(|l| l.code).unwrap_or("en").to_string(), map);
    }
}

pub fn language() -> String {
    current().read().map(|g| g.0.clone()).unwrap_or_else(|_| "en".into())
}

/// Translate a UI string.
pub fn tr(s: &str) -> String {
    match current().read() {
        Ok(g) => g.1.get(s).cloned().unwrap_or_else(|| s.to_string()),
        Err(_) => s.to_string(),
    }
}

/// Translate, then fill `{name}` placeholders in one pass over the template, so an inserted value
/// stays literal even when it looks like a placeholder. Unknown names and stray braces are kept.
pub fn trf(s: &str, args: &[(&str, &str)]) -> String {
    let template = tr(s);
    if !template.contains('{') {
        return template;
    }
    let mut out = String::with_capacity(args.iter().fold(template.len(), |n, (_, v)| n.saturating_add(v.len())));
    let mut rest = template.as_str();
    while let Some((before, after)) = rest.split_once('{') {
        out.push_str(before);
        // The name runs to the next brace; a `{` first means this one opens no placeholder.
        let filled = after.find(['{', '}']).and_then(|end| {
            let (name, tail) = after.split_at_checked(end)?;
            let tail = tail.strip_prefix('}')?;
            Some((args.iter().find(|(k, _)| *k == name)?.1, tail))
        });
        match filled {
            Some((value, tail)) => {
                out.push_str(value);
                rest = tail;
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parse_and_fallback() {
        let m = parse("# comment\nHello\tHola\nEmpty\t \nbad line\n");
        assert_eq!(m.get("Hello").map(String::as_str), Some("Hola"));
        assert!(!m.contains_key("Empty"));
        assert_eq!(trf("Open {name}", &[("name", "a.exe")]), "Open a.exe");
        assert_eq!(tr("Untranslated"), "Untranslated");
    }

    /// Inserted values stay literal: a value that looks like a placeholder, its own or a later
    /// one, is never filled in turn.
    #[test]
    fn trf_keeps_inserted_values_literal() {
        let two = "Saved {saved}, using {used}.";
        assert_eq!(trf(two, &[("saved", "{used}"), ("used", "Claude")]), "Saved {used}, using Claude.");
        assert_eq!(trf(two, &[("used", "{saved}"), ("saved", "Off")]), "Saved Off, using {saved}.");
        assert_eq!(trf(two, &[("saved", "{saved}"), ("used", "{used}")]), two);
        assert_eq!(trf("{a}{a} {b}", &[("a", "{b}"), ("b", "x")]), "{b}{b} x");
        // Unknown, empty and unbalanced braces are kept as written; the first duplicate key wins.
        assert_eq!(trf("{x} {} {a {{a}} a}", &[("a", "1")]), "{x} {} {a {1} a}");
        assert_eq!(trf("{}", &[("", "empty")]), "empty");
        assert_eq!(trf("{a}", &[("a", "1"), ("a", "2")]), "1");
        assert_eq!(trf("été {nom} ✓", &[("nom", "Zoë")]), "été Zoë ✓");
        assert_eq!(trf("", &[("a", "1")]), "");
    }

    /// Script and region matching: an explicit script wins over a region, a registered language
    /// resolves from any of its regions, and a locale with no catalogue falls back to English.
    #[test]
    fn chinese_scripts_regions_and_fallback() {
        for tag in ["zh", "zh-CN", "zh_SG.UTF-8", "ZH-Hans", "zh-Hans-TW"] {
            assert_eq!(language_from_tag(tag), Some("zh-hans"), "{tag}");
        }
        for tag in ["zh-TW", "zh-HK", "zh-MO", "zh-Hant", "zh-Hant-CN"] {
            assert_eq!(language_from_tag(tag), Some("zh-hant"), "{tag}");
        }
        assert_eq!(language_from_tag("en-US"), Some("en"));
        assert_eq!(language_from_tag("ja-JP"), Some("ja"));
        assert_eq!(language_from_tag("fr-CA"), Some("fr"));
        assert_eq!(language_from_tag("de-DE"), Some("de"));
        assert_eq!(language_from_tag("de_AT.UTF-8"), Some("de"));
        assert_eq!(language_from_tag("es-ES"), Some("es"));
        assert_eq!(language_from_tag("es-MX"), Some("es"));
        assert_eq!(language_from_tag("sv-SE"), None);
        assert_eq!(preferred_language(["sv-SE", "zh-HK", "en-US"]), "zh-hant");
        assert_eq!(language_from_tag("vi-VN"), Some("vi"));
        assert_eq!(language_from_tag("vi_VN.UTF-8"), Some("vi"));
        assert_eq!(language_from_tag("pt_BR.UTF-8"), Some("pt-br"));
        assert_eq!(language_from_tag("pt-PT"), Some("pt-br"));
        assert_eq!(language_from_tag("tr-TR"), Some("tr"));
        assert_eq!(language_from_tag("th-TH"), Some("th"));
        assert_eq!(language_from_tag("th_TH.UTF-8"), Some("th"));
        assert_eq!(preferred_language(["de-DE", "zh-HK"]), "de");
        assert_eq!(preferred_language(["sv-SE", "vi-VN", "en-US"]), "vi");
        assert_eq!(preferred_language(["en-US", "zh-CN"]), "en");
        assert_eq!(preferred_language(["sv-SE"]), "en");
        assert_eq!(preferred_language(std::iter::empty::<&str>()), "en");
    }

    /// Every registered catalogue parses, and all of them carry the same key set: a language that
    /// missed keys would fall back to English mid-interface.
    #[test]
    fn catalogs_are_valid_and_have_matching_keys() {
        fn placeholders(s: &str) -> Vec<&str> {
            let mut out: Vec<_> = s.split('{').skip(1).filter_map(|s| s.split_once('}').map(|(p, _)| p)).collect();
            out.sort_unstable();
            out
        }
        let mut catalogs = Vec::new();
        for lang in LANGUAGES
            .iter()
            .filter(|l| matches!(l.code, "zh-hans" | "zh-hant" | "ru" | "ko" | "cs" | "de" | "ka" | "es" | "it" | "vi" | "pt-br" | "tr" | "th" | "et"))
        {
            let mut keys = std::collections::BTreeSet::new();
            for (i, line) in lang.source.lines().enumerate() {
                if line.trim().is_empty() || line.starts_with('#') {
                    continue;
                }
                let (key, value) = line.split_once('\t').unwrap_or_else(|| panic!("{}:{}: missing tab", lang.code, i + 1));
                assert!(!key.is_empty() && !value.trim().is_empty(), "{}:{}: empty column", lang.code, i + 1);
                assert!(!value.contains('\t'), "{}:{}: extra column", lang.code, i + 1);
                assert!(keys.insert(key.to_string()), "{}:{}: duplicate {key}", lang.code, i + 1);
                assert_eq!(placeholders(key), placeholders(value), "{}: {key}", lang.code);
                assert_eq!(key.ends_with('…'), value.ends_with('…'), "{}: {key}", lang.code);
            }
            let parsed = parse(lang.source);
            assert_eq!(parsed.len(), keys.len());
            assert!(keys.len() > 500, "{}: incomplete catalog", lang.code);
            catalogs.push(keys);
        }
        assert!(catalogs.len() >= 2);
        for c in catalogs.iter().skip(1) {
            assert_eq!(catalogs.first(), Some(c));
        }
    }

    /// Every translation keeps its key's `{placeholders}`, or `trf` would leave them unfilled.
    #[test]
    fn catalogs_keep_placeholders() {
        let holes = |s: &str| {
            let mut v: Vec<String> = s.split('{').skip(1).filter_map(|p| p.split_once('}')).map(|(h, _)| h.to_string()).collect();
            v.sort();
            v
        };
        for l in LANGUAGES {
            let m = parse(l.source);
            assert!(l.source.is_empty() || m.len() > 100, "{} catalog barely parsed", l.code);
            for (k, v) in &m {
                assert_eq!(holes(k), holes(v), "{}: {k:?}", l.code);
            }
        }
    }
}
