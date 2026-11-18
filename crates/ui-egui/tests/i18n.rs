//! Keep locale-changing UI checks in one integration-test executable: i18n is process-wide.

use craft_anything_engine::{Mode, Session, Settings};
use craft_anything_ui_egui::{CraftApp, demo, i18n};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::path::Path;

fn harness(mut settings: Settings, dir: &Path) -> Harness<'static, CraftApp> {
    let dir = dir.to_path_buf();
    // Keep every engine path under the test directory, including after a saved-settings reload.
    settings.engines.store_dir = Some(dir.join("engines"));
    settings.projects_dir = Some(dir.join("projects"));
    Harness::builder().with_size(egui::vec2(1280.0, 1120.0)).with_max_steps(32).build_eframe(move |cc| {
        CraftApp::setup_context(&cc.egui_ctx);
        let engine = demo::engine(Mode::Beginner, true);
        {
            // The home/settings scenario needs no sample target. Isolate the session and set
            // its language BEFORE CraftApp::new (called by demo::app) reads its settings.
            let mut session = engine.session();
            *session = Session::isolated(&dir);
            session.settings = settings;
        }
        let mut app = demo::app(engine);
        // demo::app keeps persistence disabled and uses an isolated, file-only AI key store.
        // Never enable the real settings worker in this test: Settings::save uses the global
        // config path. The roundtrip below uses save_to with an explicit temporary path.
        assert!(!app.persist);
        assert!(app.headless);
        app.state.settings_open = true;
        app
    })
}

