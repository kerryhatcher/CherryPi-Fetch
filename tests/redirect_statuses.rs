//! Only 301, 302, 303, 307 and 308 with a `Location` are redirects (F4). A 300
//! with a `Location` and a 304 with none are ordinary responses, and the page
//! sees them as they are.
//!
//! Run with `cargo test --test redirect_statuses`.

mod common;

use common::{mark_public, render, serve, setup, Reply};

const PAGE: &str = "<html><body><div id=\"out\">before</div>\
    <script>\
    function status(path) {\
        var x = new XMLHttpRequest();\
        x.open('GET', path, false);\
        try { x.send(); return String(x.status); } catch (e) { return 'error'; }\
    }\
    document.getElementById('out').textContent = 'RESULT:' + status('/n304') + ':' + status('/m300');\
    </script></body></html>";

#[tokio::test(flavor = "multi_thread")]
async fn only_the_five_redirect_statuses_are_followed() {
    setup();
    let origin = serve("127.0.0.1:0", |req| match req.route() {
        "/" => Reply::html(PAGE),
        // A 304 has no Location and is the final response.
        "/n304" => Reply {
            status: "304 Not Modified",
            headers: Vec::new(),
            body: Vec::new(),
        },
        // A 300 names a Location but is not a redirect status, so it is final.
        "/m300" => Reply {
            status: "300 Multiple Choices",
            headers: vec![("Location".into(), "/elsewhere".into())],
            body: b"choices".to_vec(),
        },
        "/elsewhere" => Reply::text("FOLLOWED"),
        _ => Reply::not_found(),
    });
    mark_public(&[("127.0.0.1", origin.port)]);

    let page = render(&format!("http://127.0.0.1:{}/", origin.port))
        .await
        .expect("the page should render");
    assert!(
        page.text.contains("RESULT:304:300"),
        "the statuses were not returned as they are: {page:?}"
    );
    assert!(
        origin.hits("/elsewhere").is_empty(),
        "a 300 was followed as a redirect"
    );
    cherrypi_fetch::shutdown_renderer();
}
