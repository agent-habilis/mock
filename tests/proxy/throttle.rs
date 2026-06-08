use std::time::Instant;

use agent_habilis_mock::args::Mode;
use http_body_util::Full;
use hyper::Request;
use hyper::body::Bytes;

use crate::helpers::{build_client, make_test_args, read_body, start_echo_server, start_mocker};

// ---------------------------------------------------------------------------
// Synthetic throttle: the response is delivered as a paced trickle over the
// wire, and arrives intact.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn throttled_response_is_paced_and_intact() {
    let echo = start_echo_server().await;
    let origin = format!("http://{}", echo.addr);
    let tmp = tempfile::TempDir::new().unwrap();

    let mut args = make_test_args(&origin, Mode::Pass, tmp.path());
    args.throttle = 2000; // bytes/second

    let mocker = start_mocker(args).await;
    let client = build_client();

    // The echo server reflects the request body, so a sizeable payload makes a
    // response large enough that 2000 B/s pacing is clearly observable.
    let payload = "x".repeat(1500);
    let req = Request::builder()
        .method("POST")
        .uri(format!("http://{}/throttle", mocker.addr))
        .body(Full::new(Bytes::from(payload)))
        .unwrap();

    let start = Instant::now();
    let resp = client.request(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = read_body(resp).await;
    let elapsed = start.elapsed();

    // Body arrives intact (the echo JSON embeds the 1500-byte payload).
    assert!(body.len() >= 1500, "throttled body was truncated");
    // ~1.6 KB at 2000 B/s is ~0.8s; assert it is clearly paced, not instant.
    assert!(
        elapsed.as_millis() >= 400,
        "throttled delivery should be paced; took {elapsed:?}"
    );
}

#[tokio::test]
async fn unthrottled_response_is_not_delayed() {
    let echo = start_echo_server().await;
    let origin = format!("http://{}", echo.addr);
    let tmp = tempfile::TempDir::new().unwrap();

    // Default throttle is 0 (unlimited); the response should be near-instant.
    let args = make_test_args(&origin, Mode::Pass, tmp.path());
    let mocker = start_mocker(args).await;
    let client = build_client();

    let payload = "y".repeat(1500);
    let req = Request::builder()
        .method("POST")
        .uri(format!("http://{}/fast", mocker.addr))
        .body(Full::new(Bytes::from(payload)))
        .unwrap();

    let start = Instant::now();
    let resp = client.request(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = read_body(resp).await;
    let elapsed = start.elapsed();

    assert!(body.len() >= 1500);
    assert!(
        elapsed.as_millis() < 250,
        "unthrottled delivery should be fast; took {elapsed:?}"
    );
}
