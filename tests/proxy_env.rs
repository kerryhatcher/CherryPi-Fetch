//! A proxy set in the environment is ignored. Servo would otherwise copy it into
//! its own settings and send requests to it, outside the address check.
//!
//! Its own test binary: the renderer runs Servo once per process, and the
//! environment must be set before the first render.

mod common;

use std::time::Duration;

use cherrypi_fetch::{
    configure, render_page, set_test_public_origins, shutdown_renderer, RendererConfig,
};

#[tokio::test(flavor = "multi_thread")]
async fn an_environment_proxy_is_ignored_and_loopback_is_still_refused() {
    // A proxy that does not exist. If Servo used it, the public page below
    // could not load.
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        std::env::set_var(name, "http://127.0.0.1:1");
    }
    configure(RendererConfig {
        user_agent: "proxy-env-test/1.0".into(),
        allow_private_networks: false,
    })
    .expect("configure the renderer");

    let public = common::serve("127.0.0.1:0", |_| {
        common::Reply::html("<html><body>env-ignored-marker</body></html>")
    });
    set_test_public_origins(&[("127.0.0.1", public.port)]);
    let page = render_page(
        url::Url::parse(&format!("http://127.0.0.1:{}/", public.port)).unwrap(),
        Duration::from_secs(30),
    )
    .await
    .expect("the public page should render with a proxy set in the environment");
    assert!(page.text.contains("env-ignored-marker"), "{page:?}");

    // The same server, not marked public: loopback is still refused, and the
    // refusal is about the address, not about the environment.
    set_test_public_origins(&[]);
    let refused = render_page(
        url::Url::parse(&format!("http://127.0.0.1:{}/", public.port)).unwrap(),
        Duration::from_secs(30),
    )
    .await;
    let message = format!("{refused:?}");
    assert!(refused.is_err(), "loopback was rendered: {refused:?}");
    assert!(message.contains("non-public"), "{message}");
    assert!(!message.contains("HTTP_PROXY"), "{message}");
    assert_eq!(
        public.requests().len(),
        1,
        "loopback was requested after the refusal"
    );

    shutdown_renderer();
}
