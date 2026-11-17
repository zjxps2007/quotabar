use super::*;
use serde_json::{Value, json};

const ACCOUNT: &str = "492c628f-ec48-4b07-bf81-a2a5cb8c7254";
const LOGIN: &[u8] =
    br#"{"access_token":"old","refresh_token":"stay","account_id":"a","user_id":"u"}"#;
const ROTATED: &[u8] =
    br#"{"access_token":"new","refresh_token":"stay","account_id":"a","user_id":"u"}"#;

struct Store {
    root: std::path::PathBuf,
    path: std::path::PathBuf,
    vault: std::path::PathBuf,
}
impl Store {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("quotabar-mac-settings-{}", Uuid::new_v4()));
        crate::files::ensure_private_directory(&root).unwrap();
        Self {
            path: root.join("settings.json"),
            vault: root.join("Credentials"),
            root,
        }
    }
    fn execute(&self, path: &Path, request: Value, data: &[u8]) -> Result<Value, Error> {
        let reply = run(
            path,
            Some(&self.vault),
            &serde_json::to_vec(&request)?,
            data,
        )?;
        Ok(serde_json::from_slice(&reply)?)
    }
    fn edit(&self, state: &Value, edit: Value) -> Result<Value, Error> {
        self.execute(
            &self.path,
            json!({"edit":{"settings":state,"edit":edit,"storage":"disk"}}),
            &[],
        )
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn legacy() -> Value {
    json!({"accounts":[{"id":ACCOUNT,"provider":"codex","name":"별칭","identity":"codex:account:a:user:u",
        "sourcePath":"/old","enabled":false,"showInMenuBar":false,"createdAt":-12345.123456789}],
        "refreshSeconds":300,"scanLocalLogs":true})
}

#[test]
fn selects_imported_login_without_committing_a_pending_rotation() {
    use imported::{Error, Import, Selection::*};
    let store = Store::new();
    let source = store.root.join("로그인.json");
    let id = Uuid::parse_str(ACCOUNT).unwrap();
    let request: Import = serde_json::from_value(json!({
        "id": ACCOUNT, "provider": "gemini", "sourcePath": source,
        "identity": "gemini:email:cafe\u{301}@example.test"
    }))
    .unwrap();
    let make = |token: &str, email: &str| {
        serde_json::to_vec(&json!({
            "access_token": token, "email": email
        }))
        .unwrap()
    };
    let stored = make("before", "café@example.test");
    let rotated = make("after", "café@example.test");
    let switched = make("other", "other@example.test");
    let select = || request.select(&store.vault);
    fs::write(&source, &rotated).unwrap();
    assert!(matches!(select(), Err(Error::Missing)));
    vault::save(&store.vault, id, &stored).unwrap();
    let check = |bytes: &[u8], mode| {
        let (selected, status) = select().unwrap();
        assert_eq!(selected.as_slice(), bytes);
        assert_eq!(status, mode);
        assert_eq!(
            vault::load(&store.vault, id).unwrap().unwrap().as_slice(),
            stored
        );
    };
    check(&rotated, Updated);
    for (cli, expected, mode) in [
        (stored.as_slice(), stored.as_slice(), Stored),
        (&switched, &stored, OtherAccount),
        (b"invalid", &stored, Stored),
        (&vec![b' '; vault::MAX_SIZE + 1], &stored, Stored),
    ] {
        fs::write(&source, cli).unwrap();
        check(expected, mode);
    }
    fs::remove_file(&source).unwrap();
    check(&stored, Stored);
    vault::save(&store.vault, id, &switched).unwrap();
    assert!(matches!(select(), Err(Error::Mismatch)));
    vault::save(&store.vault, id, b"{}").unwrap();
    assert!(matches!(select(), Err(Error::Credential(_))));
    fs::write(source, &rotated).unwrap();
    let (selected, mode) = select().unwrap();
    assert_eq!(selected.as_slice(), rotated);
    assert_eq!(mode, Updated);
    assert_eq!(
        vault::load(&store.vault, id).unwrap().unwrap().as_slice(),
        b"{}"
    );
}

#[test]
fn claude_account_and_cli_binding_commit_and_undo_together() {
    let store = Store::new();
    let dir = store.root.join("외부 CLI");
    let external = dir.join("settings.json");
    fs::create_dir(&dir).unwrap();
    let bad_path = store.root.join("cannot-replace-directory");
    fs::create_dir(&bad_path).unwrap();
    let state = store.execute(&store.path, json!({"load":{}}), &[]).unwrap()["settings"].clone();
    let bind = |helper: &str| json!({"bindClaude":{"directory":dir,"helper":helper,"alias":""}});
    let failed_edit = |state: &Value, edit: Value| {
        store.execute(
            &bad_path,
            json!({"edit":{"settings":state,"edit":edit,"storage":"disk"}}),
            &[],
        )
    };
    for initial in [
        json!({"keep":22}),
        json!({"keep":22,"statusLine":{"type":"command","command":"old official command","padding":3}}),
    ] {
        fs::write(&external, serde_json::to_vec(&initial).unwrap()).unwrap();
        assert!(failed_edit(&state, bind("cannot-save-account")).is_err());
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&external).unwrap()).unwrap(),
            initial
        );
        let change = store.edit(&state, bind("current")).unwrap();
        let account = &change["settings"]["accounts"][0];
        assert_eq!(account["name"], "Claude · 외부 CLI");
        let folder = store
            .root
            .join("ClaudeProfiles")
            .join(account["identity"].as_str().unwrap());
        let active = folder.join("profile.json");
        let profile: Value = serde_json::from_slice(&fs::read(&active).unwrap()).unwrap();
        let settings: Value = serde_json::from_slice(&fs::read(&external).unwrap()).unwrap();
        assert_eq!(
            settings["statusLine"]["command"],
            profile["installedCommand"]
        );
        assert!(failed_edit(&change["settings"], bind("reconnect-that-fails")).is_err());
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&active).unwrap()).unwrap(),
            profile
        );
        let remove = json!({"removeClaude":{"id":account["id"],"identity":account["identity"]}});
        assert!(failed_edit(&change["settings"], remove.clone()).is_err());
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&active).unwrap()).unwrap(),
            profile
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&external).unwrap()).unwrap(),
            settings
        );
        assert_eq!(
            store.edit(&change["settings"], remove).unwrap()["settings"]["accounts"],
            json!([])
        );
        assert!(!folder.exists());
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&external).unwrap()).unwrap(),
            initial
        );
    }
}

