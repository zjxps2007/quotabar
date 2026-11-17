use eframe::egui;
use quotabar::dashboard::Endpoint;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, mpsc};
use uuid::Uuid;
use zeroize::Zeroizing;

pub enum Event {
    State(Value),
    Show(String),
    Close,
    Capture {
        route: String,
        width: f32,
        height: f32,
        path: String,
    },
    Reply(String, Option<String>),
}
type Wake = Arc<Mutex<Option<egui::Context>>>;
pub struct Peer {
    output: mpsc::Sender<Zeroizing<Vec<u8>>>,
    pub initial: Option<Event>,
}
impl Peer {
    pub fn start(args: &[String]) -> Option<(Self, mpsc::Receiver<Event>, Wake)> {
        #[cfg(windows)]
        let endpoint = {
            let handle = |flag| {
                args.windows(2)
                    .find(|p| p[0] == flag)?
                    .get(1)?
                    .parse::<usize>()
                    .ok()
            };
            // The launcher gives this process sole ownership of these inherited handles.
            unsafe { Endpoint::inherited(handle("--in")?, handle("--out")?).ok()? }
        };
        #[cfg(target_os = "macos")]
        let endpoint = {
            if !args.iter().any(|s| s == "--dashboard-helper")
                || ![libc::STDIN_FILENO, libc::STDOUT_FILENO]
                    .into_iter()
                    .all(|fd| {
                        let mut stat = std::mem::MaybeUninit::uninit();
                        unsafe {
                            libc::fstat(fd, stat.as_mut_ptr()) == 0
                                && stat.assume_init().st_mode & libc::S_IFMT == libc::S_IFIFO
                        }
                    })
            {
                return None;
            }
            Endpoint::standard().ok()?
        };
        let endpoint = Arc::new(endpoint);
        let reader = endpoint.clone();
        let (events, inbox) = mpsc::channel();
        let wake: Wake = Default::default();
        let reader_wake = wake.clone();
        std::thread::spawn(move || {
            let publish = |event| {
                let sent = events.send(event).is_ok();
                if let Some(ctx) = reader_wake.lock().unwrap().as_ref() {
                    ctx.request_repaint();
                }
                sent
            };
            while let Ok(Some(bytes)) = reader.receive() {
                let Ok(value) = serde_json::from_slice(&bytes) else {
                    break;
                };
                match decode(value, cfg!(windows)) {
                    Some(Event::Close) => break,
                    Some(e) => {
                        if !publish(e) {
                            return;
                        }
                    }
                    _ => {}
                }
            }
            publish(Event::Close);
        });
        let (output, outgoing) = mpsc::channel::<Zeroizing<Vec<u8>>>();
        std::thread::spawn(move || {
            for bytes in outgoing {
                if endpoint.send(&bytes).is_err() {
                    break;
                }
            }
        });
        let peer = Self {
            output,
            initial: None,
        };
        peer.action("ready", Value::Null);
        Some((peer, inbox, wake))
    }
    pub fn action(&self, name: &str, payload: Value) -> String {
        let id = Uuid::new_v4().to_string();
        let mut value = encode(name, payload, &id, cfg!(windows));
        if let Ok(bytes) = serde_json::to_vec(&value) {
            let _ = self.output.send(Zeroizing::new(bytes));
        }
        if let Some(Value::String(secret)) = value.pointer_mut("/codexLogin/data") {
            zeroize::Zeroize::zeroize(secret);
        }
        id
    }
}

fn decode(mut v: Value, windows: bool) -> Option<Event> {
    if windows {
        if v["version"] != 1 {
            return Some(Event::Close);
        }
        match v["type"].as_str()? {
            "state" => Some(Event::State(v["payload"].take())),
            "show" => Some(Event::Show(v["payload"].as_str()?.into())),
            "close" => Some(Event::Close),
            "capture" => {
                let p = &v["payload"];
                Some(Event::Capture {
                    route: p["route"].as_str()?.into(),
                    path: p["path"].as_str()?.into(),
                    width: p["width"].as_f64().unwrap_or(1120.) as f32,
                    height: p["height"].as_f64().unwrap_or(810.) as f32,
                })
            }
            "result" => Some(Event::Reply(
                v["requestId"].as_str()?.into(),
                v["payload"]["error"].as_str().map(str::to_owned),
            )),
            _ => None,
        }
    } else if v.get("state").is_some() {
        Some(Event::State(v["state"]["_0"].take()))
    } else if let Some(s) = v.get("show") {
        Some(Event::Show(
            s["route"].as_str().unwrap_or("dashboard").into(),
        ))
    } else if let Some(p) = v.get("capture") {
        Some(Event::Capture {
            route: p["route"].as_str()?.into(),
            path: p["path"].as_str()?.into(),
            width: p["width"].as_f64().unwrap_or(1120.) as f32,
            height: p["height"].as_f64().unwrap_or(810.) as f32,
        })
    } else if let Some(r) = v.get("reply") {
        Some(Event::Reply(
            r["id"].as_str()?.into(),
            r["error"].as_str().map(str::to_owned),
        ))
    } else {
        v.get("close").map(|_| Event::Close)
    }
}

fn encode(action: &str, payload: Value, id: &str, windows: bool) -> Value {
    if windows {
        return json!({"version":1,"type":action,"requestId":id,"payload":if payload.is_null(){json!(true)}else{payload}});
    }
    let key = match action {
        "refresh" => "refreshAll",
        "refresh-account" => "refresh",
        "scan" => "refreshAccounts",
        "startup-status" => "refreshLaunchAtLogin",
        "startup-settings" => "openLoginItemsSettings",
        "startup" => "launchAtLogin",
        "check-app-update" => "checkAppUpdate",
        "install-app-update" => "installAppUpdate",
        k => k,
    };
    let body = match key {
        "connect" | "codexLogin" => {
            let mut p = payload;
            p["id"] = json!(id);
            p
        }
        "menuBarVisibility" => json!({"_0":payload[0],"_1":payload[1]}),
        _ if payload.is_null() => json!({}),
        _ => json!({"_0":payload}),
    };
    Value::Object([(key.to_owned(), body)].into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_monitor_protocols_keep_ids_and_argument_shapes() {
        let id = Uuid::new_v4().to_string();
        assert_eq!(
            encode("refresh-account", json!(id), "request", false),
            json!({"refresh":{"_0":id}})
        );
        assert_eq!(
            encode("ready", Value::Null, "request", false),
            json!({"ready":{}})
        );
        assert_eq!(
            encode("menuBarVisibility", json!([id, false]), "request", false),
            json!({"menuBarVisibility":{"_0":id,"_1":false}})
        );
        assert_eq!(
            encode("connect", json!({"path":"한글"}), &id, false)["connect"]["id"],
            id
        );
        assert_eq!(
            encode("route", json!("accounts"), &id, true)["payload"],
            "accounts"
        );
        assert!(matches!(
            decode(json!({"version":2,"type":"show"}), true),
            Some(Event::Close)
        ));
        assert!(matches!(decode(json!({"show":{}}),false),Some(Event::Show(s)) if s=="dashboard"));
    }
}
