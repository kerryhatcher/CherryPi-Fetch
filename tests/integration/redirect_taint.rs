//! A same-origin redirect to a cross-origin stylesheet stays cross-origin (F1).
//! Servo fetches the stylesheet itself and applies its tainting to the final
//! response, so the page cannot read the rules: `cssRules` throws
//! `SecurityError`, as it does with the address check off.
//!
//! Run with `cargo test --test integration`.

use crate::common::{mark_public, render, serve, setup, Reply};

pub async fn a_redirected_cross_origin_stylesheet_is_not_readable() {
    setup();
    let other = serve("127.0.0.1:0", |req| match req.route() {
        "/cross.css" => Reply::ok_with_type(b"p { color: red; }".to_vec(), "text/css"),
        _ => Reply::not_found(),
    });
    let other_port = other.port;
    let page: &str = "<html><head><link rel=\"stylesheet\" href=\"/tr\"></head><body>\
         <div id=\"out\">before</div>\
         <script>\
         window.addEventListener('load', function () {\
             var out;\
             try { out = 'readable:' + document.styleSheets[0].cssRules.length; }\
             catch (e) { out = e.name; }\
             document.getElementById('out').textContent = 'TAINT:' + out;\
         });\
         </script></body></html>";
    let origin = serve("127.0.0.1:0", move |req| match req.route() {
        "/" => Reply::html(page),
        "/tr" => Reply::redirect(&format!("http://127.0.0.1:{other_port}/cross.css")),
        _ => Reply::not_found(),
    });
    mark_public(&[("127.0.0.1", origin.port), ("127.0.0.1", other_port)]);

    let rendered = render(&format!("http://127.0.0.1:{}/", origin.port))
        .await
        .expect("the page should render");
    assert!(
        rendered.text.contains("TAINT:SecurityError"),
        "the cross-origin stylesheet was readable after a redirect: {rendered:?}"
    );
}