#[test]
fn reads_foundation_settings_and_keeps_legacy_defaults_and_date() {
    let store = Store::new();
    let default = store.execute(&store.path, json!({"load":{}}), &[]).unwrap();
    assert_eq!(
        default["settings"],
        json!({"accounts":[],"refreshSeconds":300,"scanLocalLogs":true})
    );
    assert!(!store.path.exists());
    let json = serde_json::to_string(&legacy())
        .unwrap()
        .replace("300,", "3e2,");
    for bytes in [
        json.as_bytes().to_vec(),
        [b"\xef\xbb\xbf".as_slice(), json.as_bytes()].concat(),
        json.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        json.encode_utf16().flat_map(u16::to_be_bytes).collect(),
        json.chars()
            .flat_map(|c| u32::from(c).to_le_bytes())
            .collect(),
        json.chars()
            .flat_map(|c| u32::from(c).to_be_bytes())
            .collect(),
    ] {
        fs::write(&store.path, &bytes).unwrap();
        let reply = store.execute(&store.path, json!({"load":{}}), &[]).unwrap();
        let mut expected = legacy();
        expected["accounts"][0]["id"] = json!(ACCOUNT.to_ascii_uppercase());
        assert_eq!(reply["settings"], expected);
        assert_eq!(fs::read(&store.path).unwrap(), bytes);
        store
            .execute(
                &store.path,
                json!({"save":{"settings":reply["settings"]}}),
                &[],
            )
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&store.path).unwrap()).unwrap(),
            expected
        );
    }
    let from_text = |s: &str| codec::decode(s.as_bytes());
    let first = from_text(r#"{"accounts":[],"refreshSeconds":900.0,"refreshSeconds":1,"appearance":"new","menuBarDisplayMode":"new","scanLocalLogs":true,}"#).unwrap();
    assert_eq!(first.refresh_seconds, 900);
    assert_eq!(first.appearance, Some(Appearance::System));
    assert_eq!(
        first.menu_bar_display_mode,
        Some(MenuDisplay::ProviderAndRemaining)
    );
    for (value, expected) in [
        ("-9223372036854775808", Some(i64::MIN)),
        ("9223372036854775807", Some(i64::MAX)),
        ("-9223372036854775808.0", None),
        ("9223372036854775807.0", None),
        ("-9223372036854775807.0", Some(-i64::MAX)),
        ("9007199254740993.5", Some(9_007_199_254_740_993)),
        ("9007199254740993.00001", None),
        ("9.007199254740993e15", Some(9_007_199_254_740_993)),
    ] {
        let s = format!(r#"{{"accounts":[],"refreshSeconds":{value},"scanLocalLogs":true}}"#);
        assert_eq!(
            from_text(&s).ok().map(|s| s.refresh_seconds),
            expected,
            "{value}"
        );
    }
    for bad in [
        r#"{"accounts":[],"refreshSeconds":"300","scanLocalLogs":true}"#,
        r#"{"accounts":[],"refreshSeconds":1.5,"scanLocalLogs":true}"#,
        r#"{"accounts":[,],"refreshSeconds":300,"scanLocalLogs":true}"#,
        r#"{"accounts":[],"refreshSeconds":300,"scanLocalLogs":1}"#,
        r#"{"accounts":[]}"#,
    ] {
        fs::write(&store.path, bad).unwrap();
        assert!(matches!(Settings::load(&store.path), Err(Error::Json)));
        assert_eq!(fs::read(&store.path).unwrap(), bad.as_bytes());
    }
}

#[test]
fn preferences_preserve_absent_defaults_and_write_only_effective_changes() {
    let store = Store::new();
    let mut state = legacy();
    for edit in [
        json!({"autoRefresh":{"enabled":true}}),
        json!({"appearance":{"mode":"system"}}),
        json!({"showDockIcon":{"visible":true}}),
        json!({"menuBarDisplayMode":{"mode":"providerAndRemaining"}}),
        json!({"interval":{"seconds":120}}),
        json!({"interval":{"seconds":300}}),
        json!({"menuVisibility":{"id":ACCOUNT,"visible":false}}),
    ] {
        let result = store.edit(&state, edit).unwrap();
        assert_eq!(result["changed"], false);
        assert!(!store.path.exists());
    }
    for (edit, key, expected) in [
        (
            json!({"automaticUpdates":{"enabled":true}}),
            "automaticUpdates",
            json!(true),
        ),
        (
            json!({"autoRefresh":{"enabled":false}}),
            "autoRefresh",
            json!(false),
        ),
        (
            json!({"appearance":{"mode":"dark"}}),
            "appearance",
            json!("dark"),
        ),
        (
            json!({"showDockIcon":{"visible":false}}),
            "showDockIcon",
            json!(false),
        ),
        (
            json!({"menuBarDisplayMode":{"mode":"iconOnly"}}),
            "menuBarDisplayMode",
            json!("iconOnly"),
        ),
        (
            json!({"scanLocalLogs":{"enabled":false}}),
            "scanLocalLogs",
            json!(false),
        ),
        (
            json!({"interval":{"seconds":3600}}),
            "refreshSeconds",
            json!(3600),
        ),
    ] {
        let reply = store.edit(&state, edit).unwrap();
        state = reply["settings"].clone();
        assert_eq!(reply["changed"], true);
        assert_eq!(state[key], expected);
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&store.path).unwrap()).unwrap(),
            state
        );
    }
    let before = fs::read(&store.path).unwrap();
    let edit = json!({"toggle":{"id":ACCOUNT}});
    let request = |storage| json!({"edit":{"settings":state,"edit":edit,"storage":storage}});
    let reply = store.execute(&store.path, request("memory"), &[]).unwrap();
    assert_eq!(reply["settings"]["accounts"][0]["enabled"], true);
    assert!(matches!(
        store.execute(&store.path, request("unavailable"), &[]),
        Err(Error::Unavailable)
    ));
    assert_eq!(fs::read(&store.path).unwrap(), before);
}

