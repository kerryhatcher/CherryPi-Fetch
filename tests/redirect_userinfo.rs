//! Credentials in a redirect's `Location` are never sent (F9). The top-level
//! redirect lands on a URL with user info. That load is refused before Servo
//! fetches it: Servo's own fetch of such a navigation never completes, and the
//! credentials must not reach the target in any case. The target sees no
//! Authorization header, and the secret appears in no header it receives.
//!
//! Run with `cargo test --test redirect_userinfo`.

mod common;

use std::sync::{Arc, OnceLock};

use common::{mark_public, render, serve, setup, Reply};

const SECRET: &str = "s3cret-password";

#[tokio::test(flavor = "multi_thread")]
async fn userinfo_in_a_redirect_location_is_not_sent() {
    setup();
    // The Location names this server's own port, which is known only after it
    // is bound, so the route reads it from a cell set straight away.
    let port = Arc::new(OnceLock::new());
    let cell = port.clone();
    let origin = serve("127.0.0.1:0", move |req| match req.route() {
        // Servo follows this hop itself.
        "/u" => Reply::redirect("/v"),
        // The Location carries credentials, so the load it names is refused.
        "/v" => Reply::redirect(&format!(
            "http://user:{SECRET}@127.0.0.1:{}/target",
            cell.get().copied().unwrap_or(0)
        )),
        "/target" => Reply::text("USERINFO-TARGET"),
        _ => Reply::not_found(),
    });
    let _ = port.set(origin.port);
    mark_public(&[("127.0.0.1", origin.port)]);

    let page = render(&format!("http://127.0.0.1:{}/u", origin.port)).await;
    std::thread::sleep(std::time::Duration::from_millis(300));

    for hit in origin.hits("/target") {
        assert!(
            hit.header("authorization").is_none(),
            "credentials from the Location were sent: {hit:?}"
        );
        assert!(
            !hit.headers.iter().any(|(_, v)| v.contains(SECRET)),
            "the password from the Location appears in a request header: {hit:?}"
        );
    }
    let text = page.map(|p| p.text).unwrap_or_default();
    assert!(
        !text.contains("USERINFO-TARGET"),
        "the target's body reached the page: {text:?}"
    );
    cherrypi_fetch::shutdown_renderer();
}
