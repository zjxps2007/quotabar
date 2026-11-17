use super::*;
use crate::{
    provider_http::{Reply, TransportError},
    vault,
};
use serde_json::json;
use std::{
    fs, thread,
    time::{Duration, Instant},
};

const IDENTITY: &str = "codex:account:workspace:user:user";
const DEADLINE: Duration = Duration::from_secs(8);

struct Fixture {
    id: Uuid,
    accounts: ManagedAccounts,
}

impl Fixture {
    fn new(platform: Platform) -> Self {
        let id = Uuid::new_v4();
        let directory = std::env::temp_dir().join(format!("quotabar-managed-{id}"));
        let accounts = ManagedAccounts::new(directory, platform);
        let fixture = Self { id, accounts };
        fixture.save("initial", false);
        fixture
    }
    fn save(&self, token: &str, fresh: bool) {
        let bytes = serde_json::to_vec(&json!({
            "tokens": {"account_id":"workspace", "access_token":token, "refresh_token":"synthetic"},
            "chatgpt_user_id":"user", "expires_at":Utc::now().timestamp() + if fresh { 7200 } else { -60 }
        })).unwrap();
        vault::save(&self.accounts.directory, self.id, &bytes).unwrap();
    }
    fn access(&self) -> String {
        let bytes = vault::load(&self.accounts.directory, self.id)
            .unwrap()
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        json["tokens"]["access_token"].as_str().unwrap().into()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        vault::delete(&self.accounts.directory, self.id).unwrap();
        fs::remove_dir(&self.accounts.directory).unwrap();
    }
}

#[test]
fn sleep_cancels_quota_but_persists_an_issued_rotation_before_shutdown() {
    for platform in [Platform::Mac, Platform::Windows] {
        let f = Fixture::new(platform);
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let outcome = f
            .accounts
            .fetch_with(f.id, IDENTITY, &cancel, |query, flag, now| {
                provider_http::drive(query, flag, now, |_, oauth, _, transport_cancel| {
                    assert!(oauth);
                    calls += 1;
                    cancel.store(true, Ordering::Release);
                    assert!(!transport_cancel.load(Ordering::Acquire));
                    Ok(Reply {
                        status: 200,
                        body: Secret::new(
                            br#"{"access_token":"issued","expires_in":7200}"#.to_vec(),
                        ),
                    })
                })
            });
        assert!(matches!(
            outcome,
            Err(FetchError::Provider(Failure::Transport(
                TransportError::Cancelled
            )))
        ));
        assert_eq!(calls, 1);
        f.accounts.flush().unwrap();
        assert_eq!(f.access(), "issued");
    }
}

#[test]
fn overlapping_callers_share_mac_failure_but_windows_retries_in_queue_order() {
    for platform in [Platform::Mac, Platform::Windows] {
        let f = Fixture::new(platform);
        let (started, begun) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        thread::scope(|scope| {
            let leader = scope.spawn(|| {
                f.accounts.credential(f.id, IDENTITY, None, move |_, _| {
                    started.send(()).unwrap();
                    gate.recv_timeout(DEADLINE).unwrap();
                    Err(Failure::Transport(TransportError::Network(111)))
                })
            });
            begun.recv_timeout(DEADLINE).unwrap();
            let follower = scope.spawn(|| {
                f.accounts.credential(f.id, IDENTITY, None, |_, _| {
                    Err(Failure::Transport(TransportError::Network(222)))
                })
            });
            let deadline = Instant::now() + DEADLINE;
            loop {
                let state = f.accounts.state.lock().unwrap();
                let waiting = match platform {
                    Platform::Mac => state.rotations.values().any(|r| Arc::strong_count(r) == 3),
                    Platform::Windows => state
                        .sessions
                        .state
                        .borrow()
                        .windows_queue
                        .get(&f.id)
                        .is_some_and(|q| !q.is_empty()),
                };
                if waiting {
                    break;
                }
                drop(state);
                assert!(
                    Instant::now() < deadline,
                    "second call did not reach the in-flight rotation"
                );
                thread::yield_now();
            }
            release.send(()).unwrap();
            let code = |r| match r {
                Err(FetchError::Provider(Failure::Transport(TransportError::Network(code)))) => {
                    code
                }
                _ => panic!("missing exchange failure"),
            };
            assert_eq!(code(leader.join().unwrap()), 111);
            assert_eq!(
                code(follower.join().unwrap()),
                if platform == Platform::Mac { 111 } else { 222 }
            );
        });
    }
}

#[test]
fn a_reconnected_account_outlives_an_old_inflight_exchange() {
    for platform in [Platform::Mac, Platform::Windows] {
        let f = Fixture::new(platform);
        let (started, begun) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        thread::scope(|scope| {
            let job = scope.spawn(|| {
                f.accounts.credential(f.id, IDENTITY, None, move |_, flag| {
                    started.send(()).unwrap();
                    gate.recv_timeout(DEADLINE).unwrap();
                    assert_eq!(flag.load(Ordering::Acquire), platform == Platform::Mac);
                    Ok(b"superseded response".to_vec())
                })
            });
            begun.recv_timeout(DEADLINE).unwrap();
            f.accounts.invalidate(f.id);
            f.save("reconnected", true);
            release.send(()).unwrap();
            assert!(matches!(
                job.join().unwrap(),
                Err(FetchError::Session(Error::Superseded))
            ));
        });
        f.accounts.flush().unwrap();
        assert_eq!(f.access(), "reconnected");
    }
}
