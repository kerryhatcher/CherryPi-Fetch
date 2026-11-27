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
            } else {
                ("", "<html><body>iframe success<iframe src='http://127.0.0.1:9/'></iframe></body></html>".to_string())
            };
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n{header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = stream.write_all(response.as_bytes());
        }
    });
    for path in ["sandbox", "origin", "iframe"] {
        let page = render_page(
            format!("http://{addr}/{path}").parse().unwrap(),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert!(page.text.contains("success"), "{path}: {page:?}");
    }
    for value in ["data:text/html,<p>data success</p>"] {
        assert!(render_page(value.parse().unwrap(), Duration::from_secs(10))
            .await
            .is_ok());
    }
    for value in [
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
    shutdown_renderer();
}
