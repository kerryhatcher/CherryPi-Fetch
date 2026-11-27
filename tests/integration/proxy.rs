//! Servo's own connections go through the checking proxy. A plain-http page is
//! tunnelled with CONNECT and checked there, so the proxy's tunnel counter moves.
//!
//! Its own test binary: the renderer runs Servo once per process.

use cherrypi_fetch::{proxied_tunnel_count, RenderedPage};

use crate::common::Reply;

pub async fn plain_http_is_tunnelled_through_the_checking_proxy() {
    crate::common::setup();
    let server = crate::common::serve("127.0.0.1:0", |_| {
        Reply::html("<html><body>tunnelled-marker</body></html>")
    });
    crate::common::mark_public(&[("127.0.0.1", server.port)]);

    let before = proxied_tunnel_count();
    let page: RenderedPage = crate::common::render(&format!("http://127.0.0.1:{}/", server.port))
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
}
