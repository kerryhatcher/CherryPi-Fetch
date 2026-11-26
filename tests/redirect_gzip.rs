//! A compressed body is delivered decoded, on a redirected hop (F2) and on a
//! same-origin script request this crate proxies. The fixture is a gzip stream
//! of `GZIP-TARGET-TEXT`, made with Python's `gzip` module (mtime 0).
//!
//! Run with `cargo test --test redirect_gzip`.

mod common;

use common::{mark_public, render, serve, setup, Reply};

/// `gzip(b"GZIP-TARGET-TEXT")`, 36 bytes.
const GZIP_TEXT: &[u8] = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x02\xff\x73\x8f\xf2\x0c\xd0\x0d\x71\x0c\x72\x77\x0d\xd1\x0d\x71\x8d\x08\x01\x00\xf1\x37\x95\xe4\x10\x00\x00\x00";

fn gzip_reply() -> Reply {
    Reply::ok_with_type(GZIP_TEXT.to_vec(), "text/plain; charset=utf-8")
        .with_header("Content-Encoding", "gzip")
}

#[tokio::test(flavor = "multi_thread")]
async fn compressed_bodies_are_delivered_decoded() {
    setup();
    let origin = serve("127.0.0.1:0", |req| {
        match req.route() {
        // A top-level navigation whose redirect target is compressed.
        "/g" => Reply::redirect("/gz"),
        "/gz" => gzip_reply(),
        // A same-origin script request (not a redirect) that is compressed.
        "/" => Reply::html(
            "<html><body><script>\
             var x = new XMLHttpRequest();\
             x.open('GET', '/gzx', false);\
             x.send();\
             document.body.textContent = 'XHR:' + x.responseText + ' CE:' + (x.getResponseHeader('Content-Encoding') || 'none');\
             </script></body></html>",
        ),
        "/gzx" => gzip_reply(),
        _ => Reply::not_found(),
    }
    });
    mark_public(&[("127.0.0.1", origin.port)]);

    let redirected = render(&format!("http://127.0.0.1:{}/g", origin.port))
        .await
        .expect("the compressed redirect target should render");
    assert!(
        redirected.text.contains("GZIP-TARGET-TEXT"),
        "a compressed redirect target was not delivered decoded: {redirected:?}"
    );

    let scripted = render(&format!("http://127.0.0.1:{}/", origin.port))
        .await
        .expect("the page with a compressed script request should render");
    assert!(
        scripted.text.contains("XHR:GZIP-TARGET-TEXT CE:none"),
        "a compressed script response was not delivered decoded: {scripted:?}"
    );
    cherrypi_fetch::shutdown_renderer();
}
