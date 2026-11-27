//! Cookies on a redirect chain (F3). Hop `/c` is fetched by Servo, so its
//! `Set-Cookie` lands in Servo's jar. Hops `/d` and `/e` are fetched by this
//! crate, so the jar's cookies must be sent on each of them, and each hop's own
//! `Set-Cookie` must be stored for the hops after it.
//!
//! Run with `cargo test --test redirect_cookies`.

mod common;

use common::{mark_public, render, serve, setup, Reply};

#[tokio::test(flavor = "multi_thread")]
async fn cookies_are_sent_and_stored_on_every_redirect_hop() {
    setup();
    let origin = serve("127.0.0.1:0", |req| match req.route() {
        "/c" => Reply::redirect("/d").with_header("Set-Cookie", "sid=abc123; Path=/"),
        "/d" => Reply::redirect("/e").with_header("Set-Cookie", "hop=2; Path=/"),
        "/e" => Reply::text("chain ended"),
        _ => Reply::not_found(),
    });
    mark_public(&[("127.0.0.1", origin.port)]);

    let page = render(&format!("http://127.0.0.1:{}/c", origin.port))
        .await
        .expect("the redirect chain should render");

    // Hop /d was fetched by this crate: it must have had the cookie Servo stored.
    let d = origin.hits("/d");
    assert_eq!(d.len(), 1, "hop /d was not requested once: {d:?}");
    assert_eq!(
        d[0].header("cookie"),
        Some("sid=abc123"),
        "hop /d did not receive the cookie set by hop /c"
    );

    // Hop /e must have both cookies: sid from Servo's jar, and hop=2 stored
    // from the intermediate hop /d's own Set-Cookie.
    let e = origin.hits("/e");
    assert_eq!(e.len(), 1, "hop /e was not requested once: {e:?}");
    let mut sent: Vec<&str> = e[0].header("cookie").unwrap_or("").split("; ").collect();
    sent.sort_unstable();
    assert_eq!(
        sent,
        ["hop=2", "sid=abc123"],
        "hop /e did not receive both cookies (intermediate Set-Cookie not stored)"
    );
    assert!(page.text.contains("chain ended"), "page: {page:?}");

    // A cookie set by 127.0.0.1 is not sent to a redirect to localhost, which is
    // another host name: the jar answers for the next hop's own URL. (127.0.0.2
    // is not used: macOS has no such loopback address by default.)
    let other = serve("127.0.0.1:0", |_| Reply::text("other host"));
    let other_port = other.port;
    let origin = serve("127.0.0.1:0", move |req| match req.route() {
        "/s" => Reply::redirect(&format!("http://localhost:{other_port}/f"))
            .with_header("Set-Cookie", "sid=abc123; Path=/"),
        _ => Reply::not_found(),
    });
    mark_public(&[("127.0.0.1", origin.port), ("localhost", other_port)]);
    let page = render(&format!("http://127.0.0.1:{}/s", origin.port))
        .await
        .expect("the cross-host redirect should render");
    let f = other.hits("/f");
    assert_eq!(f.len(), 1, "hop /f was not requested once: {f:?}");
    assert_eq!(
        f[0].header("cookie"),
        None,
        "a cookie set by 127.0.0.1 was sent to localhost"
    );
    assert!(page.text.contains("other host"), "page: {page:?}");
    cherrypi_fetch::shutdown_renderer();
}
