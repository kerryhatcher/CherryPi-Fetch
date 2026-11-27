//! Redirected subresources must apply (B1). A script, a stylesheet and an image
//! each go through a same-origin and a cross-origin redirect, and every origin
//! is marked public through the test hook, so the only thing under test is that
//! the redirected responses are delivered and applied.
//!
//! Its own test binary: the renderer runs Servo once per process.

use crate::common::Reply;
use cherrypi_fetch::RenderedPage;

/// A 1x1 transparent PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xfc, 0xcf, 0xc0, 0x50,
    0x0f, 0x00, 0x04, 0x85, 0x01, 0x80, 0x84, 0xa9, 0x8c, 0x21, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

const PAGE: &str = "<html><head><title>Redirected subresources</title>\
    <script src=\"/rj\"></script>\
    <script src=\"/rjs\"></script>\
    <link rel=\"stylesheet\" href=\"/rcs\">\
    </head><body>\
    <div id=\"out\">before</div>\
    <div id=\"h2\">HIDDEN2</div>\
    <img id=\"pic\" src=\"/ri\">\
    <script>\
    function report() {\
        var pic = document.getElementById('pic');\
        var img = pic.complete && pic.naturalWidth > 0 ? 'ok' : 'err';\
        document.getElementById('out').textContent = 'js=' + (window.jsA || 'no') + \
            ' jss=' + (window.jsB || 'no') + ' img=' + img;\
    }\
    window.addEventListener('load', report);\
    document.getElementById('pic').addEventListener('error', report);\
    </script></body></html>";

pub async fn redirected_scripts_stylesheets_and_images_apply_in_default_mode() {
    crate::common::setup();

    // The cross-origin target: a script and an image, reached by redirect.
    let target = crate::common::serve("127.0.0.1:0", |req| match req.route() {
        "/j.js" => Reply::ok_with_type(b"window.jsA = 'yes';".to_vec(), "text/javascript"),
        "/p.png" => Reply::ok_with_type(PNG.to_vec(), "image/png"),
        _ => Reply::not_found(),
    });
    let target_port = target.port;

    // The origin: the page, the same-origin targets, and the redirects to both.
    let origin = crate::common::serve("127.0.0.1:0", move |req| match req.route() {
        "/" => Reply::html(PAGE),
        "/rj" => Reply::redirect(&format!("http://127.0.0.1:{target_port}/j.js")),
        "/rjs" => Reply::redirect("/js2.js"),
        "/js2.js" => Reply::ok_with_type(b"window.jsB = 'yes';".to_vec(), "text/javascript"),
        "/rcs" => Reply::redirect("/same.css"),
        // Hides the element, so its text must not reach the snapshot.
        "/same.css" => Reply::ok_with_type(b"#h2 { display: none; }".to_vec(), "text/css"),
        "/ri" => Reply::redirect(&format!("http://127.0.0.1:{target_port}/p.png")),
        _ => Reply::not_found(),
    });
    crate::common::mark_public(&[("127.0.0.1", origin.port), ("127.0.0.1", target_port)]);

    let page: RenderedPage = crate::common::render(&format!("http://127.0.0.1:{}/", origin.port))
        .await
        .expect("the page should render");
    assert!(
        page.text.contains("js=yes") && page.text.contains("jss=yes"),
        "a redirected script did not run: {:?}",
        page.text
    );
    assert!(
        page.text.contains("img=ok"),
        "a redirected image did not load: {:?}",
        page.text
    );
    assert!(
        !page.text.contains("HIDDEN2"),
        "a redirected stylesheet did not apply: {:?}",
        page.text
    );
}
