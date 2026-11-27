//! A cross-origin script fetch whose request redirects is not made to read the
//! target (F1). Servo follows the redirect itself and runs its CORS check on the
//! final response, as a browser does. The target has no CORS header, so its body
//! must never reach the page. The target may be requested, on the same host and
//! port the script already named: it is an ordinary request that CORS then blocks.
//!
//! Run with `cargo test --test integration`.

use crate::common::{mark_public, render, serve, setup, Reply};

pub async fn a_cross_origin_script_fetch_that_redirects_is_refused() {
    setup();
    // A second origin (another port, so another origin) that redirects a
    // script fetch to a page with no CORS header.
    let other = serve("127.0.0.1:0", |req| match req.route() {
        // The redirect itself passes CORS (ACAO on the hop), so Servo follows
        // it; the target /b has no ACAO, so its body is blocked.
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

    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        !page.text.contains("CROSS-ORIGIN-SECRET"),
        "the cross-origin redirect target's body reached the page: {page:?}"
    );
    assert!(
        page.text.contains("RESULT:"),
        "the script did not run: {page:?}"
    );
}
