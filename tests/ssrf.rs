//! SSRF guard: a private subresource is blocked, a non-http scheme is refused,
//! and a request from a finished render never reaches the network. The
//! positive control lives in `tests/ssrf_positive.rs`, because `configure` is
//! process-wide.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use cherrypi_fetch::{configure, render_page, shutdown_renderer, RendererConfig};

/// A proxy in the environment would refuse every render, so clear it before
/// the test starts.
fn clear_proxy_env() {
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        std::env::remove_var(name);
    }
}

/// A server that counts every request it receives and answers `secret`.
fn counting_server() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            counter.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let body = "secret";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    (format!("http://{addr}/"), hits)
}

/// Minimal standard base64, so a page can be a `data:` URL without a
/// dependency.
fn base64(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn data_page(html: &str) -> url::Url {
    url::Url::parse(&format!(
        "data:text/html;base64,{}",
        base64(html.as_bytes())
    ))
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn blocks_private_and_non_http_destinations() {
    clear_proxy_env();
    configure(RendererConfig {
        user_agent: "ssrf-test/1.0".into(),
        allow_private_networks: false,
    })
    .unwrap();

    // A non-http top-level scheme is refused before the renderer starts.
    let file = url::Url::parse("file:///etc/hostname").unwrap();
    let result = render_page(file, Duration::from_secs(5)).await;
    assert!(result.is_err(), "file: should be refused: {result:?}");

    // A `file:` subresource is refused, which the page sees as a load error.
    let file_page = data_page(
        "<html><body><div id='out'>before</div>\
         <img src='file:///etc/hostname' \
         onerror=\"document.getElementById('out').textContent='file-blocked'\">\
         </body></html>",
    );
    let page = render_page(file_page, Duration::from_secs(10))
        .await
        .expect("the data: page should render");
    assert!(
        page.text.contains("file-blocked"),
        "the file: subresource was not refused: {:?}",
        page.text
    );

    // A private subresource is blocked. A `data:` page has no host, so the
    // top-level check lets it through; its only resource points at loopback.
    let (secret, hits) = counting_server();
    let page = render_page(
        data_page(&format!(
            "<html><body>hello<img src=\"{secret}\"></body></html>"
        )),
        Duration::from_secs(10),
    )
    .await
    .expect("the data: page should render");
    assert!(page.text.contains("hello"), "text was: {:?}", page.text);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the private subresource was fetched"
    );

    // A dedicated worker's fetch. The worker posts back first so the test
    // knows it ran; then its fetch to loopback must also be blocked.
    let (worker_secret, worker_hits) = counting_server();
    let worker_page = data_page(&format!(
        "<html><body><div id='out'>no worker</div>\
         <script>\
         var blob = new Blob(['postMessage(\"ran\"); fetch(\"{worker_secret}\")'], {{type: 'text/javascript'}});\
         var w = new Worker(URL.createObjectURL(blob));\
         w.onmessage = function (e) {{ document.getElementById('out').textContent = 'worker-' + e.data; }};\
         </script></body></html>"
    ));
    let page = render_page(worker_page, Duration::from_secs(10))
        .await
        .expect("the worker page should render");
    thread::sleep(Duration::from_millis(300));
    assert!(
        page.text.contains("worker-ran"),
        "the worker did not run, so the test would be vacuous: {:?}",
        page.text
    );
    assert_eq!(
        worker_hits.load(Ordering::SeqCst),
        0,
        "a worker fetch reached a private address"
    );

    // A page that keeps fetching in the background must not reach the network
    // after its render has returned. Every load with no live webview goes to
    // the servo-level delegate, which cancels it.
    let (flood, flood_hits) = counting_server();
    let flood_page = data_page(&format!(
        "<html><body><script>\
         setInterval(function () {{ fetch('{flood}'); }}, 0);\
         </script></body></html>"
    ));
    let _ = render_page(flood_page, Duration::from_secs(2)).await;
    let _ = render_page(
        url::Url::parse("data:text/plain,hi").unwrap(),
        Duration::from_secs(2),
    )
    .await;
    thread::sleep(Duration::from_millis(500));
    assert_eq!(
        flood_hits.load(Ordering::SeqCst),
        0,
        "requests from a finished render reached the network"
    );

    shutdown_renderer();
}
