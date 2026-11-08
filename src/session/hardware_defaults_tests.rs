use super::*;

fn session_with_far_offer(name: &str) -> Session {
    let mut session = Session::new_mode(name.into(), false);
    let mut hello = vec![1, 1];
    hello.extend(8192u32.to_be_bytes());
    hello.extend(4u16.to_be_bytes());
    for value in [-64i32, 384, 63, 27] {
        hello.extend(value.to_be_bytes());
    }
    session.far.enter_dimension("minecraft:overworld", None);
    session.far.hello(&hello).unwrap();
    session
}

#[test]
fn software_defaults_change_only_unselected_values() {
    let mut session = session_with_far_offer("software-defaults");
    session.set_software_renderer(true);
    assert_eq!(session.render_distance, 4);
    assert_eq!(session.far.metrics()["distance"], 256);
    assert!(session.client_defaults.saved.is_empty());
    assert_eq!(session.client_settings_snapshot()["render_distance"], 4);
    session.set_software_renderer(false);
    assert_eq!(session.render_distance, 8);
    assert_eq!(
        session.far.metrics()["distance"],
        crate::far::LOW_PRESET_DISTANCE
    );
}

#[test]
fn saved_choices_survive_adapter_classification() {
    let mut session = session_with_far_offer("saved-software-settings");
    session.apply_persisted_settings(BTreeMap::from([
        ("render_distance".into(), json!(12)),
        ("graphics_preset".into(), json!("high")),
        ("far_terrain".into(), json!(768)),
    ]));
    session.set_software_renderer(true);
    assert_eq!(session.render_distance, 12);
    assert_eq!(
        session.native_setting("graphics_preset"),
        Some(&json!("high"))
    );
    assert_eq!(session.far.metrics()["distance"], 768);
}

#[test]
fn invalid_saved_values_do_not_disable_capacity_defaults() {
    let mut session = session_with_far_offer("invalid-software-settings");
    session.apply_persisted_settings(BTreeMap::from([
        ("render_distance".into(), json!("invalid")),
        ("graphics_preset".into(), json!("invalid")),
        ("far_terrain".into(), json!("invalid")),
    ]));
    session.set_software_renderer(true);
    assert_eq!(session.render_distance, 4);
    assert_eq!(session.far.metrics()["distance"], 256);
}

#[test]
fn saving_a_choice_does_not_persist_automatic_defaults() {
    const CHILD: &str = "SOLARIS_HARDWARE_DEFAULTS_TEST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let path = crate::local_config::client_settings_path().unwrap();
        let mut session = session_with_far_offer("explicit-setting-save");
        session.set_software_renderer(true);
        session.apply_client_setting("volume", &json!(0.5)).unwrap();
        let saved = crate::local_config::load_client_settings_from(&path);
        assert_eq!(saved, BTreeMap::from([("volume".into(), json!(0.5))]));
        assert_eq!(session.render_distance, 4);
        assert_eq!(session.far.metrics()["distance"], 256);
        session
            .apply_client_setting("render_distance", &json!(12))
            .unwrap();
        session
            .apply_client_setting("graphics_preset", &json!("high"))
            .unwrap();
        let saved = crate::local_config::load_client_settings_from(&path);
        assert_eq!(saved["render_distance"], 12);
        assert_eq!(saved["graphics_preset"], "high");
        assert_eq!(saved["volume"], 0.5);
        assert!(!saved.contains_key("far_terrain"));
        session.set_software_renderer(true);
        assert_eq!(session.render_distance, 12);
        assert_eq!(
            session.far.metrics()["distance"],
            crate::far::DEFAULT_DISTANCE
        );
        let mut saved = crate::local_config::load_client_settings_from(&path);
        saved.insert("far_terrain".into(), json!(768));
        session.apply_persisted_settings(saved);
        session
            .apply_client_setting("volume", &json!(0.25))
            .unwrap();
        let saved = crate::local_config::load_client_settings_from(&path);
        assert_eq!(
            saved["far_terrain"], 768,
            "an explicit far choice must survive other saves"
        );
        return;
    }
    let root = std::env::temp_dir().join(format!(
        "solaris-hardware-defaults-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "session::actions::hardware_defaults_tests::saving_a_choice_does_not_persist_automatic_defaults", "--nocapture"])
        .env(CHILD, "1")
        .env("SOLARIS_CLIENT_SETTINGS", root.join("settings.json"))
        .env_remove("SOLARIS_CLIENT_FAR_TERRAIN")
        .output().unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