#[test]
fn chinese_startup_language_menu_and_saved_selection() {
    let temp = tempfile::tempdir().unwrap();
    for (initial, native, other, other_native, translated_language, translated_open) in [
        ("zh-hans", "简体中文", "zh-hant", "繁體中文", "语言", "已打开 sample.exe"),
        ("zh-hant", "繁體中文", "zh-hans", "简体中文", "語言", "已開啟 sample.exe"),
    ] {
        let dir = temp.path().join(initial);
        let settings = Settings { mode: Mode::Beginner, first_run_done: true, language: initial.into(), ..Default::default() };
        let mut h = harness(settings, &dir);
        h.run_steps(3);

        // Startup must apply the saved locale itself, rather than relying on a manual call to
        // set_language after app construction. Check both lookup and a rendered settings row.
        assert_eq!(i18n::language(), initial);
        h.get_by_label(translated_language);
        assert_eq!(i18n::trf("Opened {name}", &[("name", "sample.exe")]), translated_open);
        assert_eq!(i18n::tr("A deliberately uncatalogued message"), "A deliberately uncatalogued message");
        assert_eq!(i18n::trf("Uncatalogued {name}", &[("name", "sample.exe")]), "Uncatalogued sample.exe");
        assert_eq!(craft_anything_ui_egui::usage::asset_cost_words(0.0031), if initial == "zh-hans" { "低于 $0.01" } else { "低於 $0.01" });
        let kinds = craft_anything_ai::rebuild::KindRegistry::built_in();
        for (suggestion, expected) in [("A website for my bakery", "website"), ("A simple snake game", "game")] {
            let translated = i18n::tr(suggestion);
            assert_ne!(translated, suggestion);
            let (kind, _) = kinds.choose(&translated, &craft_anything_engine::workspace::TargetRef::None, false).unwrap();
            assert_eq!(kind.id(), expected, "{initial}: {translated}");
        }
        let title = h.state().engine.registry().get("native_functions_list").unwrap().title;
        let translated_title = i18n::tr(title);
        assert_ne!(translated_title, title);
        h.state_mut().state.settings_open = false;
        h.state_mut().state.palette.open = true;
        h.state_mut().state.palette.query = translated_title.clone();
        h.run_steps(3);
        h.get_by_label(&translated_title);
        h.state_mut().state.palette.open = false;
        h.state_mut().state.settings_open = true;
        h.run_steps(3);

        // Exercise the real ComboBox and selectable row, including the language-change callback.
        h.get_by_value(native).click();
        h.run();
        h.get_by_label(other_native).scroll_to_me();
        h.run();
        let choice_rect = h.get_by_label(other_native).rect();
        h.get_by_label(other_native).click();
        h.run_steps(3);
        assert_eq!(i18n::language(), other, "language choice at {choice_rect:?}");
        assert_eq!(h.state().settings.language, other);
        let selected = h.state().engine.session().settings.clone();
        assert_eq!(selected.language, other, "the UI must propagate the choice to the engine session");

        // Verify the persisted settings representation and the next startup using only a
        // temporary config.toml. This does not claim to test the UI's debounce worker path.
        let config = dir.join("config.toml");
        selected.save_to(&config).unwrap();
        let loaded = Settings::load_from(&config);
        assert_eq!(loaded.language, other);
        drop(h);
        let mut restored = harness(loaded, &dir);
        restored.run_steps(3);
        assert_eq!(i18n::language(), other);
        restored.get_by_value(other_native);
        restored.get_by_label(if other == "zh-hans" { "语言" } else { "語言" });

        // English remains available from either Chinese catalog and takes effect immediately.
        restored.get_by_value(other_native).click();
        restored.run();
        restored.get_by_label("English").scroll_to_me();
        restored.run();
        restored.get_by_label("English").click();
        restored.run_steps(3);
        assert_eq!(i18n::language(), "en");
        assert_eq!(restored.state().settings.language, "en");
        assert_eq!(restored.state().engine.session().settings.language, "en");
        restored.get_by_label("Language");
        assert!(restored.query_by_label("语言").is_none());
        assert!(restored.query_by_label("語言").is_none());

        restored.get_by_value("English").click();
        restored.run();
        // Theme has a System choice too, on Studio's line: take the language menu's.
        let off_theme_line = |y: f32, line: f32| (y - line).abs() >= 4.0;
        let line = restored.get_by_label("Studio").rect().center().y;
        restored.query_all_by_label("System").find(|n| off_theme_line(n.rect().center().y, line)).expect("the language menu's System").scroll_to_me();
        restored.run();
        let line = restored.get_by_label("Studio").rect().center().y;
        restored.query_all_by_label("System").find(|n| off_theme_line(n.rect().center().y, line)).expect("the language menu's System").click();
        restored.run_steps(3);
        assert!(restored.state().settings.language.is_empty());
        let expected = sys_locale::get_locales().find_map(|tag| i18n::language_from_tag(&tag)).unwrap_or("en");
        assert_eq!(i18n::language(), expected);
        assert!(restored.state().engine.session().settings.language.is_empty());
    }
    for (code, native, label) in [
        ("de", "Deutsch", "Sprache"),
        ("fr", "Français", "Langue"),
        ("ja", "日本語", "言語"),
        ("ka", "ქართული", "ენა"),
        ("ko", "한국어", "언어"),
        ("vi", "Tiếng Việt", "Ngôn ngữ"),
    ] {
        let settings = Settings { mode: Mode::Beginner, first_run_done: true, language: code.into(), ..Default::default() };
        let mut existing = harness(settings, &temp.path().join(code));
        existing.run_steps(3);
        assert_eq!(i18n::language(), code);
        existing.get_by_label(label);
        existing.get_by_value(native);
    }
    i18n::set_language("unsupported-locale");
    assert_eq!(i18n::language(), "en");
    assert_eq!(i18n::tr("Settings"), "Settings");
    builder_replacement_notices(temp.path());
}

