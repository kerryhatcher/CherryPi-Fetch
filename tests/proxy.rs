//! Servo's own connections go through the checking proxy. A plain-http page is
//! tunnelled with CONNECT and checked there, so the proxy's tunnel counter moves.
//!
//! Its own test binary: the renderer runs Servo once per process.

mod common;

use cherrypi_fetch::{proxied_tunnel_count, shutdown_renderer, RenderedPage};

use common::Reply;

#[tokio::test(flavor = "multi_thread")]
async fn plain_http_is_tunnelled_through_the_checking_proxy() {
    common::setup();
    let server = common::serve("127.0.0.1:0", |_| {
        Reply::html("<html><body>tunnelled-marker</body></html>")
    });
    common::mark_public(&[("127.0.0.1", server.port)]);

    let before = proxied_tunnel_count();
    let page: RenderedPage = common::render(&format!("http://127.0.0.1:{}/", server.port))
        .await
        .expect("the page should render through the proxy");
    assert!(
        page.text.contains("tunnelled-marker"),
        "the page did not load through the proxy: {page:?}"
    );
    assert!(
        proxied_tunnel_count() > before,
        "the page was fetched without the checking proxy"
    );

    shutdown_renderer();
}
