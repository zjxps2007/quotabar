use super::*;
use crate::{
    credentials::Platform,
    provider_requests::{self, Endpoint, Parameters},
};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

fn send_fixture(
    request: &Request,
    oauth: bool,
    discovery: bool,
    cancel: &AtomicBool,
) -> Result<Reply, TransportError> {
    #[cfg(windows)]
    {
        let _ = oauth;
        platform::send_winrt(request, discovery, cancel, false)
    }
    #[cfg(target_os = "macos")]
    platform::send(request, oauth, discovery, cancel)
}

pub(super) fn request(endpoint: Endpoint, url: String) -> Request {
    let platform = if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Mac
    };
    let mut request = provider_requests::build(
        &Parameters {
            endpoint,
            token: "fixture-token".into(),
            account: Some("fixture-workspace".into()),
            project: None,
            version: None,
            timeout: 3.,
        },
        platform,
    );
    request.url = url;
    request
}

pub(super) fn server(
    requests: usize,
    mut answer: impl FnMut(TcpStream, String, Vec<u8>) + Send + 'static,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let url = format!("http://{}/usage", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        for _ in 0..requests {
            let end = Instant::now() + Duration::from_secs(6);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < end =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("waiting for native request: {e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut headers = String::new();
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() != 0 && line != "\r\n" {
                headers.push_str(&line);
                line.clear();
            }
            let headers = headers.to_ascii_lowercase();
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map_or(0, |n| n.trim().parse().unwrap());
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            answer(stream, headers, body);
        }
    });
    (url, worker)
}

#[test]
fn native_quota_sends_account_headers_and_streams_the_response() {
    let (url, worker) = server(1, |mut stream, headers, body| {
        assert!(headers.starts_with("get /usage http/1.1"));
        assert!(headers.contains("authorization: bearer fixture-token"));
        assert!(headers.contains("chatgpt-account-id: fixture-workspace"));
        assert!(headers.contains("no-cache"));
        assert!(body.is_empty());
        stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\none\r\n3\r\ntwo\r\n0\r\n\r\n").unwrap();
    });
    let reply = send_fixture(
        &request(Endpoint::Codex, url),
        false,
        false,
        &AtomicBool::new(false),
    )
    .unwrap();
    worker.join().unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body.as_slice(), b"onetwo");
}

