//! The packaged monitor's synthetic update checks drive the normal asynchronous
//! controller. Only the remote HTTP response and the isolated data root are supplied.
use super::{Event, Phase, Updater};
use crate::updates::{
    REPOSITORY,
    download::{Resource, windows::fetch_with},
};
use serde_json::{Value, json};
use std::{
    fs, io,
    path::Path,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};
use uuid::Uuid;

// Compiled only into the old package in the disposable signing-key job. Use
// the same length limits, streaming, cancellation, signature and payload checks.
#[cfg(feature = "windows-update-fixture")]
pub(super) fn file_response(
    directory: &Path,
    transfer: crate::updates::download::Transfer<'_>,
) -> Result<bool, crate::updates::download::Error> {
    use std::io::Read;
    let name = match transfer.resource {
        Resource::Feed => "feed.json",
        Resource::Asset => transfer
            .url
            .rsplit('/')
            .next()
            .ok_or(crate::updates::download::Error::Download)?,
    };
    if Path::new(name).file_name().and_then(|s| s.to_str()) != Some(name) {
        return Err(crate::updates::download::Error::Download);
    }
    let path = directory.join(name);
    fetch_with(transfer, |_, response| {
        let mut input = match fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                response.headers(404, None, None)?;
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        if response.headers(200, Some(input.metadata()?.len()), None)? {
            let mut buffer = [0; 64 * 1024];
            loop {
                let size = input.read(&mut buffer)?;
                if size == 0 {
                    break;
                }
                response.write(&buffer[..size])?;
            }
        }
        Ok(())
    })
}

pub fn run(path: &Path, current: &str) -> io::Result<()> {
    let root = std::env::temp_dir().join(format!("QuotaBar.Update.Manager-{}", Uuid::new_v4()));
    let result = exercise(&root, current)
        .unwrap_or_else(|error| json!({"passed":false,"error":error.to_string()}));
    if root.exists() {
        fs::remove_dir_all(root)?;
    }
    fs::write(path, serde_json::to_vec(&result)?)
}

fn exercise(root: &Path, current: &str) -> io::Result<Value> {
    let (wake, wait) = mpsc::channel();
    let fixture = Arc::new(Mutex::new((404u16, "", 0usize)));
    let http = fixture.clone();
    let mut owner = Updater::with_transport(
        root.to_owned(),
        root.join("app"),
        current.to_owned(),
        move || wake.send(()).is_ok(),
        Arc::new(move |transfer| {
            assert!(transfer.resource == Resource::Feed);
            fetch_with(transfer, |_, response| {
                let mut fixture = http.lock().unwrap();
                fixture.2 += 1;
                let (status, tag, _) = *fixture;
                if response.headers(status, None, None)? {
                    let body = json!({"draft":false,"prerelease":false,"tag_name":tag,
                        "html_url":format!("{REPOSITORY}/releases/tag/{tag}"),"assets":[]});
                    response.write(&serde_json::to_vec(&body).expect("fixture response"))?;
                }
                Ok(())
            })
        }),
    );
    let offered = "2147483646.0.0";
    let mut checks = Vec::new();
    for (name, automatic, status, tag, phase, version, message) in [
        (
            "no published release",
            false,
            404,
            "",
            Phase::Idle,
            None,
            Some("공개된 정식 릴리즈가 없습니다."),
        ),
        (
            "installed version is current",
            false,
            200,
            "v0.0.1",
            Phase::Current,
            Some(current),
            None,
        ),
        (
            "manual check with automatic updates off",
            false,
            200,
            "v2147483646.0.0",
            Phase::Available,
            Some(offered),
            None,
        ),
        (
            "automatic update without a platform asset",
            true,
            200,
            "v2147483646.0.0",
            Phase::Available,
            Some(offered),
            Some("이 PC에 맞는 업데이트 파일이 아직 없습니다."),
        ),
        (
            "rate-limited feed",
            false,
            429,
            "",
            Phase::Failed,
            None,
            Some("업데이트 확인 요청이 제한되었습니다. 잠시 후 다시 시도하세요."),
        ),
        (
            "retry after rate limit",
            false,
            200,
            "v2147483646.0.0",
            Phase::Available,
            Some(offered),
            None,
        ),
    ] {
        let expected = {
            let mut f = fixture.lock().unwrap();
            f.0 = status;
            f.1 = tag;
            f.2 + 1
        };
        owner.configure(automatic, false, false);
        owner.check(true, false);
        owner.check(true, false);
        while owner.operation.is_some() {
            wait.recv_timeout(Duration::from_secs(5))
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, name))?;
            owner.dispatch();
        }
        let state = owner.state();
        if state.phase != phase
            || state.version.as_deref() != version
            || state.message.as_deref() != message
            || state.checked_at.is_none()
            || fixture.lock().unwrap().2 != expected
        {
            return Err(io::Error::other(format!("{name}: {state:?}")));
        }
        while let Some(event) = owner.next_event() {
            if !matches!(event, Event::State(_)) {
                return Err(io::Error::other("Unexpected installer handoff"));
            }
        }
        checks.push(name);
    }
    Ok(
        json!({"passed":true,"monitorPid":std::process::id(),"manualCheckWhileAutomaticUpdatesDisabled":true,"updateChecks":checks}),
    )
}
