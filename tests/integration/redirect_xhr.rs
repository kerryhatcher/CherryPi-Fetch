//! A same-origin script request (XHR) that redirects is proxied by this crate,
//! so every hop of its redirect chain must be checked, not left to reqwest's
//! own redirect following.
//!
//! - `/r` redirects to `localhost`, which resolves to 127.0.0.1 and is not
//!   marked public: the target must never be requested.
//! - `/s` redirects to another path on the same public origin: it must work.
//! - `/x` redirects to another origin: the request is handed to Servo, which
//!   follows the redirect with CORS. The target has no CORS header, so its body
//!   must stay unreadable.
//!
//! Run with `cargo test --test integration`.

use crate::common::{mark_public, render, serve, setup, Reply};

/// A page whose script fetches `path` synchronously and reports the outcome.
fn page(path: &str) -> Reply {
    Reply::html(format!(
        "<html><body><script>\
         var x = new XMLHttpRequest();\
         try {{ x.open('GET', '{path}', false); x.send();\
           document.body.textContent = 'RESULT:' + x.status + ':' + x.responseText; }}\
         catch (e) {{ document.body.textContent = 'RESULT:BLOCKED'; }}\
         </script></body></html>"
    ))
}

pub async fn a_script_request_redirect_is_checked_on_every_hop() {
    setup();

    // Case 1: a redirect to a hostname that resolves to loopback.
    let loopback = serve("127.0.0.1:0", |_| Reply::text("LOCALHOST-SECRET"));
    let loopback_port = loopback.port;

    // Case 3 target: another origin that is public (marked below).
    let other = serve("127.0.0.1:0", |_| Reply::text("OTHER-ORIGIN"));

    let other_port = other.port;
    let origin = serve("127.0.0.1:0", move |req| match req.route() {
        "/" => page("/r"),
        "/r" => Reply::redirect(&format!("http://localhost:{loopback_port}/target")),
        "/target" => Reply::text("LOCALHOST-SECRET"),
        "/b" => page("/s"),
        "/s" => Reply::redirect("/same"),
        "/same" => Reply::text("SAME-ORIGIN-TARGET"),
        "/c" => page("/x"),
        "/x" => Reply::redirect(&format!("http://127.0.0.1:{other_port}/o")),
        _ => Reply::not_found(),
    });
    mark_public(&[("127.0.0.1", origin.port), ("127.0.0.1", other_port)]);

    let blocked = render(&format!("http://127.0.0.1:{}/", origin.port))
        .await
        .expect("the page should render");
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        loopback.hits("/target").is_empty(),
        "a same-origin XHR redirect reached a hostname that resolves to loopback: {:?}",
        loopback.requests()
    );
    assert!(
        !blocked.text.contains("LOCALHOST-SECRET"),
        "the loopback body reached the page: {blocked:?}"
    );

    // Case 2: a same-origin redirect to a public origin still works.
    let same = render(&format!("http://127.0.0.1:{}/b", origin.port))
        .await
        .expect("the page should render");
    assert!(
        same.text.contains("RESULT:200:SAME-ORIGIN-TARGET"),
        "a same-origin XHR redirect did not reach its target: {same:?}"
    );

    // Case 3: a script request redirected to another origin is released to
    // Servo, which applies CORS. The target sends no CORS header, so the body
    // must not reach the page (it may be requested, by Servo).
    let cross = render(&format!("http://127.0.0.1:{}/c", origin.port))
        .await
        .expect("the page should render");
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        !cross.text.contains("OTHER-ORIGIN"),
        "the other origin's body reached the page: {cross:?}"
    );
}