#[test]
fn imported_and_managed_accounts_share_identity_and_rollback_credentials_on_failure() {
    let store = Store::new();
    let source = store.root.join("login.json");
    fs::write(&source, LOGIN).unwrap();
    let imported = store
        .edit(
            &legacy(),
            json!({"importLogin":{"provider":"codex","path":source,"alias":""}}),
        )
        .unwrap();
    let state = &imported["settings"];
    let account = &state["accounts"][0];
    assert_eq!(state["accounts"].as_array().unwrap().len(), 1);
    assert_eq!(account["id"], ACCOUNT.to_ascii_uppercase());
    assert_eq!(account["name"], "별칭");
    assert_eq!(account["showInMenuBar"], false);
    assert_eq!(account["enabled"], true);
    assert_eq!(account["createdAt"], legacy()["accounts"][0]["createdAt"]);
    assert_eq!(account["appManagedLogin"], false);
    let id = Uuid::parse_str(ACCOUNT).unwrap();
    let read = || vault::load(&store.vault, id).unwrap().map(|s| s.to_vec());
    assert_eq!(read().as_deref(), Some(LOGIN));
    let managed = |state: &Value, target: &Value| {
        json!({"edit":{"settings":state,
        "edit":{"managedLogin":{"alias":"New alias","target":target}},"storage":"disk"}})
    };
    let wrong = json!({"id":ACCOUNT, "identity":"another workspace"});
    assert!(matches!(
        store.execute(&store.path, managed(state, &wrong), ROTATED),
        Err(Error::WrongAccount)
    ));
    assert!(matches!(
        store.execute(&store.root, managed(state, account), ROTATED),
        Err(Error::Io(_))
    ));
    assert_eq!(read().as_deref(), Some(LOGIN));
    let next = store
        .execute(&store.path, managed(state, account), ROTATED)
        .unwrap();
    let state = &next["settings"];
    let account = &state["accounts"][0];
    assert_eq!(account["sourcePath"], "");
    assert_eq!(account["name"], "New alias");
    assert_eq!(account["appManagedLogin"], true);
    assert_eq!(read().as_deref(), Some(ROTATED));
    let removal = json!({"remove":{"id":ACCOUNT,"deleteCredential":true}});
    let failed = json!({"edit":{"settings":state,"edit":removal,"storage":"disk"}});
    assert!(store.execute(&store.root, failed, &[]).is_err());
    assert_eq!(read().as_deref(), Some(ROTATED));
    assert_eq!(
        store.edit(state, removal).unwrap()["settings"]["accounts"],
        json!([])
    );
    assert!(read().is_none());
    // A first-time connection must also roll back its newly allocated vault entry.
    let empty = json!({"accounts":[],"refreshSeconds":300,"scanLocalLogs":true});
    assert!(
        store
            .execute(&store.root, managed(&empty, &Value::Null), LOGIN)
            .is_err()
    );
    assert_eq!(fs::read_dir(&store.vault).unwrap().count(), 0);
}

#[test]
fn claude_reconnect_uses_canonical_unicode_and_remove_clears_duplicate_ids() {
    let store = Store::new();
    let mut state = legacy();
    let row = &mut state["accounts"][0];
    row["provider"] = json!("claude");
    row["identity"] = json!("e\u{301}");
    let edit = json!({"claude":{"identity":"é","label":"Claude · profile","sourcePath":"/new","alias":""}});
    let result = store.edit(&state, edit).unwrap();
    let mut accounts = result["settings"]["accounts"].as_array().unwrap().clone();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0]["name"], "별칭");
    assert_eq!(accounts[0]["sourcePath"], "/old");
    assert!(accounts[0]["appManagedLogin"].is_null());
    assert_eq!(accounts[0]["enabled"], true);
    accounts.push(accounts[0].clone());
    state["accounts"] = json!(accounts);
    assert_eq!(
        store
            .edit(
                &state,
                json!({"remove":{"id":ACCOUNT,"deleteCredential":false}})
            )
            .unwrap()["settings"]["accounts"],
        json!([])
    );
}
