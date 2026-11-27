use cherrypi_fetch::{configure, render_page, shutdown_renderer, RendererConfig};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn classifies_main_document_loads() {
    configure(RendererConfig {
        allow_private_networks: true,
        ..Default::default()
    })
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]);
            let (header, body) = if request.starts_with("GET /sandbox ") {
                (
                    "Content-Security-Policy: sandbox\r\n",
                    "<html><body>sandbox success</body></html>".to_string(),
                )
            } else if request.starts_with("GET /origin ") {
                ("", "<html><body>origin success<script>window.origin = 'null';</script></body></html>".to_string())
            } else if request.starts_with("GET /pending ") {
                (
                    "",
                    "<html><body>pending<script>fetch('/held');</script></body></html>".to_string(),
                )
            } else if request.starts_with("GET /held ") {
                // Arm the failure only now, while this request is held open, so
                // the proxy task is certainly still running when it fires.
                cherrypi_fetch::test_fail_with_pending();
                std::thread::sleep(Duration::from_secs(5));
                continue;
            } else if request.starts_with("GET /shadow ") {
                ("", "<html><body>shadow success<script>document.body.querySelector = undefined;</script></body></html>".to_string())
            } else if request.starts_with("GET /accessor ") {
                ("", "<html><body>accessor success<script>Object.defineProperty(Element.prototype, 'querySelector', {get: function () { throw new Error('no'); }});</script></body></html>".to_string())
            } else if request.starts_with("GET /fallback ") {
                ("", "<html><head><title>fallback success</title></head><body>hidden<script>Object.defineProperty(HTMLElement.prototype, 'innerText', {get: function () { return undefined; }}); Object.defineProperty(Node.prototype, 'cloneNode', {get: function () { throw new Error('no'); }});</script></body></html>".to_string())
            } else if request.starts_with("GET /prototype ") {
                ("", "<html><body>prototype success<script>Element.prototype.querySelector = undefined;</script></body></html>".to_string())
            } else {
                ("", "<html><body>iframe success<iframe src='http://127.0.0.1:9/'></iframe></body></html>".to_string())
            };
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n{header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = stream.write_all(response.as_bytes());
        }
    });
    for path in [
        "sandbox",
        "origin",
        "iframe",
        "shadow",
        "prototype",
        "accessor",
    ] {
        let page = render_page(
            format!("http://{addr}/{path}").parse().unwrap(),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert!(page.text.contains("success"), "{path}: {page:?}");
    }
    // innerText is not a string and the fallback throws: the title survives.
    let page = render_page(
        format!("http://{addr}/fallback").parse().unwrap(),
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert_eq!(page.title, "fallback success", "{page:?}");
    for value in ["data:text/html,<p>data success</p>"] {
        assert!(render_page(value.parse().unwrap(), Duration::from_secs(10))
            .await
            .is_ok());
    }
    // A local TLS server presents an untrusted self-signed certificate.
    let tls_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let tls_addr = tls_listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let cert = rustls::pki_types::CertificateDer::from(
            include_bytes!("fixtures/untrusted-cert.der").to_vec(),
        );
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(
            include_bytes!("fixtures/untrusted-key.der").to_vec(),
        );
        let config = rustls::ServerConfig::builder_with_provider(
            rustls::crypto::aws_lc_rs::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key.into())
        .unwrap();
        if let Ok((mut socket, _)) = tls_listener.accept() {
            let _ = socket.set_read_timeout(Some(Duration::from_secs(5)));
            let mut connection =
                rustls::ServerConnection::new(std::sync::Arc::new(config)).unwrap();
            while connection.is_handshaking() {
                if connection.complete_io(&mut socket).is_err() {
                    break;
                }
            }
        }
    });
    let tls_url = format!("https://{tls_addr}/");
    for value in [
        tls_url.as_str(),
        "data:text/plain;base64,%%%",
        "http://127.0.0.1:9/",
        "http://this-domain-does-not-exist.invalid/",
    ] {
        let before = cherrypi_fetch::test_cleanup_count();
        assert!(
            render_page(value.parse().unwrap(), Duration::from_secs(10))
                .await
                .is_err(),
            "{value}"
        );
        assert_eq!(
            cherrypi_fetch::test_cleanup_count(),
            before + 1,
            "cleanup missing for {value}"
        );
    }
    let before = cherrypi_fetch::test_pending_cleanup_counts();
    let error = render_page(
        format!("http://{addr}/pending").parse().unwrap(),
        Duration::from_secs(10),
    )
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("test failure with pending work"),
        "{error}"
    );
    // An aborted task is dropped on the runtime soon after the abort; /held
    // answers only after 5 seconds, so a task that was not aborted cannot be
    // counted within this wait.
    let mut after = cherrypi_fetch::test_pending_cleanup_counts();
    for _ in 0..40 {
        if after.1 > before.1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        after = cherrypi_fetch::test_pending_cleanup_counts();
    }
    shutdown_renderer();
    assert!(after.0 > before.0, "pending load was not cancelled");
    assert!(after.1 > before.1, "pending task was not aborted");
}
