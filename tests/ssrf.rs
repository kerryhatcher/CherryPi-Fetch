//! A resource load that points at a private address is blocked, even when the
//! top-level page itself is allowed.
#![cfg(feature = "cli")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use cherrypi_fetch::{configure, render_page, shutdown_renderer, RendererConfig};

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

/// Minimal standard base64, so the page can be a `data:` URL without pulling
/// in a dependency.
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

#[tokio::test(flavor = "multi_thread")]
async fn blocks_a_subresource_to_a_private_address() {
    configure(RendererConfig {
        user_agent: "ssrf-test/1.0".into(),
        allow_private_networks: false,
    })
    .unwrap();

    let (secret, hits) = counting_server();
    // A `data:` page has no host, so the top-level check lets it through. Its
    // only resource points at the loopback server.
    let html = format!("<html><body>hello<img src=\"{secret}\"></body></html>");
    let url = url::Url::parse(&format!(
        "data:text/html;base64,{}",
        base64(html.as_bytes())
    ))
    .unwrap();

    let page = render_page(url, Duration::from_secs(10))
        .await
        .expect("the data: page should render");
    assert!(page.text.contains("hello"), "text was: {:?}", page.text);

    // Give a leaked request time to arrive.
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the private subresource was fetched"
    );

    shutdown_renderer();
}