#[test]
fn native_oauth_posts_without_bearer_and_refuses_redirects() {
    let (url, worker) = server(1, |mut stream, headers, body| {
        assert!(headers.starts_with("post /usage http/1.1"));
        assert!(!headers.contains("authorization:"));
        assert!(headers.contains("content-type: application/json"));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["refresh_token"],
            "fixture-token"
        );
        stream.write_all(b"HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:1/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let reply = send_fixture(
        &request(Endpoint::Refresh, url),
        true,
        false,
        &AtomicBool::new(false),
    )
    .unwrap();
    worker.join().unwrap();
    assert_eq!(reply.status, 307);
}

#[test]
fn the_same_url_does_not_replay_a_cached_response_for_the_next_account() {
    let mut accounts = ["first", "second"].into_iter();
    let (url, worker) = server(2, move |mut stream, headers, _| {
        let account = accounts.next().unwrap();
        assert!(headers.contains(&format!("authorization: bearer {account}")));
        if cfg!(windows) {
            assert!(!headers.contains("\ncookie:"));
        }
        write!(stream, "HTTP/1.1 200 OK\r\nCache-Control: public, max-age=3600\r\nSet-Cookie: test=one; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{account}", account.len()).unwrap();
    });
    for account in ["first", "second"] {
        let mut description = request(Endpoint::Codex, url.clone());
        description
            .headers
            .insert("Authorization", format!("Bearer {account}"));
        let reply = send_fixture(&description, false, false, &AtomicBool::new(false)).unwrap();
        assert_eq!(reply.body.as_slice(), account.as_bytes());
    }
    worker.join().unwrap();
}

#[test]
fn native_gemini_lookup_uses_the_discovered_project_in_the_final_snapshot() {
    use crate::provider_query::Operation;
    use serde_json::{Value, json};
    let platform_kind = if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Mac
    };
    let make_server = |document: Value, expected: Option<Value>| {
        server(1, move |mut stream, headers, body| {
            assert!(headers.contains("authorization: bearer synthetic"));
            assert!(!headers.contains("chatgpt-account-id"));
            if let Some(expected) = &expected {
                assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), *expected);
            } else {
                assert!(body.is_empty());
            }
            let bytes = serde_json::to_vec(&document).unwrap();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).unwrap();
            stream.write_all(&bytes).unwrap();
        })
    };
    let (assist, a) = make_server(
        json!({"currentTier":{"id":"standard"}}),
        Some(json!({"metadata":{"ideType":"GEMINI_CLI","pluginType":"GEMINI"}})),
    );
    let (projects, b) = make_server(
        json!({"projects":[{"projectId":"gen-lang-client-example"}]}),
        None,
    );
    let (quota, c) = make_server(
        json!({"buckets":[{"modelId":"gemini-pro","remainingFraction":0.2}]}),
        Some(json!({"project":"gen-lang-client-example"})),
    );
    let query = Query::new(
        Operation::Gemini,
        br#"{"access_token":"synthetic","email":"fixture@example.invalid"}"#,
        platform_kind,
        15.,
    )
    .unwrap();
    let now = DateTime::from_timestamp(1_750_000_000, 0);
    let mut destinations = [assist, projects, quota].into_iter();
    let bytes = drive(
        query,
        &AtomicBool::new(false),
        now,
        |mut request, oauth, optional, flag| {
            request.url = destinations.next().expect("unexpected fourth request");
            send_fixture(&request, oauth, optional, flag)
        },
    )
    .unwrap_or_else(|_| panic!("native lookup failed"));
    for server in [a, b, c] {
        server.join().unwrap();
    }
    assert!(destinations.next().is_none());
    let snapshot: Value = serde_json::from_slice(&bytes).unwrap();
    if platform_kind == Platform::Windows {
        assert_eq!(snapshot["plan"], "standard");
        assert_eq!(snapshot["windows"][0]["remaining"], 20.);
        assert_eq!(snapshot["email"], "fixture@example.invalid");
    } else {
        assert_eq!(snapshot["accountId"], "gen-lang-client-example");
        assert_eq!(snapshot["primary"]["remainingPercent"], 20.);
        assert_eq!(snapshot["accountEmail"], "fixture@example.invalid");
    }
}

#[test]
fn native_cancellation_interrupts_a_pending_response() {
    let (started, signalled) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let (url, worker) = server(1, move |_stream, _, _| {
        started.send(()).unwrap();
        let _ = released.recv_timeout(Duration::from_secs(4));
    });
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let send =
        thread::spawn(move || send_fixture(&request(Endpoint::Codex, url), false, false, &flag));
    signalled.recv_timeout(Duration::from_secs(5)).unwrap();
    let start = Instant::now();
    cancel.store(true, Ordering::Release);
    assert!(matches!(
        send.join().unwrap(),
        Err(TransportError::Cancelled)
    ));
    assert!(start.elapsed() < Duration::from_secs(2));
    release.send(()).unwrap();
    worker.join().unwrap();
}

#[cfg(windows)]
#[test]
fn discovery_deadline_includes_the_body_and_success_bodies_have_a_limit() {
    let (release, released) = mpsc::channel();
    let (url, worker) = server(1, move |mut stream, _, _| {
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nx")
            .unwrap();
        let _ = released.recv_timeout(Duration::from_secs(4));
    });
    let mut lookup = request(Endpoint::GeminiProjects, url);
    lookup.timeout = 0.35;
    assert!(matches!(
        send_fixture(&lookup, false, true, &AtomicBool::new(false)),
        Err(TransportError::Timeout)
    ));
    let _ = release.send(());
    worker.join().unwrap();

    let (url, worker) = server(1, |mut stream, _, _| {
        let length = 2 * crate::vault::MAX_SIZE + 1;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        stream.write_all(&vec![b'x'; length]).unwrap();
    });
    assert!(matches!(
        send_fixture(
            &request(Endpoint::Codex, url),
            false,
            false,
            &AtomicBool::new(false)
        ),
        Err(TransportError::LargeResponse)
    ));
    worker.join().unwrap();
}
