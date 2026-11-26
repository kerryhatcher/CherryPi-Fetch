//! Credentials in a redirect's `Location` are never sent (F9). The credentials
//! are on a hop this crate fetches (`/v`), so the `Location` it returns is
//! joined and stripped by the chain. The target then sees no Authorization
//! header, and the secret appears in no header it receives.
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
        // Servo follows this hop, so the chain fetches /v itself.
        "/u" => Reply::redirect("/v"),
        // The chain fetches /v and gets a Location with credentials.
        "/v" => Reply::redirect(&format!(
            "http://user:{SECRET}@127.0.0.1:{}/target",
            cell.get().copied().unwrap_or(0)
        )),
        "/target" => Reply::text("USERINFO-TARGET"),
        _ => Reply::not_found(),
    });
    let _ = port.set(origin.port);
    mark_public(&[("127.0.0.1", origin.port)]);

    let page = render(&format!("http://127.0.0.1:{}/u", origin.port))
        .await
        .expect("the redirect should render");

    let target = origin.hits("/target");
    assert!(
        !target.is_empty(),
        "the redirect target was never requested"
    );
    for hit in &target {
        assert!(
            hit.header("authorization").is_none(),
            "credentials from the Location were sent: {hit:?}"
        );
        assert!(
            !hit.headers.iter().any(|(_, v)| v.contains(SECRET)),
            "the password from the Location appears in a request header: {hit:?}"
        );
    }
    assert!(page.text.contains("USERINFO-TARGET"), "page: {page:?}");
    cherrypi_fetch::shutdown_renderer();
}
