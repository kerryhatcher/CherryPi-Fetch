//! A HEAD request that redirects keeps its method on the chained hop (F8).
//! The hop is fetched by this crate, so it must arrive as HEAD, not GET.
//!
//! Run with `cargo test --test integration`.

use crate::common::{mark_public, render, serve, setup, Reply};

const PAGE: &str = "<html><body><script>\
    var x = new XMLHttpRequest();\
    x.open('HEAD', '/h', false);\
    x.send();\
    document.body.textContent = 'HEAD-STATUS:' + x.status;\
    </script></body></html>";

pub async fn a_redirected_head_request_stays_a_head_request() {
    setup();
    let origin = serve("127.0.0.1:0", |req| match req.route() {
        "/" => Reply::html(PAGE),
        "/h" => Reply::redirect("/x"),
        "/x" => Reply::text("target body"),
        _ => Reply::not_found(),
    });
    mark_public(&[("127.0.0.1", origin.port)]);

    let page = render(&format!("http://127.0.0.1:{}/", origin.port))
        .await
        .expect("the page should render");

    let target = origin.hits("/x");
    assert!(
        !target.is_empty(),
        "the redirect target was never requested"
    );
    for hit in &target {
        assert_eq!(
            hit.method, "HEAD",
            "the redirected hop was sent as {} instead of HEAD",
            hit.method
        );
    }
    assert!(
        page.text.contains("HEAD-STATUS:200"),
        "the page did not get the HEAD response: {page:?}"
    );
}
