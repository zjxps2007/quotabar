use super::*;
use crate::provider_http::tests::server;
use std::sync::atomic::Ordering;

/// Only the socket endpoint is replaced. Both clients consume the same HTTP/1.1
/// response using their ordinary Windows header and streaming implementations.
fn loopback(endpoint: &str, proxy: bool, response: &mut Response<'_>) -> Result<(), Error> {
    if proxy {
        let proxy = Proxy::parse(endpoint).unwrap();
        proxied(
            &Url::parse("http://release.invalid/asset").unwrap(),
            response,
            &proxy,
        )
    } else {
        winrt(&Url::parse(endpoint).unwrap(), response, false)
    }
}

#[test]
fn release_clients_stream_chunked_assets_larger_than_provider_responses() {
    const BLOCK: usize = 32768;
    const BLOCKS: usize = 96;
    for proxy in [false, true] {
        let (endpoint, worker) = server(1, move |mut stream, headers, body| {
            let target = if proxy {
                "http://release.invalid/asset"
            } else {
                "/usage"
            };
            assert!(headers.starts_with(&format!("get {target} http/1.1")));
            assert!(headers.contains("user-agent: quotabar-updater"));
            assert!(
                !headers.contains("\nauthorization:")
                    && !headers.contains("\ncookie:")
                    && body.is_empty()
            );
            stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nTrailer: X-Fixture\r\nConnection: close\r\n\r\n").unwrap();
            for n in 0..BLOCKS {
                write!(stream, "{BLOCK:x}\r\n").unwrap();
                stream.write_all(&[n as u8; BLOCK]).unwrap();
                stream.write_all(b"\r\n").unwrap();
            }
            stream.write_all(b"0\r\nX-Fixture: end\r\n\r\n").unwrap();
        });
        let mut output = Vec::new();
        let mut progress = Vec::new();
        let found = fetch_with(
            Transfer {
                resource: Resource::Asset,
                url: "https://release.invalid/asset",
                agent: "QuotaBar-Updater",
                limit: (BLOCK * BLOCKS) as u64,
                cancel: &AtomicBool::new(false),
                output: &mut output,
                progress: &mut |n| progress.push(n),
            },
            |_, response| loopback(&endpoint, proxy, response),
        )
        .unwrap();
        worker.join().unwrap();
        assert!(found);
        assert_eq!(output.len(), BLOCK * BLOCKS);
        for (n, block) in output.chunks_exact(BLOCK).enumerate() {
            assert!(block.iter().all(|b| *b == n as u8));
        }
        assert!(progress.len() > 1 && progress.windows(2).all(|v| v[0] < v[1]));
        assert_eq!(progress.last(), Some(&100));
    }
}

#[test]
fn release_clients_stop_during_the_body_and_leave_the_sink_bounded() {
    for proxy in [false, true] {
        let (received, hold_open) = std::sync::mpsc::channel();
        let (endpoint, worker) = server(1, move |mut stream, headers, _| {
            assert!(headers.contains("accept: application/vnd.github+json"));
            assert!(headers.contains("user-agent: quotabar/0.1.0"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 10000\r\nConnection: close\r\n\r\nprefix",
                )
                .unwrap();
            hold_open.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        let stop = AtomicBool::new(false);
        let mut output = Vec::new();
        let result = fetch_with(
            Transfer {
                resource: Resource::Feed,
                url: "https://release.invalid/latest",
                agent: "QuotaBar/0.1.0",
                limit: 10000,
                cancel: &stop,
                output: &mut output,
                progress: &mut |_| {
                    stop.store(true, Ordering::Release);
                    received.send(()).unwrap();
                },
            },
            |_, response| loopback(&endpoint, proxy, response),
        );
        worker.join().unwrap();
        assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
        assert_eq!(output, b"prefix");
    }
}
