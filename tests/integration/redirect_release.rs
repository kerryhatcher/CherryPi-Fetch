//! A same-origin script request (XHR) whose redirect leaves the origin is handed
//! back to Servo, which makes the request itself and follows the redirect with
//! CORS, as a browser does. Before this, the chain refused the hop and the page
//! saw a network error, so a public target that allows the page's origin could
//! not be read.
//!
//! - `/x` redirects to another public origin that sends
//!   `Access-Control-Allow-Origin: *`: the page must read the body.
//! - `/y` redirects to another public origin with no CORS header: the target is
//!   requested (by Servo, with an `Origin` header), but its body must not reach
//!   the page.
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

pub async fn a_same_origin_script_redirect_to_another_origin_follows_cors() {
    setup();

    // Both targets are other origins (other ports), and both are public.
    let cors = serve("127.0.0.1:0", |_| {
        Reply::text("CORS-TARGET-BODY").with_header("Access-Control-Allow-Origin", "*")
    });
    let plain = serve("127.0.0.1:0", |_| Reply::text("PLAIN-TARGET-BODY"));
    let cors_port = cors.port;
    let plain_port = plain.port;
    let origin = serve("127.0.0.1:0", move |req| match req.route() {
        "/" => page("/x"),
        "/x" => Reply::redirect(&format!("http://127.0.0.1:{cors_port}/cors")),
        "/p" => page("/y"),
        "/y" => Reply::redirect(&format!("http://127.0.0.1:{plain_port}/plain")),
        _ => Reply::not_found(),
    });
    mark_public(&[
        ("127.0.0.1", origin.port),
        ("127.0.0.1", cors_port),
        ("127.0.0.1", plain_port),
    ]);

    // Case 1: the target allows the origin, so the body is readable.
    let allowed = render(&format!("http://127.0.0.1:{}/", origin.port))
        .await
        .expect("the page should render");
    let hits = cors.hits("/cors");
    assert!(
        !hits.is_empty(),
        "the cross-origin hop was never requested: {:?}",
        cors.requests()
    );
    assert_eq!(
        hits[0].header("Origin"),
        Some(format!("http://127.0.0.1:{}", origin.port).as_str()),
        "the hop was not requested in CORS mode: {hits:?}"
    );
    assert!(
        allowed.text.contains("RESULT:200:CORS-TARGET-BODY"),
        "a same-origin XHR redirect to a CORS-enabled origin was not readable: {allowed:?}"
    );

    // Case 2: the target has no CORS header, so its body must stay unreadable.
    let blocked = render(&format!("http://127.0.0.1:{}/p", origin.port))
        .await
        .expect("the page should render");
    assert!(
        !plain.hits("/plain").is_empty(),
        "the cross-origin hop was never requested: {:?}",
        plain.requests()
    );
    assert!(
        blocked.text.contains("RESULT:"),
        "the script did not run: {blocked:?}"
    );
    assert!(
        !blocked.text.contains("PLAIN-TARGET-BODY"),
        "a target without CORS headers was readable by the page: {blocked:?}"
    );
}
