//! Differential test for the normal Windows entry point. Each monitor launches this
//! executable as its Dashboard and receives real commands over its private pipes.
#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() -> std::io::Result<()> {
    test::run()
}

#[cfg(windows)]
mod test {
    use chrono::Utc;
    use quotabar::{dashboard::Endpoint, files, vault};
    use serde_json::{Value, json};
    use std::{
        env, fs, io,
        path::{Path, PathBuf},
        process::Command,
        thread,
        time::{Duration, Instant},
    };
    use uuid::Uuid;

    const ID: &str = "e4ba1c3d-e141-4e3d-a567-f770593a311b";
    fn require(ok: bool, why: &str) -> io::Result<()> {
        if ok {
            Ok(())
        } else {
            Err(io::Error::other(why.to_owned()))
        }
    }
    fn read(path: &Path) -> io::Result<Value> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }
    fn write(path: &Path, data: Value) -> io::Result<()> {
        fs::write(path, serde_json::to_vec(&data)?)
    }
    fn account(state: &Value, id: &str) -> Option<Value> {
        state["settings"]["accounts"]
            .as_array()?
            .iter()
            .find(|a| a["id"] == id)
            .cloned()
    }
    struct Peer {
        pipe: Endpoint,
        state: Value,
        log: Vec<Value>,
        work: PathBuf,
    }
    impl Peer {
        fn send(&self, kind: &str, payload: Value, id: Value) -> io::Result<()> {
            self.pipe.send(&serde_json::to_vec(
                &json!({"version":1,"type":kind,"requestId":id,"payload":payload}),
            )?)
        }
        fn until(&mut self, condition: impl Fn(&Value) -> bool) -> io::Result<Value> {
            loop {
                let bytes = self
                    .pipe
                    .receive()?
                    .ok_or_else(|| io::Error::other("Monitor closed the pipe early"))?;
                let f: Value = serde_json::from_slice(&bytes)?;
                if f["type"] == "state" {
                    require(
                        !String::from_utf8_lossy(&bytes).contains("private-synthetic-access"),
                        "Secret leaked into a dashboard state",
                    )?;
                    self.state = f["payload"].clone();
                }
                if condition(&f) {
                    return Ok(f);
                }
            }
        }
        fn command(&mut self, kind: &str, payload: Value) -> io::Result<Value> {
            let id = Uuid::new_v4().to_string();
            self.send(kind, payload, json!(id))?;
            Ok(self.until(|f| f["type"] == "result" && f["requestId"] == id)?["payload"].clone())
        }
        fn record(&mut self, name: &str, got: Value, expected: Value) -> io::Result<()> {
            self.log.push(json!({"check":name,"actual":got}));
            write(&self.work.join("progress.json"), json!(self.log))?;
            require(
                got == expected,
                &format!("{name}: expected {expected}, got {got}"),
            )
        }
        fn success(&mut self, kind: &str, payload: Value, name: &str) -> io::Result<()> {
            let reply = self.command(kind, payload)?;
            self.record(name, reply, json!({"success":true}))
        }
        fn rejected(
            &mut self,
            kind: &str,
            payload: Value,
            message: &str,
            name: &str,
        ) -> io::Result<()> {
            let reply = self.command(kind, payload)?;
            self.record(name, reply, json!({"success":false,"error":message}))
        }
        fn exercise(&mut self, phase: &str) -> io::Result<()> {
            self.send("ready", Value::Null, Value::Null)?;
            let shown = self.until(|f| f["type"] == "show")?;
            self.record(
                "state before first show",
                json!([self.state["demo"], shown["payload"]]),
                json!([false, "dashboard"]),
            )?;
            let claude = self.work.join("claude");
            let file = claude.join("settings.json");
            let data = self.work.join("data");
            self.record(
                "private application data",
                json!(Path::new(self.state["dataDirectory"].as_str().unwrap_or_default()) == data),
                json!(true),
            )?;
            let defaults = &self.state["defaultPaths"];
            self.record(
                "isolated CLI defaults",
                json!([
                    Path::new(defaults["codex"].as_str().unwrap_or_default())
                        == self.work.join("codex").join("auth.json"),
                    Path::new(defaults["claude"].as_str().unwrap_or_default()) == claude
                ]),
                json!([true, true]),
            )?;
            if phase == "restart" {
                let s = &self.state;
                let saved = account(s, ID)
                    .ok_or_else(|| io::Error::other("Persisted account is missing"))?;
                self.record(
                    "restarted settings, account and last quota",
                    json!([
                        s["settings"]["appearance"],
                        s["settings"]["refreshSeconds"],
                        saved["name"],
                        saved["enabled"],
                        s["snapshots"][ID]["lowest"].as_f64()
                    ]),
                    json!(["dark", 900, "보관 계정", false, 42.0]),
                )?;
                let accounts = self.state["settings"]["accounts"].as_array().unwrap();
                let id = accounts.iter().find(|a| a["provider"] == "claude").unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                self.success("remove", json!(id), "disconnect Claude")?;
                self.record("prior status line and unrelated setting restored",read(&file)?,json!({"statusLine":{"type":"command","command":"echo previous"},"theme":"fixture"}))?;
                self.success("remove", json!(ID), "remove imported account")?;
                self.record(
                    "vault credential removed",
                    json!(
                        vault::load(&data.join("Credentials"), Uuid::parse_str(ID).unwrap())?
                            .is_none()
                    ),
                    json!(true),
                )?;
                self.record(
                    "empty persisted cache",
                    read(&data.join("snapshots.json"))?,
                    json!({}),
                )?;
                self.record(
                    "no accounts after disconnect",
                    self.state["settings"]["accounts"].clone(),
                    json!([]),
                )?;
            } else {
                self.record(
                    "existing snapshot on first launch",
                    json!(self.state["snapshots"][ID]["lowest"].as_f64()),
                    json!(42.0),
                )?;
                let p = json!({"theme":"dark","seconds":900,"scan":true,"taskbar":false,"trayDisplay":"icon","autoRefresh":false,"automaticUpdates":false});
                self.success("preferences", p, "change preferences")?;
                let bad = self.work.join("invalid.json");
                self.rejected(
                    "import",
                    json!({"provider":"codex","path":bad}),
                    "JSON 파일 형식을 확인하세요. 원본 파일은 보존했습니다.",
                    "reject malformed credential",
                )?;
                self.rejected(
                    "edit",
                    json!({"id":ID,"name":" ","enabled":false,"tray":true}),
                    "이름은 1~100자로 입력하세요.",
                    "reject blank name",
                )?;
                self.success(
                    "edit",
                    json!({"id":ID,"name":"  보관 계정  ","enabled":false,"tray":false}),
                    "rename and hide account",
                )?;
                self.success(
                    "import",
                    json!({"provider":"Codex","path":self.work.join("auth.json")}),
                    "reconnect disabled imported account",
                )?;
                let a = account(&self.state, ID).unwrap();
                self.record(
                    "identity, disabled state and user name survive reconnect",
                    json!([
                        a["name"],
                        a["enabled"],
                        a["showInTray"],
                        a["appManagedLogin"],
                        a["identity"]
                    ]),
                    json!([
                        "보관 계정",
                        false,
                        false,
                        false,
                        "codex:account:workspace:user:user"
                    ]),
                )?;
                let guid = Uuid::parse_str(ID).unwrap();
                let bytes = vault::load(&data.join("Credentials"), guid)?
                    .ok_or_else(|| io::Error::other("Missing DPAPI fixture"))?;
                self.record(
                    "DPAPI credential matches source",
                    json!(*bytes == fs::read(self.work.join("auth.json"))?),
                    json!(true),
                )?;
                let auth = self.work.join("auth.json");
                let original = fs::read(&auth)?;
                let mut other: Value = serde_json::from_slice(&original)?;
                other["tokens"]["account_id"] = json!("different-workspace");
                write(&auth, other)?;
                self.success(
                    "edit",
                    json!({"id":ID,"name":"보관 계정","enabled":true,"tray":false}),
                    "enable account for a manual refresh",
                )?;
                self.send("refresh-account", json!(ID), Value::Null)?;
                self.until(|f| {
                    f["type"] == "state"
                        && f["payload"]["errors"][ID].is_string()
                        && f["payload"]["loading"]
                            .as_array()
                            .is_some_and(Vec::is_empty)
                })?;
                self.record("rejected switched workspace keeps cached quota",json!([self.state["errors"][ID],self.state["snapshots"][ID]["lowest"].as_f64()]),json!(["CLI에 다른 계정이 로그인되어 있습니다. 기존 계정과 섞지 않으려면 다시 연결하세요.",42.0]))?;
                self.success(
                    "edit",
                    json!({"id":ID,"name":"보관 계정","enabled":false,"tray":false}),
                    "disable account after refresh",
                )?;
                fs::write(auth, original)?;
                self.rejected(
                    "claude",
                    json!({"path":claude,"consent":false}),
                    "기존 status line을 보존해 연결하는 동의를 선택하세요.",
                    "reject unconsented Claude change",
                )?;
                self.success(
                    "claude",
                    json!({"path":claude,"consent":true}),
                    "connect Claude and fetch local quota",
                )?;
                let accounts = self.state["settings"]["accounts"].as_array().unwrap();
                let id = accounts.last().unwrap()["id"].as_str().unwrap().to_owned();
                let snap = &self.state["snapshots"][&id];
                self.record(
                    "Claude waiting snapshot",
                    json!([snap["state"], snap["windows"], snap["lowest"]]),
                    json!(["waiting", [], null]),
                )?;
                self.record(
                    "Claude wrapper installed",
                    json!(
                        read(&file)?["statusLine"]["command"]
                            .as_str()
                            .unwrap_or_default()
                            .contains("--claude-statusline")
                    ),
                    json!(true),
                )?;
                let profile_id = quotabar::claude::connection::profile_id(
                    &quotabar::claude::connection::canonical(&claude)?,
                );
                let profile = read(
                    &data
                        .join("ClaudeProfiles")
                        .join(&profile_id)
                        .join("profile.json"),
                )?;
                let mut helper = Command::new(
                    self.work
                        .join("program")
                        .join("StatusLine")
                        .join("QuotaBar.StatusLine.exe"),
                )
                .args([
                    "--claude-statusline",
                    &profile_id,
                    profile["generation"].as_str().unwrap(),
                ])
                .arg(&data)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()?;
                let sample = json!({"session_id":Uuid::new_v4().to_string(),"rate_limits":{"five_hour":{"used_percentage":25,"resets_at":Utc::now().timestamp()+3600}}});
                serde_json::to_writer(helper.stdin.take().unwrap(), &sample)?;
                let output = helper.wait_with_output()?;
                self.record(
                    "packaged callback forwards original output",
                    json!(
                        output.status.success()
                            && String::from_utf8_lossy(&output.stdout).trim() == "previous"
                    ),
                    json!(true),
                )?;
                self.send("refresh-account", json!(id), Value::Null)?;
                self.until(|f| {
                    f["type"] == "state" && f["payload"]["snapshots"][&id]["state"] == "snapshot"
                })?;
                self.record(
                    "manual refresh reads real Claude callback",
                    json!([
                        self.state["snapshots"][&id]["windows"][0]["title"],
                        self.state["snapshots"][&id]["lowest"].as_f64()
                    ]),
                    json!(["5시간", 75.0]),
                )?;
                self.send("visibility", json!(true), Value::Null)?;
                self.until(|f| {
                    f["type"] == "state"
                        && f["payload"]["activity"]["scannedFiles"] == 1
                        && f["payload"]["scanning"] == false
                })?;
                let report = &self.state["activity"];
                let first = &report["days"][0];
                self.record(
                    "local activity scan",
                    json!([first["model"], first["counts"], report["warnings"]]),
                    json!(["fixture-model",{"input":130,"cached":30,"output":10,"total":140},[]]),
                )?;
                let login = Uuid::new_v4().to_string();
                self.send("login", Value::Null, json!(login))?;
                self.until(|f| f["type"] == "state" && f["payload"]["loggingIn"] == true)?;
                let marker = self.work.join("cli-started");
                let deadline = Instant::now() + Duration::from_secs(5);
                while !marker.is_file() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(10));
                }
                require(marker.is_file(), "Isolated Codex CLI did not start")?;
                self.record(
                    "Codex login uses a private home",
                    json!(
                        Path::new(fs::read_to_string(marker)?.trim())
                            .starts_with(data.join("Login"))
                    ),
                    json!(true),
                )?;
                let cancel = Uuid::new_v4().to_string();
                self.send("cancel-login", Value::Null, json!(cancel))?;
                let mut results = serde_json::Map::new();
                while results.len() < 2 {
                    let f = self.until(|f| {
                        f["type"] == "result"
                            && (f["requestId"] == cancel || f["requestId"] == login)
                    })?;
                    let key = if f["requestId"] == cancel {
                        "cancel"
                    } else {
                        "login"
                    };
                    results.insert(key.into(), f["payload"].clone());
                }
                self.record("cancelled login results",Value::Object(results),json!({"cancel":{"success":true},"login":{"success":false,"error":"작업을 취소했습니다."}}))?;
                self.record(
                    "cancelled login released private home",
                    json!(fs::read_dir(data.join("Login"))?.next().is_none()),
                    json!(true),
                )?;
                fs::write(self.work.join("complete-login"), b"")?;
                self.success("login", Value::Null, "complete isolated Codex login")?;
                let connected = account(&self.state, ID).unwrap();
                self.record(
                    "managed login reconnects existing account",
                    json!([
                        connected["name"],
                        connected["enabled"],
                        connected["appManagedLogin"],
                        connected["sourcePath"]
                    ]),
                    json!(["보관 계정", false, true, ""]),
                )?;
                self.success(
                    "import",
                    json!({"provider":"codex","path":self.work.join("auth.json")}),
                    "switch managed login back to imported credentials",
                )?;
                self.success(
                    "edit",
                    json!({"id":id,"name":"로컬 Claude","enabled":false,"tray":false}),
                    "disable Claude before restart",
                )?;
            }
            Ok(())
        }
    }

    fn fixture(input: &str, output: &str) -> io::Result<()> {
        let handle = |s: &str| s.parse::<usize>().map_err(io::Error::other);
        let pipe = unsafe { Endpoint::inherited(handle(input)?, handle(output)?)? };
        let work = PathBuf::from(
            env::var_os("QUOTABAR_RESIDENT_CONTRACT")
                .ok_or_else(|| io::Error::other("Missing fixture directory"))?,
        );
        let phase = env::var("QUOTABAR_CONTRACT_PHASE").map_err(io::Error::other)?;
        let mut p = Peer {
            pipe,
            work,
            state: Value::Null,
            log: Vec::new(),
        };
        let result = p.exercise(&phase);
        write(
            &p.work.join(format!("{phase}.json")),
            json!({"checks":p.log,"error":result.as_ref().err().map(ToString::to_string)}),
        )?;
        p.send("quit", Value::Null, Value::Null)?;
        result
    }

    fn compare_one(source: &Path, base: &Path, workspace: &Path) -> io::Result<Value> {
        let app = workspace.join("program");
        let data = workspace.join("data");
        let claude = workspace.join("claude");
        for dir in [
            &app,
            &data,
            &claude,
            &workspace.join("cli"),
            &app.join("StatusLine"),
            &app.join("Dashboard"),
            &claude.join("projects/project"),
            &workspace.join("codex"),
        ] {
            files::ensure_private_directory(dir)?;
        }
        for entry in fs::read_dir(source)? {
            let e = entry?;
            if e.file_type()?.is_file() {
                fs::copy(e.path(), app.join(e.file_name()))?;
            }
        }
        let me = env::current_exe()?;
        fs::copy(&me, app.join("Dashboard/QuotaBar.Dashboard.exe"))?;
        fs::copy(&me, workspace.join("cli/codex.exe"))?;
        // Use the same packaged callback for both monitors to check the registered
        // profile formats, pass-through output and snapshots consumed on refresh.
        fs::copy(
            base.join("StatusLine/QuotaBar.StatusLine.exe"),
            app.join("StatusLine/QuotaBar.StatusLine.exe"),
        )?;
        write(
            &workspace.join("auth.json"),
            json!({"tokens":{"access_token":"private-synthetic-access","refresh_token":"synthetic-refresh","account_id":"workspace"},"chatgpt_user_id":"user"}),
        )?;
        fs::write(workspace.join("invalid.json"), b"{broken")?;
        write(
            &claude.join("settings.json"),
            json!({"statusLine":{"type":"command","command":"echo previous"},"theme":"fixture"}),
        )?;
        let day = json!({"type":"assistant","timestamp":Utc::now(),"message":{"id":"msg","model":"fixture-model","usage":{"input_tokens":100,"cache_read_input_tokens":30,"output_tokens":10}}});
        fs::write(
            claude.join("projects/project/fixture.jsonl"),
            format!("{day}\n"),
        )?;
        let settings = json!({"schemaVersion":1,"automaticUpdates":false,"autoRefresh":false,"scanLocalLogs":false,
            "accounts":[{"id":ID,"name":"seed","identity":"codex:account:workspace:user:user","sourcePath":"","provider":"codex","enabled":false,"showInTray":true}]});
        write(&data.join("settings.json"), settings)?;
        write(
            &data.join("snapshots.json"),
            json!({(ID):{"fetchedAt":"2025-01-01T00:00:00+00:00","windows":[{"title":"5시간","remaining":42}]}}),
        )?;
        let path = env::join_paths(
            std::iter::once(workspace.join("cli"))
                .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
        )
        .map_err(io::Error::other)?;
        let mut phases = Vec::new();
        for phase in ["initial", "restart"] {
            let mut launch = Command::new(app.join("QuotaBar.Monitor.exe"));
            for key in quotabar::claude::connection::API_KEYS {
                launch.env_remove(key);
            }
            let mut child = launch
                .arg("--data-dir")
                .arg(&data)
                .env("QUOTABAR_RESIDENT_CONTRACT", workspace)
                .env("QUOTABAR_CONTRACT_PHASE", phase)
                .env("PATH", &path)
                .env("CODEX_HOME", workspace.join("codex"))
                .env("CLAUDE_CONFIG_DIR", &claude)
                .current_dir(&app)
                .spawn()?;
            let end = Instant::now() + Duration::from_secs(28);
            while child.try_wait()?.is_none() {
                if Instant::now() >= end {
                    let _ = child.kill();
                    let _ = child.wait();
                    let progress =
                        fs::read_to_string(workspace.join("progress.json")).unwrap_or_default();
                    return Err(io::Error::other(format!(
                        "{phase} timed out; last actions: {progress}"
                    )));
                }
                thread::sleep(Duration::from_millis(30));
            }
            let data = read(&workspace.join(format!("{phase}.json")))?;
            require(
                data["error"].is_null(),
                &format!("{phase}: {}", data["error"]),
            )?;
            phases.push(data);
        }
        Ok(json!(phases))
    }

    pub(super) fn run() -> io::Result<()> {
        let args: Vec<_> = env::args().skip(1).collect();
        if let [in_arg, input, out_arg, output] = args.as_slice()
            && in_arg == "--in"
            && out_arg == "--out"
        {
            return fixture(input, output);
        }
        if args.last().is_some_and(|v| v == "login")
            && let Some(work) = env::var_os("QUOTABAR_RESIDENT_CONTRACT")
        {
            let home = env::var("CODEX_HOME").map_err(io::Error::other)?;
            fs::write(Path::new(&work).join("cli-started"), &home)?;
            if Path::new(&work).join("complete-login").is_file() {
                fs::copy(
                    Path::new(&work).join("auth.json"),
                    Path::new(&home).join("auth.json"),
                )?;
                return Ok(());
            }
            thread::sleep(Duration::from_secs(20));
            return Ok(());
        }
        require(
            args.len() == 3,
            "Usage: windows-resident-contract <original folder> <candidate folder> <report.json>",
        )?;
        let root = env::temp_dir().join(format!("QuotaBar-Resident-{}", Uuid::new_v4()));
        files::ensure_private_directory(&root)?;
        let result = (|| {
            let mut observations = Vec::new();
            for (i, path) in args[..2].iter().enumerate() {
                let dest = root.join(i.to_string());
                files::ensure_private_directory(&dest)?;
                observations.push(compare_one(Path::new(path), Path::new(&args[1]), &dest)?);
            }
            require(
                observations[0] == observations[1],
                "Resident observations disagree",
            )?;
            let count: usize = observations[0]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["checks"].as_array().unwrap().len())
                .sum();
            write(
                Path::new(&args[2]),
                json!({"passed":true,"checkpoints":count,"original":observations[0],"rust":observations[1]}),
            )?;
            println!(
                "PASS {count} matching resident checkpoints across normal launch, commands, shutdown and restart"
            );
            Ok(())
        })();
        if let Err(e) = &result {
            let _ = write(
                Path::new(&args[2]),
                json!({"passed":false,"error":format!("{e}")}),
            );
        }
        // Only the unique, test-owned temp directory contains the mutable copies.
        fs::remove_dir_all(root)?;
        result
    }
}
