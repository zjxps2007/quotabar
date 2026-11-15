use super::*;
use crate::provider_http::{
    platform,
    tests::{request, server},
};
use crate::provider_requests::Endpoint;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::{Arc, mpsc},
    thread,
};

#[test]
fn packaged_transport_supports_windows_tls_and_proxy_authentication() {
    let library = curl::Version::get();
    assert_eq!(library.ssl_version(), Some("Schannel"));
    assert!(library.feature_https_proxy() && library.feature_sspi());
    assert!(library.feature_ntlm() && library.feature_spnego());
}

#[test]
fn environment_routes_match_windows_proxy_and_bypass_rules() {
    fn routes(vars: &[(&str, &str)]) -> Environment {
        Environment::read(|key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, s)| s.to_string())
        })
    }
    fn endpoint<'a>(config: &'a Environment, destination: &str) -> &'a str {
        match config.route(destination).unwrap() {
            Route::System => "system",
            Route::Direct => "direct",
            Route::Explicit(p) => &p.endpoint,
        }
    }
    for (vars, http, https) in [
        (vec![], "system", "system"),
        (vec![("HTTP_PROXY", "bad:port")], "system", "system"),
        (
            vec![("HTTP_PROXY", "localhost:3128/")],
            "http://localhost:3128",
            "direct",
        ),
        (
            vec![("GATEWAY_INTERFACE", "CGI/1.1"), ("HTTP_PROXY", "ignored")],
            "system",
            "system",
        ),
        (
            vec![
                ("ALL_PROXY", "socks5://[::1]"),
                ("HTTP_PROXY", "bad:65536"),
                ("HTTPS_PROXY", "HTTPS://secure"),
            ],
            "socks5h://[::1]:80",
            "https://secure:443",
        ),
        (
            vec![
                ("HTTP_PROXY", "proxy.test"),
                ("ALL_PROXY", "Socks4a://all.test:1080"),
            ],
            "http://proxy.test:80",
            "socks4a://all.test:1080",
        ),
    ] {
        let config = routes(&vars);
        assert_eq!(endpoint(&config, "http://provider.invalid/"), http);
        assert_eq!(endpoint(&config, "https://provider.invalid/"), https);
    }
    let config = routes(&[
        ("ALL_PROXY", "socks4://proxy:1234"),
        (
            "NO_PROXY",
            " EXACT.test , .example.test,*,else.test:80,192.0.2.0/24,[::1]",
        ),
    ]);
    for host in ["exact.test", "example.test", "sub.example.test", "[::1]"] {
        assert_eq!(endpoint(&config, &format!("https://{host}/")), "direct");
    }
    for host in [
        "sub.exact.test",
        "notexample.test",
        "else.test",
        "192.0.2.4",
        "other.test",
    ] {
        assert_eq!(
            endpoint(&config, &format!("http://{host}/")),
            "socks4://proxy:1234"
        );
    }
}

#[test]
fn connect_is_authenticated_after_challenge_and_keeps_provider_headers_private() {
    let mut count = 0;
    let (url, worker) = server(2, move |mut stream, headers, body| {
        assert!(headers.starts_with("connect provider.invalid:443 http/1.1"));
        assert!(!headers.contains("\nauthorization:"));
        assert!(!headers.contains("chatgpt-account-id"));
        assert!(body.is_empty());
        if count == 0 {
            assert!(!headers.contains("proxy-authorization:"));
            stream.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"proxy\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        } else {
            // user:p@ss, compared case-insensitively by the shared header reader.
            assert!(
                headers.contains(
                    "proxy-authorization: basic dxnlcjpwqhNz"
                        .to_ascii_lowercase()
                        .as_str()
                )
            );
            stream
                .write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        }
        count += 1;
    });
    let authority = Url::parse(&url).unwrap();
    let proxy = Proxy::parse(&format!(
        "http://user:p%40ss@127.0.0.1:{}",
        authority.port().unwrap()
    ))
    .unwrap();
    let result = proxy.send(
        &request(Endpoint::Codex, "https://provider.invalid/usage".into()),
        false,
        &AtomicBool::new(false),
    );
    worker.join().unwrap();
    assert!(matches!(result, Err(TransportError::Network(_))));
}

fn spawn_proxy(test: impl FnOnce(TcpStream) + Send + 'static) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let worker = thread::spawn(move || {
        let end = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < end => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("waiting for proxy connection: {e}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        test(stream)
    });
    (port, worker)
}

fn read<const N: usize>(stream: &mut impl Read) -> [u8; N] {
    let mut data = [0; N];
    stream.read_exact(&mut data).unwrap();
    data
}

