use super::*;
use serde_json::json;

#[derive(Default)]
struct Vault {
    current: Option<Secret>,
    saves: usize,
    locked: bool,
}

impl Store for Vault {
    fn load(&mut self, _: Uuid) -> Result<Option<Secret>, Error> {
        Ok(self.current.clone())
    }
    fn save(&mut self, _: Uuid, data: &[u8]) -> Result<(), Error> {
        if self.locked {
            return Err(Error::Storage);
        }
        self.current = Some(Secret::new(data.to_vec()));
        self.saves += 1;
        Ok(())
    }
}

const KEY: &str = "codex:account:é:user:u";
fn credential(expiry: Option<i64>, last: Option<&str>, token: &str) -> Secret {
    Secret::new(
        serde_json::to_vec(&json!({
            "tokens":{"account_id":"é","access_token":token}, "chatgpt_user_id":"u",
            "expires_at":expiry, "last_refresh":last
        }))
        .unwrap(),
    )
}

#[test]
fn expiry_refresh_age_and_rejected_bytes_follow_the_original_boundaries() {
    let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
    let id = Uuid::new_v4();
    for platform in [Platform::Mac, Platform::Windows] {
        let sessions = Sessions::new(platform);
        for (until, age, rejected, rotates) in [
            (Some(301), None, false, false),
            (Some(300), None, false, true),
            (Some(301), Some(9999), true, true),
            (None, Some(3599), false, false),
            (None, Some(3600), false, true),
            (None, None, false, true),
        ] {
            let last = age.map(|n| (now - TimeDelta::seconds(n)).to_rfc3339());
            let data = credential(
                until.map(|n| now.timestamp() + n),
                last.as_deref(),
                "fixture",
            );
            let mut vault = Vault {
                current: Some(data.clone()),
                ..Default::default()
            };
            let rejected = if rejected {
                &data[..]
            } else {
                b"another login"
            };
            let decision = sessions
                .begin(&mut vault, id, KEY, Some(rejected), now)
                .unwrap();
            assert_eq!(matches!(decision, Decision::Rotate(..)), rotates);
            sessions.invalidate(id);
        }
        let mut vault = Vault {
            current: Some(credential(Some(now.timestamp() + 600), None, "fixture")),
            ..Default::default()
        };
        let result = sessions.begin(&mut vault, id, "codex:account:e\u{301}:user:u", None, now);
        assert_eq!(
            matches!(result, Ok(Decision::Ready(_))),
            platform == Platform::Mac
        );
    }
}

#[test]
fn superseded_rotations_and_failed_writes_do_not_replace_a_reconnected_login() {
    let now = Utc::now();
    let id = Uuid::new_v4();
    let fresh = credential(Some(now.timestamp() + 3600), None, "reconnected");
    let original = credential(None, None, "original");
    let sessions = Sessions::new(Platform::Mac);
    let mut vault = Vault {
        current: Some(original.clone()),
        ..Default::default()
    };
    let Decision::Rotate(old, _) = sessions.begin(&mut vault, id, KEY, None, now).unwrap() else {
        panic!("expected rotation")
    };
    assert!(
        matches!(sessions.begin(&mut vault, id, "", None, now), Ok(Decision::Join(ticket)) if ticket == old)
    );
    assert_eq!(sessions.invalidate(id), Some(old));
    let Decision::Rotate(new, _) = sessions.begin(&mut vault, id, KEY, None, now).unwrap() else {
        panic!("expected new rotation")
    };
    sessions.revoke(id, old);
    sessions.end(id, old);
    assert_eq!(
        sessions.complete(&mut vault, id, old, &fresh),
        Err(Error::Superseded)
    );
    vault.locked = true;
    assert_eq!(
        sessions.complete(&mut vault, id, new, &fresh),
        Err(Error::Storage)
    );
    sessions.end(id, new);
    assert_eq!(sessions.flush(&mut vault), Err(Error::Storage));
    vault.current = Some(credential(Some(now.timestamp() + 900), None, "different"));
    sessions.flush(&mut vault).unwrap();
    assert_eq!(vault.saves, 0);
    assert!(
        matches!(sessions.begin(&mut vault, id, KEY, Some(&original), now), Ok(Decision::Ready(value)) if Some(&value) == vault.current.as_ref())
    );
    vault.current = Some(original);
    sessions.flush(&mut vault).unwrap();
    assert_eq!(vault.saves, 0);
}

#[test]
fn windows_waiters_keep_per_account_order_through_invalidation_and_are_released() {
    let sessions = Sessions::new(Platform::Windows);
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    assert!(sessions.enter_windows(a, 1));
    assert!(!sessions.enter_windows(a, 2));
    assert!(sessions.enter_windows(b, 3));
    sessions.invalidate(a);
    assert!(!sessions.enter_windows(a, 4));
    assert_eq!(sessions.leave_windows(a), Some(2));
    assert_eq!(sessions.leave_windows(b), None);
    assert_eq!(sessions.leave_windows(a), Some(4));
    assert_eq!(sessions.leave_windows(a), None);
    assert!(sessions.state.borrow().windows_queue.is_empty());
}

#[test]
fn login_metadata_obeys_each_platforms_json_depth_limit() {
    let id = Uuid::new_v4();
    let now = Utc::now();
    for (platform, maximum) in [(Platform::Windows, 64), (Platform::Mac, 512)] {
        for depth in [maximum, maximum + 1] {
            let metadata = format!("{}1{}", "[".repeat(depth - 1), "]".repeat(depth - 1));
            let source = format!(
                r#"{{"account_id":"é","chatgpt_user_id":"u","access_token":"fixture","expires_at":253402300000,"metadata":{metadata}}}"#
            );
            let mut vault = Vault {
                current: Some(Secret::new(source.into_bytes())),
                ..Default::default()
            };
            let result = Sessions::new(platform).begin(&mut vault, id, KEY, None, now);
            assert_eq!(matches!(result, Ok(Decision::Ready(_))), depth == maximum);
        }
    }
}