/// Run alongside the other locale checks in this test, never concurrently with them.
fn builder_replacement_notices(dir: &Path) {
    use craft_anything_ai::{AiHub, BackendConfig, BackendKind, Detected, KeyStore};
    use std::sync::Arc;

    for code in i18n::LANGUAGES.iter().map(|l| l.code).chain(["tr-TR", "unsupported-locale"]) {
        let dir = dir.join(format!("builder-{code}"));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = Settings { mode: Mode::Beginner, first_run_done: true, language: code.into(), ..Default::default() };
        let mut h = harness(settings, &dir);
        let turkish = i18n::language() == "tr";
        let notice = |saved: &str, used: &str| {
            if turkish {
                format!(
                    "Bu proje için seçilen yapay zekâ ({saved}) devre dışı bırakılmış veya artık yapılandırılmamış. Projeyi bunun yerine {used} oluşturacak."
                )
            } else {
                craft_anything_ai::rebuild::replaced_builder_notice(saved, used)
            }
        };
        assert_eq!(
            craft_anything_ai::rebuild::replaced_builder_notice("Off", "Claude"),
            "The AI chosen for this project (Off) is turned off or no longer set up. Claude builds instead.",
            "{code}: changing the UI language must not change the headless notice",
        );
        h.state_mut().detected = Some(Vec::new());
        h.state_mut().backends.push(BackendConfig {
            name: "Off".into(),
            kind: BackendKind::Anthropic,
            model: "test-model".into(),
            base_url: None,
            ssh_host: None,
            enabled: false,
        });

        // Disabled and removed choices retain both names, including Unicode and punctuation.
        for saved in ["Off", "Gone", "Önceki AI (özel)", "{used}", "{saved}"] {
            assert_eq!(h.state_mut().builder_backend(Some(saved)).as_deref(), Some("Claude"));
            assert_eq!(h.state().state.toasts.last(), Some(&(notice(saved, "Claude"), false)), "{code}");
        }

        // The Make picker uses the same localised notice and still clears its stale choice.
        h.state_mut().state.settings_open = false;
        h.state_mut().state.make.ai = Some("Off".into());
        h.run_steps(3);
        assert!(h.state().state.make.ai.is_none());
        h.get_by_label(&notice("Off", "Claude"));

        h.state_mut().state.toasts.clear();
        for wanted in [Some("Claude"), None, Some("")] {
            assert_eq!(h.state_mut().builder_backend(wanted).as_deref(), Some("Claude"));
            assert!(h.state().state.toasts.is_empty(), "{code}: no replacement");
        }
        h.state_mut().backends.clear();
        assert!(h.state_mut().builder_backend(Some("Off")).is_none());
        assert!(h.state().state.toasts.is_empty(), "{code}: nothing can build");

        // A discovered agent is announced only after its setup succeeds.
        h.state_mut().detected = Some(vec![Detected {
            suggestion: BackendConfig {
                name: "Yeni AI {saved} {used}".into(),
                kind: BackendKind::AgentCli,
                model: "claude-code".into(),
                base_url: None,
                ssh_host: None,
                enabled: true,
            },
            reason: "Fixture".into(),
            needs_key: false,
        }]);
        let hub = h.state().hub.clone();
        let blocked = dir.join("blocked");
        std::fs::write(&blocked, "a file, not a settings folder").unwrap();
        h.state_mut().hub = Arc::new(AiHub::with_stores(h.state().engine.clone(), blocked.join("config.toml"), KeyStore::file_only(dir.join("keys.toml"))));
        assert!(h.state_mut().builder_backend(Some("Off")).is_none());
        assert!(!h.state().backends.iter().any(|b| b.name == "Yeni AI {saved} {used}"));
        assert!(!h.state().state.toasts.is_empty());
        assert!(h.state().state.toasts.iter().all(|(_, error)| *error), "{code}: failed setup must not claim an agent builds");

        h.state_mut().hub = hub;
        assert_eq!(h.state_mut().builder_backend(Some("Off")).as_deref(), Some("Yeni AI {saved} {used}"));
        assert_eq!(h.state().state.toasts.last(), Some(&(notice("Off", "Yeni AI {saved} {used}"), false)), "{code}");
    }
    i18n::set_language("en");
}
