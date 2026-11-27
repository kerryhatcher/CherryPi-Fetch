//! Runs the `cherrypi-fetch` binary against a local page.
#![cfg(feature = "cli")]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;

/// The text is written by a script, so only a real render sees it.
const PAGE: &str = "<html><head><title>CLI Test</title></head><body>\
    <div id=\"app\">loading</div>\
    <script>setTimeout(function(){document.getElementById('app').textContent='cli-rendered-marker ua:'+navigator.userAgent;},200);</script>\
    </body></html>";

fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{PAGE}",
                PAGE.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    format!("http://{addr}/")
}

fn cherrypi_fetch() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cherrypi-fetch"));
    // A proxy in the environment would refuse the render, so remove it unless
    // a test sets one on purpose.
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(name);
    }
    command
}

#[test]
fn renders_to_json_lines_and_reports_failures() {
    let url = serve();
    let output = cherrypi_fetch()
        .args([
            "--json",
            "--timeout",
            "30",
            "--allow-private",
            "--user-agent",
            "cli-test/1.0",
            "--max-chars",
            "200",
            &url,
            "ftp://example.com/",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let records: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is JSON"))
        .collect();
    assert_eq!(records.len(), 2, "stdout: {stdout}");

    let page = &records[0];
    assert_eq!(page["ok"], true, "{page}");
    assert_eq!(page["title"], "CLI Test");
    assert!(page["incomplete"].is_null(), "{page}");
    let text = page["text"].as_str().unwrap();
    assert!(text.contains("cli-rendered-marker"), "{text}");
    assert!(text.contains("ua:cli-test/1.0"), "{text}");

    assert_eq!(records[1]["ok"], false);
    assert!(records[1]["error"].as_str().unwrap().contains("scheme"));
    assert_eq!(output.status.code(), Some(1), "a failed URL exits 1");
}

#[test]
fn blocks_private_addresses_by_default() {
    let url = serve();

    let output = cherrypi_fetch().args(["--json", &url]).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "a private address should fail"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let record: serde_json::Value =
        serde_json::from_str(stdout.lines().next().expect("a JSON record")).unwrap();
    assert_eq!(record["ok"], false, "{record}");
    assert!(
        record["error"].as_str().unwrap().contains("non-public"),
        "{record}"
    );

    let output = cherrypi_fetch()
        .args(["--json", "--allow-private", &url])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn does_not_echo_credentials_for_rejected_urls() {
    for url in [
        "ftp://probeuser:probesecret@example.com/",
        "http://probeuser:probesecret@[bad/",
    ] {
        let output = cherrypi_fetch().args(["--json", url]).output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stdout.contains("probesecret") && !stderr.contains("probesecret"),
            "credentials leaked for {url}: {stdout} {stderr}"
        );
    }
}

#[test]
fn a_proxy_in_the_environment_is_ignored() {
    // The loopback page is refused for its address, not because a proxy is set.
    let url = serve();
    let output = cherrypi_fetch()
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .args(["--json", &url])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("non-public"), "{stdout}");
    assert!(!stdout.contains("HTTP_PROXY"), "{stdout}");
}

#[cfg(unix)]
#[test]
fn does_not_panic_on_a_non_utf8_environment() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let url = serve();
    let output = cherrypi_fetch()
        .env("BADVAR", OsStr::from_bytes(b"\xff\xfe"))
        .args(["--json", &url])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    // The loopback address is still refused, just not by a panic.
    assert_eq!(output.status.code(), Some(1), "{stderr}");
}

#[test]
fn rejects_an_invalid_user_agent() {
    let output = cherrypi_fetch()
        .args(["--user-agent", "", "http://127.0.0.1:9/"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid user agent"));
}

#[test]
fn usage_errors_exit_1_and_help_exits_0() {
    for args in [
        &["--timeout", "nope", "http://127.0.0.1:9/"][..],
        &["--timeout", "0", "http://127.0.0.1:9/"],
        &[],
    ] {
        let output = cherrypi_fetch().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(1), "args {args:?}");
    }
    let help = cherrypi_fetch().arg("--help").output().unwrap();
    assert_eq!(help.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&help.stdout).contains("Exit status"));
}

/// An address that nothing listens on: the port was free a moment ago.
fn closed_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}/")
}

#[test]
fn a_page_that_cannot_load_is_a_failed_render() {
    // Port 9 is a "bad port" that Servo refuses; the other is closed.
    for url in [closed_url(), "http://127.0.0.1:9/".to_string()] {
        let output = cherrypi_fetch()
            .args(["--json", "--allow-private", &url])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let record: serde_json::Value = serde_json::from_str(stdout.trim()).expect("one JSON line");
        assert_eq!(record["ok"], false, "{url}: {record}");
        assert!(record["title"].is_null(), "{url}: {record}");
        assert!(record["text"].is_null(), "{url}: {record}");
        let error = record["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("Could not load the requested page"),
            "{url}: {error}"
        );
        assert_eq!(output.status.code(), Some(1), "{url} exits 1");
    }

    let url = closed_url();
    let output = cherrypi_fetch()
        .args(["--allow-private", &url])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&url) && stderr.contains("Could not load"),
        "{stderr}"
    );
    assert!(String::from_utf8_lossy(&output.stdout).is_empty());
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_after_an_option_explains_itself() {
    let output = cherrypi_fetch().args(["--json", "mcp"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("`mcp` is a subcommand"), "{stderr}");
    assert!(!stderr.contains("invalid URL"), "{stderr}");
}
