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

use cherrypi_fetch::render_page;

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

/// A valid 1x1 transparent PNG, so an `onerror` means the file was refused
/// rather than that the target was not an image.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xfc, 0xcf, 0xc0, 0x50,
    0x0f, 0x00, 0x04, 0x85, 0x01, 0x80, 0x84, 0xa9, 0x8c, 0x21, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn write_png() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("cherrypi-fetch-test-{}.png", std::process::id()));
    std::fs::write(&path, PNG).unwrap();
    path
}

fn data_page(html: &str) -> url::Url {
    url::Url::parse(&format!(
        "data:text/html;base64,{}",
        base64(html.as_bytes())
    ))
    .unwrap()
}

pub async fn blocks_private_and_non_http_destinations() {
    crate::common::setup();

    // A non-http top-level scheme is refused before the renderer starts.
    let file = url::Url::parse("file:///etc/hostname").unwrap();
    let result = render_page(file, Duration::from_secs(5)).await;
    assert!(result.is_err(), "file: should be refused: {result:?}");

    // A `file:` subresource is refused, which the page sees as a load error.
    // The target is a real PNG, so `onerror` proves the file was not read.
    let png = write_png();
    let file_page = data_page(&format!(
        "<html><body><div id='out'>before</div>\
         <img src='file://{}' \
         onerror=\"document.getElementById('out').textContent='file-blocked'\">\
         </body></html>",
        png.display()
    ));
    let page = render_page(file_page, Duration::from_secs(30))
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
        Duration::from_secs(30),
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
    let page = render_page(worker_page, Duration::from_secs(30))
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
}
