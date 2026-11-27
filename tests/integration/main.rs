//! The renderer's integration tests, in one binary, so each one is not linked
//! into its own test executable (Servo is large to link).
//!
//! Servo runs once per process, and the renderer's configuration is fixed by
//! the first render. So every render scenario here shares one renderer with the
//! default configuration: the scenarios run one after another, each in its own
//! task so that a failing assertion is reported by name, and the renderer is
//! shut down once at the end.
//!
//! Two integration tests stay in their own binaries, because they need process
//! state that cannot be shared: `ssrf_positive` (private networks allowed, which
//! is fixed for the whole process) and `proxy_env` (an environment proxy must be
//! set before the renderer starts). The `cli` tests run the binary as a
//! subprocess, so they share this binary without touching the renderer.

mod cli;
#[path = "../common/mod.rs"]
mod common;
mod proxy;
mod redirect;
mod redirect_cookies;
mod redirect_cors;
mod redirect_cross_host;
mod redirect_gzip;
mod redirect_method;
mod redirect_release;
mod redirect_statuses;
mod redirect_subresources;
mod redirect_taint;
mod redirect_userinfo;
mod redirect_xhr;
mod ssrf;

/// Run one scenario in its own task, and record its name if it panicked.
macro_rules! scenario {
    ($failed:ident, $path:path) => {{
        let name = stringify!($path);
        if tokio::spawn($path()).await.is_err() {
            $failed.push(name);
        }
    }};
}

#[tokio::test(flavor = "multi_thread")]
async fn render_scenarios() {
    let mut failed: Vec<&str> = Vec::new();
    scenario!(
        failed,
        proxy::plain_http_is_tunnelled_through_the_checking_proxy
    );
    scenario!(failed, redirect::a_redirect_to_loopback_is_refused);
    scenario!(
        failed,
        redirect_cookies::cookies_are_sent_and_stored_on_every_redirect_hop
    );
    scenario!(
        failed,
        redirect_cors::a_cross_origin_script_fetch_that_redirects_is_refused
    );
    scenario!(
        failed,
        redirect_cross_host::redirects_between_hosts_follow_the_address_check_on_each_hop
    );
    scenario!(
        failed,
        redirect_gzip::compressed_bodies_are_delivered_decoded
    );
    scenario!(
        failed,
        redirect_release::a_same_origin_script_redirect_to_another_origin_follows_cors
    );
    scenario!(
        failed,
        redirect_method::a_redirected_head_request_stays_a_head_request
    );
    scenario!(
        failed,
        redirect_statuses::only_the_five_redirect_statuses_are_followed
    );
    scenario!(
        failed,
        redirect_subresources::redirected_scripts_stylesheets_and_images_apply_in_default_mode
    );
    scenario!(
        failed,
        redirect_taint::a_redirected_cross_origin_stylesheet_is_not_readable
    );
    scenario!(
        failed,
        redirect_userinfo::userinfo_in_a_redirect_location_is_not_sent
    );
    scenario!(
        failed,
        redirect_xhr::a_script_request_redirect_is_checked_on_every_hop
    );
    scenario!(failed, ssrf::blocks_private_and_non_http_destinations);
    cherrypi_fetch::shutdown_renderer();
    assert!(
        failed.is_empty(),
        "these render scenarios failed: {failed:?}"
    );
}
