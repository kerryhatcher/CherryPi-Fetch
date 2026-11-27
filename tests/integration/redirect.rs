//! A redirected load must be checked too (N1).
//!
//! The `__test-hooks` feature marks a chosen loopback host and port as public, so a
//! redirect can be exercised without a real public address:
//!
//! ```sh
//! cargo test --test integration
//! ```
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use cherrypi_fetch::{configure, render_page, set_test_public_origins, RendererConfig};

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

/// Read a request line and its headers, so the connection can be answered.
fn drain_request(stream: &mut TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    let _ = reader.read_line(&mut line);
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
            break;
        }
    }
}

/// A server that counts every request and answers 200 with `body`.
fn counting_server(body: &'static str) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            counter.fetch_add(1, Ordering::SeqCst);
            drain_request(&mut stream);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    (port, hits)
}

/// A server that answers every request with a 302 to `target`.
fn redirect_server(target: &str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let target = target.to_string();
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            drain_request(&mut stream);
            let resp = format!(
                "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

/// A server that answers every request with 200 and `html`.
fn html_server(html: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            drain_request(&mut stream);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
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

pub async fn a_redirect_to_loopback_is_refused() {
    clear_proxy_env();
    let (secret_port, secret_hits) = counting_server("LOOPBACK-SECRET");
    let top_port = redirect_server(&format!("http://127.0.0.1:{secret_port}/secret"));
    // The top-level port is public; the redirect target is not.
    set_test_public_origins(&[("127.0.0.1", top_port)]);

    configure(RendererConfig {
        user_agent: "redirect-test/1.0".into(),
        allow_private_networks: false,
    })
    .unwrap();

    // A top-level navigation that redirects to loopback.
    let url = url::Url::parse(&format!("http://127.0.0.1:{top_port}/top")).unwrap();
    let page = render_page(url, Duration::from_secs(30)).await;
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        secret_hits.load(Ordering::SeqCst),
        0,
        "a top-level redirect to loopback was followed (N1): {page:?}"
    );

    // An image whose load redirects to loopback.
    let html = format!("<html><body><img src=\"http://127.0.0.1:{top_port}/img\"></body></html>");
    let page_url = url::Url::parse(&format!(
        "data:text/html;base64,{}",
        base64(html.as_bytes())
    ))
    .unwrap();
    let _ = render_page(page_url, Duration::from_secs(30)).await;
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        secret_hits.load(Ordering::SeqCst),
        0,
        "an image redirect to loopback was followed (N1)"
    );

    // A public page that redirects to another public page still renders the
    // target. This is the main-frame case: the redirect hop is answered with
    // a 200 and the final page must be shown.
    let target = html_server(
        "<html><head><title>Final</title></head><body>REDIRECT-FINAL-PAGE</body></html>",
    );
    let public_top = redirect_server(&format!("http://127.0.0.1:{target}/target"));
    set_test_public_origins(&[("127.0.0.1", public_top), ("127.0.0.1", target)]);
    let page = render_page(
        url::Url::parse(&format!("http://127.0.0.1:{public_top}/top")).unwrap(),
        Duration::from_secs(30),
    )
    .await
    .expect("the public redirect should render");
    assert!(
        page.text.contains("REDIRECT-FINAL-PAGE"),
        "a public redirect did not render the target: {page:?}"
    );
}
