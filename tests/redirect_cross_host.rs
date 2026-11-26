//! A redirect to another host (F4). Each hop is checked by its own address, so
//! a public host may redirect to another public host. A non-public host is
//! refused even when it listens on the same port number as a public origin,
//! which checks that the test hook matches host and port together.
//!
//! Both cases run in one test: the renderer starts once per process.
//!
//! Run with `cargo test --test redirect_cross_host`. It binds 127.0.0.2, which
//! only Linux routes to loopback by default, so it is compiled on Linux only.
#![cfg(target_os = "linux")]

mod common;

use common::{mark_public, render, serve, setup, Reply};

#[tokio::test(flavor = "multi_thread")]
async fn redirects_between_hosts_follow_the_address_check_on_each_hop() {
    setup();

    // Case 1: public origin -> another public host renders the target.
    let target = serve("127.0.0.2:0", |req| match req.route() {
        "/target" => Reply::html("<html><body>CROSS-HOST-TARGET</body></html>"),
        _ => Reply::not_found(),
    });
    let target_port = target.port;
    let origin = serve("127.0.0.1:0", move |req| match req.route() {
        "/top" => Reply::redirect(&format!("http://127.0.0.2:{target_port}/target")),
        _ => Reply::not_found(),
    });
    mark_public(&[("127.0.0.1", origin.port), ("127.0.0.2", target_port)]);
    let page = render(&format!("http://127.0.0.1:{}/top", origin.port))
        .await
        .expect("the cross-host redirect should render");
    assert!(
        page.text.contains("CROSS-HOST-TARGET"),
        "a redirect to another public host did not render the target: {page:?}"
    );

    // Case 2: a host that is not marked, listening on the same port number as
    // a marked origin, is refused. Only the host and port pair is public.
    let unmarked = serve("127.0.0.2:0", |_| {
        Reply::html("<html><body>UNMARKED-TARGET</body></html>")
    });
    let port = unmarked.port;
    let _origin = serve(&format!("127.0.0.1:{port}"), move |req| match req.route() {
        "/cross" => Reply::redirect(&format!("http://127.0.0.2:{port}/target")),
        _ => Reply::not_found(),
    });
    mark_public(&[("127.0.0.1", port)]);
    let page = render(&format!("http://127.0.0.1:{port}/cross")).await;
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        unmarked.hits("/target").is_empty(),
        "a redirect reached a host that was not marked public: {:?}",
        unmarked.requests()
    );
    assert!(
        !page
            .as_ref()
            .is_ok_and(|p| p.text.contains("UNMARKED-TARGET")),
        "the unmarked target's body reached the page: {page:?}"
    );
    cherrypi_fetch::shutdown_renderer();
}
