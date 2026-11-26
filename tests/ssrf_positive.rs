//! Positive control for the SSRF guard: with private networks allowed, the
//! same subresources really are fetched. It is a separate test binary because
//! `configure` is process-wide.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use cherrypi_fetch::{configure, render_page, shutdown_renderer, RendererConfig};

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
async fn fetches_private_subresources_when_allowed() {
    clear_proxy_env();
    configure(RendererConfig {
        user_agent: "ssrf-positive/1.0".into(),
        allow_private_networks: true,
    })
    .unwrap();

    let (secret, hits) = counting_server();
    let page = data_page(&format!(
        "<html><body>hello<img src=\"{secret}\"></body></html>"
    ));
    let _ = render_page(page, Duration::from_secs(10)).await;
    thread::sleep(Duration::from_millis(300));
    assert!(
        hits.load(Ordering::SeqCst) >= 1,
        "the private subresource was not fetched even though it was allowed"
    );

    // The worker's fetch is a positive control for the worker case too.
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
        "the worker did not run: {:?}",
        page.text
    );
    assert!(
        worker_hits.load(Ordering::SeqCst) >= 1,
        "the allowed worker fetch was not made"
    );

    shutdown_renderer();
}