#[test]
fn socks4_socks4a_and_authenticated_socks5_reach_the_origin() {
    for dialect in ["socks4", "socks4a", "socks5"] {
        let (port, worker) = spawn_proxy(move |stream| {
            let mut stream = BufReader::new(stream);
            if dialect == "socks5" {
                assert_eq!(read::<1>(&mut stream), [5]);
                let [count] = read(&mut stream);
                let mut methods = vec![0; count as usize];
                stream.read_exact(&mut methods).unwrap();
                methods.sort_unstable();
                assert_eq!(methods, [0, 2]);
                stream.get_mut().write_all(&[5, 2]).unwrap();
                assert_eq!(read::<10>(&mut stream), *b"\x01\x04user\x03p@w");
                stream.get_mut().write_all(&[1, 0]).unwrap();
                assert_eq!(read::<5>(&mut stream), [5, 1, 0, 3, 15]);
                assert_eq!(read::<15>(&mut stream), *b"example.invalid");
                assert_eq!(read::<2>(&mut stream), 8080u16.to_be_bytes());
                stream
                    .get_mut()
                    .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])
                    .unwrap();
            } else {
                assert_eq!(read::<4>(&mut stream), [4, 1, 0x1f, 0x90]);
                let ip = read::<4>(&mut stream);
                assert_eq!(read::<5>(&mut stream), *b"user\0");
                if dialect == "socks4a" {
                    assert_eq!(ip, [0, 0, 0, 1]);
                    assert_eq!(read::<16>(&mut stream), *b"example.invalid\0");
                } else {
                    assert_eq!(ip, [127, 0, 0, 1]);
                }
                stream
                    .get_mut()
                    .write_all(&[0, 90, 0, 0, 0, 0, 0, 0])
                    .unwrap();
            }
            let mut header = String::new();
            loop {
                let mut line = String::new();
                assert!(stream.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                header.push_str(&line);
            }
            assert!(header.starts_with("GET /usage HTTP/1.1"));
            assert!(header.contains("Authorization: Bearer fixture-token"));
            assert!(!header.to_ascii_lowercase().contains("proxy-authorization"));
            stream
                .get_mut()
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .unwrap();
        });
        let proxy =
            Proxy::parse(&format!("{dialect}://domain%5Cuser:p%40w@127.0.0.1:{port}")).unwrap();
        let host = if dialect == "socks4" {
            "127.0.0.1"
        } else {
            "example.invalid"
        };
        let reply = proxy.send(
            &request(Endpoint::Codex, format!("http://{host}:8080/usage")),
            false,
            &AtomicBool::new(false),
        );
        worker.join().unwrap();
        assert_eq!(reply.unwrap().body.as_slice(), b"ok");
    }
}

#[test]
fn native_entry_uses_process_proxy_and_honors_bypass() {
    const CHILD: &str = "QUOTABAR_TEST_PROXY_ORIGIN";
    if let Ok(origin) = std::env::var(CHILD) {
        for (url, expected) in [
            ("http://provider.invalid/usage".to_owned(), "proxy"),
            (origin, "direct"),
        ] {
            let reply = platform::send(
                &request(Endpoint::Codex, url),
                false,
                false,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(reply.body.as_slice(), expected.as_bytes());
        }
        return;
    }
    let (origin, direct) = server(1, |mut stream, headers, _| {
        assert!(headers.starts_with("get /usage http/1.1"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\ndirect")
            .unwrap();
    });
    let (proxy, proxied) = server(1, |mut stream, headers, _| {
        assert!(headers.starts_with("get http://provider.invalid/usage http/1.1"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nproxy")
            .unwrap();
    });
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "provider_http::proxy::tests::native_entry_uses_process_proxy_and_honors_bypass",
            "--nocapture",
        ])
        .env(CHILD, origin)
        .env_remove("GATEWAY_INTERFACE")
        .env_remove("ALL_PROXY")
        .env_remove("HTTPS_PROXY")
        .env("HTTP_PROXY", proxy)
        .env("NO_PROXY", "127.0.0.1")
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    proxied.join().unwrap();
    direct.join().unwrap();
}

#[test]
fn proxy_deadlines_cancellation_and_large_bodies_follow_native_contract() {
    let make_request = || request(Endpoint::Codex, "http://provider.invalid/usage".into());
    for discovery in [false, true] {
        let (url, worker) = server(1, |mut stream, _, _| {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n")
                .unwrap();
            thread::sleep(Duration::from_millis(850));
            let _ = stream.write_all(b"x");
        });
        let mut description = make_request();
        description.timeout = 0.4;
        let result =
            Proxy::parse(&url)
                .unwrap()
                .send(&description, discovery, &AtomicBool::new(false));
        worker.join().unwrap();
        if discovery {
            assert!(matches!(result, Err(TransportError::Timeout)));
        } else {
            assert_eq!(result.unwrap().body.as_slice(), b"x");
        }
    }
    let (url, worker) = server(1, |mut stream, _, _| {
        let size = 2 * crate::vault::MAX_SIZE + 1;
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\n\r\n").unwrap();
        let _ = stream.write_all(&vec![0; size]);
    });
    assert!(matches!(
        Proxy::parse(&url)
            .unwrap()
            .send(&make_request(), false, &AtomicBool::new(false)),
        Err(TransportError::LargeResponse)
    ));
    worker.join().unwrap();
    let (started, ready) = mpsc::channel();
    let (release, hold) = mpsc::channel();
    let (url, worker) = server(1, move |mut stream, _, _| {
        stream
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .unwrap();
        started.send(()).unwrap();
        let _ = hold.recv_timeout(Duration::from_secs(3));
    });
    let flag = Arc::new(AtomicBool::new(false));
    let cancel = Arc::clone(&flag);
    let client = thread::spawn(move || {
        Proxy::parse(&url).unwrap().send(
            &request(Endpoint::Codex, "https://provider.invalid/usage".into()),
            false,
            &flag,
        )
    });
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    cancel.store(true, Ordering::Release);
    assert!(matches!(
        client.join().unwrap(),
        Err(TransportError::Cancelled)
    ));
    release.send(()).unwrap();
    worker.join().unwrap();
}
