use super::*;
use serde_json::{Value, json};

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    dir: String,
    settings: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!("quotabar-mac-binding-{}", Uuid::new_v4()));
        let dir = base.join("Claude's 설정");
        fs::create_dir_all(&dir).unwrap();
        Self {
            root: base.join("profiles"),
            settings: dir.join("settings.json"),
            dir: dir.to_str().unwrap().into(),
            base,
        }
    }
    fn write(&self, value: Value) {
        fs::write(&self.settings, serde_json::to_vec(&value).unwrap()).unwrap();
    }
    fn read(&self) -> Value {
        serde_json::from_slice(&fs::read(&self.settings).unwrap()).unwrap()
    }
    fn install(&self, helper: &str, renew: bool) -> Result<Profile> {
        install(&self.root, &self.dir, "Claude".into(), helper, renew)
    }
    fn call(&self, request: Value) -> Result<Value> {
        Ok(serde_json::from_slice(&run(
            &self.root,
            &serde_json::to_vec(&request)?,
        )?)?)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

#[test]
fn reconnect_rollback_restore_and_keep_user_edits() {
    let f = Fixture::new();
    let original = json!({"command":"printf '/a/β'","padding":3,"type":"command"});
    f.write(json!({"statusLine":original,"env":{"KEEP":"yes"},"hooks":[true]}));
    let mut old = f.install("/Applications/CLI's app", false).unwrap();
    let backup = old.original_status_line.as_ref().unwrap();
    assert_eq!(serde_json::from_slice::<Value>(backup).unwrap(), original);
    assert!(
        old.installed_command
            .as_ref()
            .unwrap()
            .contains(r"CLI'\''s app")
    );
    assert_eq!(old.generation, old.generation.to_ascii_uppercase());
    old.email = Some("local@example.invalid".into());
    old.subscription_type = Some("max".into());
    old.save(&f.root).unwrap();
    let next = f.install("/new/helper", false).unwrap();
    assert_eq!(next.generation, old.generation);
    assert_eq!(next.email, old.email);
    assert_eq!(next.original_status_line, old.original_status_line);
    let mut state = f.read();
    state["statusLine"]["padding"] = json!(8);
    state["other"] = json!(17);
    f.write(state);
    rollback(&f.root, &next, Some(&old)).unwrap();
    assert_eq!(Profile::load(&f.root.join(&old.id), &old.id).unwrap(), old);
    assert_eq!(
        f.read()["statusLine"]["command"],
        json!(old.installed_command)
    );
    assert!(uninstall(&f.root, &old.id, false).unwrap());
    assert_eq!(
        f.read(),
        json!({"statusLine":{"command":"printf '/a/β'","type":"command","padding":8},"env":{"KEEP":"yes"},"hooks":[true],"other":17})
    );
    assert_eq!(f.call(json!({"connected":{}})).unwrap(), json!([]));
    assert_ne!(
        Profile::load(&f.root.join(&old.id), &old.id)
            .unwrap()
            .generation,
        old.generation
    );
}

#[test]
fn disconnected_wrapper_preservation_and_edited_removal() {
    let f = Fixture::new();
    let old = f.install("/helper", false).unwrap();
    let previous = f.read();
    let preserve = |active, cmd: Option<&str>| {
        f.call(json!({"preserve":{"directory":f.dir,"activeObject":active,"incomingCommand":cmd}}))
            .unwrap()
    };
    assert_eq!(preserve(true, None), true);
    let mut s = previous.clone();
    s["statusLine"]["padding"] = json!(5);
    f.write(s);
    assert!(!uninstall(&f.root, &old.id, false).unwrap());
    assert!(matches!(
        uninstall(&f.root, &old.id, true),
        Err(Error::RestoreNeeded)
    ));
    f.write(json!({"statusLine":{"type":"command","command":"my own"}}));
    assert!(!uninstall(&f.root, &old.id, true).unwrap());
    f.write(previous.clone());
    assert!(uninstall(&f.root, &old.id, true).unwrap());
    assert_eq!(preserve(false, old.installed_command.as_deref()), true);
    assert_eq!(preserve(true, Some("--claude-statusline other")), false);
    f.write(previous);
    let recovered = f.install("/other", true).unwrap();
    assert_ne!(recovered.generation, old.generation);
    assert_eq!(recovered.original_status_line, None);
}

#[test]
fn failed_connections_preserve_original_bytes_and_generations() {
    let f = Fixture::new();
    for (value, expected) in [
        (Value::Null, -52),
        (json!(7), -52),
        (json!([]), -52),
        (json!({"type":"text","command":"x"}), -51),
        (json!({"type":"command"}), -51),
        (
            json!({"type":"command","command":"--claude-statusline other"}),
            -48,
        ),
    ] {
        f.write(json!({"statusLine":value}));
        let before = fs::read(&f.settings).unwrap();
        assert_eq!(f.install("x", false).unwrap_err().status(), expected);
        assert_eq!(before, fs::read(&f.settings).unwrap());
    }
    f.write(json!({}));
    let p = f.install("first", false).unwrap();
    let before = fs::read(&f.settings).unwrap();
    let disk = f.root.join(&p.id).join(ACTIVE);
    fs::write(&disk, b"bad bytes").unwrap();
    assert_eq!(f.install("second", true).unwrap_err().status(), -47);
    assert_eq!(fs::read(&disk).unwrap(), b"bad bytes");
    fs::remove_file(&disk).unwrap();
    assert_eq!(f.install("second", true).unwrap_err().status(), -49);
    assert_eq!(before, fs::read(&f.settings).unwrap());
    let mut recursive = p.clone();
    recursive.original_status_line = Some(br#"{"command":"--claude-statusline nested"}"#.to_vec());
    recursive.save(&f.root).unwrap();
    assert_eq!(f.install("second", true).unwrap_err().status(), -50);
    assert_eq!(before, fs::read(&f.settings).unwrap());
}

#[test]
fn old_wrappers_and_stale_rollback_do_not_clobber_new_bindings() {
    let f = Fixture::new();
    f.write(json!({"statusLine":{"type":"command","command":"old shell"}}));
    let first = f.install("one", false).unwrap();
    let wrapper = f.read();
    let newer = f.install("two", true).unwrap();
    rollback(&f.root, &first, None).unwrap();
    assert_eq!(
        f.read()["statusLine"]["command"],
        json!(newer.installed_command)
    );
    let mut renamed = newer.clone();
    renamed.name = "새 이름".into();
    renamed.save(&f.root).unwrap();
    rollback(&f.root, &newer, Some(&first)).unwrap();
    assert_eq!(
        Profile::load(&f.root.join(&newer.id), &newer.id).unwrap(),
        renamed
    );
    assert_eq!(
        f.read()["statusLine"]["command"],
        json!(newer.installed_command)
    );
    f.write(wrapper);
    let restored = f.install("three", true).unwrap();
    assert_eq!(restored.original(), Some("old shell".into()));
    let mut changed = f.read();
    changed["statusLine"]["command"] = json!("user changed");
    f.write(changed.clone());
    rollback(&f.root, &restored, Some(&newer)).unwrap();
    assert_eq!(f.read(), changed);
}

#[test]
fn recovery_follows_the_external_command_and_detects_concurrent_settings_edits() {
    let f = Fixture::new();
    let p = f.install("/one", false).unwrap();
    let folder = f.root.join(&p.id);
    let pending = folder.join(PENDING);
    let mut next = p.clone();
    next.installed_command = Some("/next --claude-statusline".into());
    next.generation = generation();
    write_private(&pending, &serde_json::to_vec(&next).unwrap()).unwrap();
    assert_eq!(Profile::load(&folder, &p.id).unwrap(), p);
    assert!(!pending.exists());
    write_private(&pending, &serde_json::to_vec(&next).unwrap()).unwrap();
    f.write(json!({"statusLine":{"type":"command","command":next.installed_command}}));
    assert_eq!(Profile::load(&folder, &p.id).unwrap(), next);
    let mut old = Config::read(&f.dir).unwrap();
    old.set_status(None).unwrap();
    f.write(json!({"statusLine":{"command":"foreign"},"flag":7}));
    let before = fs::read(&f.settings).unwrap();
    assert!(matches!(commit(&folder, &old, &p), Err(Error::Changed)));
    assert_eq!(fs::read(&f.settings).unwrap(), before);
    assert_eq!(Profile::load(&folder, &p.id).unwrap(), next);
    assert!(!pending.exists());
}

#[test]
fn routing_treats_cli_environment_and_json_field_types_separately() {
    let f = Fixture::new();
    for key in AUTH_KEYS {
        for (v, active) in [
            (json!(""), false),
            (json!(" \tFaLsE\u{a0}"), false),
            (json!("0"), false),
            (json!("\n"), true),
            (json!(null), true),
            (json!(false), true),
            (json!(0), true),
            (json!("local"), true),
        ] {
            f.write(json!({"env":{*key:v}}));
            assert_eq!(
                f.call(json!({"routes":{"directory":f.dir}})).unwrap(),
                active,
                "{key}"
            );
        }
    }
    f.write(json!({"apiKeyHelper":"\t\u{200b}","env":{"HTTPS_PROXY":"keep"}}));
    assert!(!Config::read(&f.dir).unwrap().routes_through_api());
    f.write(json!({"apiKeyHelper":"\n"}));
    assert!(Config::read(&f.dir).unwrap().routes_through_api());
    assert!(matches!(validated(" relative "), Err(Error::Directory)));
    assert!(matches!(
        profile_folder(&f.root, "../not-a-profile"),
        Err(Error::InvalidId)
    ));
}

#[test]
fn profile_decoder_preserves_legacy_foundation_metadata_and_external_numeric_fields() {
    let f = Fixture::new();
    fs::write(
        &f.settings,
        br#"{"large":18446744073709551615,"statusLine":{"type":"command","command":"/a/b"}}"#,
    )
    .unwrap();
    let p = f.install("helper", false).unwrap();
    assert_eq!(f.read()["large"].as_u64(), Some(u64::MAX));
    let folder = f.root.join(&p.id);
    let path = folder.join(ACTIVE);
    let json = serde_json::to_string(&p).unwrap().replacen(
        "\"name\":",
        "\"name\":\"legacy\",\"name\":",
        1,
    );
    let mut data = vec![0xff, 0xfe];
    data.extend(json.encode_utf16().flat_map(u16::to_le_bytes));
    fs::write(&path, &data).unwrap();
    let loaded = Profile::load(&folder, &p.id).unwrap();
    assert_eq!(loaded.name, "legacy");
    assert_eq!(loaded.original(), Some("/a/b".into()));
    assert_eq!(fs::read(path).unwrap(), data);
}

#[test]
fn quota_display_preserves_identity_and_all_six_subscription_states() {
    let f = Fixture::new();
    let mut p = f.install("helper", false).unwrap();
    p.email = Some("claude@example.invalid".into());
    p.organization_name = Some("Research".into());
    p.subscription_type = Some(" Max ".into());
    p.save(&f.root).unwrap();
    let now = 800_000_000.25;
    let fetch = |time| {
        f.call(json!({"fetch":{"directory":f.dir,"authMethod":"auto","sourceKind":"claude-cli-profile","now":time}})).unwrap()
    };
    let mut state = f.read();
    assert_eq!(fetch(now)["extra"]["snapshotState"], "waiting");
    state["env"] = json!({"ANTHROPIC_API_KEY":"example"});
    f.write(state.clone());
    let routed = fetch(now);
    assert_eq!(routed["extra"]["snapshotState"], "api-routed");
    assert_eq!(routed["fetchedAt"], "");
    assert_eq!(routed["accountName"], "claude@example.invalid");
    assert_eq!(routed["extra"]["organizationName"], "Research");
    assert_eq!(routed["accountPlan"], "Max");
    let session = "12345678-A001-4823-8172-000000000001";
    let sample = json!({"session_id":session,"rate_limits":{"five_hour":{"used_percentage":29.5,"resets_at":now+978307200.+600.},"seven_day":{"used_percentage":100,"resets_at":now+978307200.-30.}}});
    let sample = crate::claude::mac_snapshots::Snapshot::parse(
        &serde_json::to_vec(&sample).unwrap(),
        &p.id,
        &p.generation,
        now,
    )
    .unwrap()
    .unwrap();
    sample.save(&f.root.join(&p.id)).unwrap();
    let usage = fetch(now + 300.);
    assert_eq!(usage["extra"]["snapshotState"], "snapshot");
    assert_eq!(usage["primary"]["remainingPercent"], 70.5);
    assert_eq!(usage["secondary"]["remainingPercent"], 100);
    assert_eq!(
        usage["secondary"]["resetDescription"],
        "Starts with next message"
    );
    assert_eq!(fetch(now + 300.0001)["extra"]["snapshotState"], "stale");
    assert_eq!(fetch(now + 600.)["extra"]["snapshotState"], "reset");
    state["statusLine"]["command"] = json!("my own status");
    f.write(state);
    assert_eq!(fetch(now)["extra"]["snapshotState"], "disconnected");
    assert!(matches!(f.call(json!({"fetch":{"directory":"unread","authMethod":"token","sourceKind":"other","now":now}})), Err(Error::LoginReference)));
}
