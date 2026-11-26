//! A cross-origin script fetch whose request redirects is refused (F1). An
//! intercepted hop skips Servo's CORS check, so the target must never be
//! requested and its body must never reach the page.
//!
//! Run with `cargo test --test redirect_cors`.

mod common;

use common::{mark_public, render, serve, setup, Reply};

#[tokio::test(flavor = "multi_thread")]
async fn a_cross_origin_script_fetch_that_redirects_is_refused() {
    setup();
    // A second origin (another port, so another origin) that redirects a
    // script fetch to a page with no CORS header.
    let other = serve("127.0.0.1:0", |req| match req.route() {
        // The redirect itself passes CORS (ACAO on the hop), so Servo follows
        // it; the target /b has no ACAO and must not be fetched.
        "/a" => Reply::redirect("/b").with_header("Access-Control-Allow-Origin", "*"),
        "/b" => Reply::text("CROSS-ORIGIN-SECRET"),
        _ => Reply::not_found(),
    });
    let page_html = format!(
        "<html><body><script>\
         var x = new XMLHttpRequest();\
         x.open('GET', 'http://127.0.0.1:{}/a', false);\
         try {{ x.send(); document.body.textContent = 'RESULT:' + x.status + ':' + x.responseText; }}\
         catch (e) {{ document.body.textContent = 'RESULT:BLOCKED'; }}\
         </script></body></html>",
        other.port
    );
    let origin = serve("127.0.0.1:0", move |req| match req.route() {
        "/" => Reply::html(page_html.clone()),
        _ => Reply::not_found(),
    });
    mark_public(&[("127.0.0.1", origin.port), ("127.0.0.1", other.port)]);

    let page = render(&format!("http://127.0.0.1:{}/", origin.port))
        .await
        .expect("the page should render");

    assert!(
        other.hits("/b").is_empty(),
        "the cross-origin redirect target was requested: {:?}",
        other.requests()
    );
    assert!(
        !page.text.contains("CROSS-ORIGIN-SECRET"),
        "the cross-origin redirect target's body reached the page: {page:?}"
    );
    assert!(
        page.text.contains("RESULT:"),
        "the script did not run: {page:?}"
    );
    cherrypi_fetch::shutdown_renderer();
}
