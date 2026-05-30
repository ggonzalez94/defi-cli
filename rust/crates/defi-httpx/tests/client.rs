//! RED-phase tests for `defi-httpx` (Go source: `internal/httpx`).
//!
//! ============================================================================
//! SUCCESS CRITERIA
//!
//! This crate owns the shared HTTP client: retry/backoff plus the mapping from
//! provider HTTP status onto the stable `defi_errors::Code` set (spec §2.2).
//! The Rust port is "correct" iff:
//!
//!  1. CONSTRUCTION. `Client::new(timeout, retries)` builds a usable client.
//!     `retries` is the count of *additional* attempts after the first
//!     (Go clamps negatives to 0; the `u32` Rust signature makes that
//!     structurally impossible, so there is no separate clamp test).
//!
//!  2. DEFAULT HEADERS. When the caller has not set them, requests carry
//!     `Accept: application/json` and `User-Agent: defi-cli/1.0`. A
//!     caller-provided `Accept`/`User-Agent` is preserved (not overwritten).
//!
//!  3. SUCCESS DECODE. A 2xx JSON body decodes into the target type and the
//!     response headers are returned to the caller.
//!
//!  4. RETRY on 5xx. With `retries >= 1`, a first 5xx followed by a 2xx
//!     succeeds and decodes the second body. (Ported from
//!     `internal/httpx/client_test.go::TestDoJSONRetriesServerError`.)
//!
//!  5. RETRY on 429. A first 429 followed by a 2xx succeeds with `retries >= 1`.
//!
//!  6. RETRY on network failure. A connection error followed by a 2xx succeeds
//!     with `retries >= 1` (recover path, distinct from the 5xx/429 retry
//!     branch); and an exhausted-retry connection error maps to
//!     Code::Unavailable (terminal path).
//!
//!  7. STATUS → CODE MAP (no-retry / exhausted-retry terminal cases):
//!       - 401, 403           → Code::Auth        (NEVER retried)
//!       - 429 (exhausted)    → Code::RateLimited
//!       - >= 500 (exhausted) → Code::Unavailable
//!       - other non-2xx (e.g. 400, 404, 3xx) → Code::Unsupported (NOT retried)
//!
//!  8. NO RETRY when budget is 0. With `retries = 0`, a single 5xx yields
//!     Code::Unavailable after exactly ONE request (no second hit).
//!
//!  9. AUTH IS TERMINAL. 401/403 are returned immediately even when a retry
//!     budget remains: exactly one request is made.
//!
//! 10. EMPTY BODY. A 2xx response whose body is empty/whitespace yields
//!     Code::Unavailable ("empty response") when a decode target is expected.
//!
//! 11. INVALID JSON. A 2xx response with non-JSON body yields Code::Unavailable
//!     ("decode" failure).
//!
//! 12. DISCARD (out == nil). `do_json_discard` returns the headers on 2xx and
//!     does NOT require a body (empty 2xx body is fine), mirroring Go's
//!     `out == nil` early return.
//!
//! 13. do_body_json: sends the method/url/body; when a body is present sets
//!     `Content-Type: application/json`; applies caller headers; decodes 2xx
//!     JSON. A caller header overrides the default `Content-Type`.
//!
//! Go httptest servers are mapped to `wiremock`. Tests that assert *number of
//! requests* prove the retry/no-retry behavior deterministically. Backoff
//! TIMING/JITTER is an internal implementation detail and is intentionally NOT
//! asserted (only that retries happen / don't happen).
//! ============================================================================

use std::collections::HashMap;
use std::time::Duration;

use defi_errors::Code;
use defi_httpx::{do_body_json, Client};
use serde::Deserialize;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Debug, Deserialize, PartialEq)]
struct Ok {
    ok: bool,
}

fn req(server: &MockServer, path_str: &str) -> reqwest::Request {
    reqwest::Request::new(
        reqwest::Method::GET,
        format!("{}{}", server.uri(), path_str).parse().unwrap(),
    )
}

// ---- Criterion 3: success decode + headers returned ------------------------

#[tokio::test]
async fn do_json_decodes_2xx_body_and_returns_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ok"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-trace", "abc")
                .set_body_string(r#"{"ok":true}"#),
        )
        .mount(&server)
        .await;

    let client = Client::new(Duration::from_secs(2), 0);
    let resp = client
        .do_json::<Ok>(req(&server, "/ok"))
        .await
        .expect("2xx JSON should decode");
    assert_eq!(resp.value, Ok { ok: true });
    assert_eq!(
        resp.headers.get("x-trace").map(|v| v.to_str().unwrap()),
        Some("abc"),
        "response headers must be surfaced to the caller"
    );
}

// ---- Criterion 2: default + preserved headers ------------------------------

#[tokio::test]
async fn sets_default_accept_and_user_agent_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/h"))
        .and(header("accept", "application/json"))
        .and(header("user-agent", "defi-cli/1.0"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .mount(&server)
        .await;

    let client = Client::new(Duration::from_secs(2), 0);
    client
        .do_json::<Ok>(req(&server, "/h"))
        .await
        .expect("default Accept + User-Agent must be applied");
}

#[tokio::test]
async fn preserves_caller_supplied_user_agent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ua"))
        .and(header("user-agent", "custom-agent/9"))
        // Reject the default UA: if the client overwrote the caller value with
        // `defi-cli/1.0`, this matcher would not match and the request would
        // 404, failing the `.expect(...)` below.
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .mount(&server)
        .await;

    let client = Client::new(Duration::from_secs(2), 0);
    let mut request = req(&server, "/ua");
    request
        .headers_mut()
        .insert("user-agent", "custom-agent/9".parse().unwrap());
    client
        .do_json::<Ok>(request)
        .await
        .expect("caller User-Agent must be preserved, not overwritten");
}

#[tokio::test]
async fn preserves_caller_supplied_accept() {
    // Criterion 2 (Accept half): a caller-set `Accept` must NOT be overwritten
    // by the default `application/json`. The matcher requires the caller value;
    // if the client clobbered it, the request would 404 and the decode would
    // fail. Mirrors the User-Agent preservation test for the Accept header.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/accept"))
        .and(header("accept", "application/vnd.custom+json"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .mount(&server)
        .await;

    let client = Client::new(Duration::from_secs(2), 0);
    let mut request = req(&server, "/accept");
    request
        .headers_mut()
        .insert("accept", "application/vnd.custom+json".parse().unwrap());
    client
        .do_json::<Ok>(request)
        .await
        .expect("caller Accept must be preserved, not overwritten");
}

// ---- Criterion 4: retry on 5xx (ported from TestDoJSONRetriesServerError) --

#[tokio::test]
async fn retries_once_on_server_error_then_succeeds() {
    let server = MockServer::start().await;
    // First response: 500. Mounted with up_to_n_times(1) so it only matches once.
    Mock::given(method("GET"))
        .and(path("/retry5xx"))
        .respond_with(ResponseTemplate::new(500).set_body_string(r#"{"error":"x"}"#))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    // Second response: 200.
    Mock::given(method("GET"))
        .and(path("/retry5xx"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::new(Duration::from_secs(2), 1);
    let resp = client
        .do_json::<Ok>(req(&server, "/retry5xx"))
        .await
        .expect("a 5xx then 2xx must succeed with retries=1");
    assert_eq!(resp.value, Ok { ok: true });
}

// ---- Criterion 5: retry on 429 ---------------------------------------------

#[tokio::test]
async fn retries_once_on_429_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/retry429"))
        .respond_with(ResponseTemplate::new(429))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/retry429"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::new(Duration::from_secs(2), 1);
    let resp = client
        .do_json::<Ok>(req(&server, "/retry429"))
        .await
        .expect("a 429 then 2xx must succeed with retries=1");
    assert_eq!(resp.value, Ok { ok: true });
}

// ---- Criterion 6: retry on network failure ---------------------------------

#[tokio::test]
async fn network_error_exhausted_maps_to_unavailable() {
    // Terminal network-failure path: a request to an unreachable address
    // (connection refused) is a transport error, and with the retry budget
    // exhausted it yields Code::Unavailable (mirrors Go `mapNetError`).
    let unreachable = "http://127.0.0.1:1"; // port 1: connection refused
    let client = Client::new(Duration::from_millis(300), 1);
    let request = reqwest::Request::new(reqwest::Method::GET, unreachable.parse().unwrap());
    let err = client
        .do_json::<Ok>(request)
        .await
        .expect_err("an unreachable host must error after exhausting retries");
    assert_eq!(
        err.code,
        Code::Unavailable,
        "network failure must map to Unavailable"
    );
}

#[tokio::test]
async fn retries_on_network_error_then_succeeds() {
    // The RECOVER half of criterion 6: a transport-level failure on the first
    // attempt followed by a successful 2xx on the retry must succeed. This is a
    // DISTINCT code path from the 5xx/429 retries (those re-loop from the
    // `Ok(resp)` arm; this re-loops from the `Err(transport)` arm). Without this
    // test, mutating the `continue` in the network-error branch goes undetected.
    //
    // Deterministic harness: a one-shot TCP server that, on the FIRST
    // connection, accepts and immediately closes the socket with no HTTP
    // response (reqwest surfaces this as a transport error, not a status), then
    // on the SECOND connection serves a valid 200 JSON response.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        // First connection: accept, then drop immediately (no bytes written) ->
        // client sees "connection closed before message completed".
        let (first, _) = listener.accept().await.unwrap();
        drop(first);

        // Second connection: read the request, then write a minimal HTTP/1.1
        // 200 response with a JSON body.
        let (mut second, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        // Read the request line/headers (best effort; we don't parse them).
        let _ = second.read(&mut buf).await.unwrap();
        let body = r#"{"ok":true}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        second.write_all(resp.as_bytes()).await.unwrap();
        second.flush().await.unwrap();
        // Hold the connection open briefly so the client reads the full body.
        second.shutdown().await.ok();
    });

    let client = Client::new(Duration::from_secs(2), 1);
    let url = format!("http://{addr}/recover");
    let request = reqwest::Request::new(reqwest::Method::GET, url.parse().unwrap());
    let resp = client
        .do_json::<Ok>(request)
        .await
        .expect("a transport error then a 2xx must succeed with retries=1");
    assert_eq!(resp.value, Ok { ok: true });

    server.await.unwrap();
}

// ---- Criterion 7: status → code map ----------------------------------------

async fn status_maps_to(status: u16, expected: Code) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/s"))
        .respond_with(ResponseTemplate::new(status))
        .mount(&server)
        .await;
    let client = Client::new(Duration::from_secs(2), 0);
    let err = client
        .do_json::<Ok>(req(&server, "/s"))
        .await
        .expect_err("non-2xx must error");
    assert_eq!(
        err.code, expected,
        "status {status} must map to {expected:?}"
    );
}

#[tokio::test]
async fn status_401_maps_to_auth() {
    status_maps_to(401, Code::Auth).await;
}

#[tokio::test]
async fn status_403_maps_to_auth() {
    status_maps_to(403, Code::Auth).await;
}

#[tokio::test]
async fn status_429_exhausted_maps_to_rate_limited() {
    status_maps_to(429, Code::RateLimited).await;
}

#[tokio::test]
async fn status_500_exhausted_maps_to_unavailable() {
    status_maps_to(500, Code::Unavailable).await;
}

#[tokio::test]
async fn status_503_exhausted_maps_to_unavailable() {
    status_maps_to(503, Code::Unavailable).await;
}

#[tokio::test]
async fn status_400_maps_to_unsupported() {
    status_maps_to(400, Code::Unsupported).await;
}

#[tokio::test]
async fn status_404_maps_to_unsupported() {
    status_maps_to(404, Code::Unsupported).await;
}

// ---- Criterion 8: no retry when budget is 0 --------------------------------

#[tokio::test]
async fn no_retry_when_budget_zero() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/once"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1) // EXACTLY one request — retries=0 means no second hit.
        .mount(&server)
        .await;

    let client = Client::new(Duration::from_secs(2), 0);
    let err = client
        .do_json::<Ok>(req(&server, "/once"))
        .await
        .expect_err("500 with retries=0 must error");
    assert_eq!(err.code, Code::Unavailable);
    // Drop the server: its `.expect(1)` verifies request count on teardown.
    drop(server);
}

// ---- Criterion 9: auth is terminal (not retried) ---------------------------

#[tokio::test]
async fn auth_status_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/auth"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1) // even with retries budget remaining, only ONE request.
        .mount(&server)
        .await;

    let client = Client::new(Duration::from_secs(2), 3);
    let err = client
        .do_json::<Ok>(req(&server, "/auth"))
        .await
        .expect_err("401 must error");
    assert_eq!(err.code, Code::Auth);
    drop(server);
}

#[tokio::test]
async fn unsupported_status_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/nope"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1) // 4xx (non-429/401/403) is terminal: one request only.
        .mount(&server)
        .await;

    let client = Client::new(Duration::from_secs(2), 3);
    let err = client
        .do_json::<Ok>(req(&server, "/nope"))
        .await
        .expect_err("404 must error");
    assert_eq!(err.code, Code::Unsupported);
    drop(server);
}

// ---- Criterion 10: empty body ----------------------------------------------

#[tokio::test]
async fn empty_2xx_body_maps_to_unavailable_when_decoding() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/empty"))
        .respond_with(ResponseTemplate::new(200).set_body_string("   "))
        .mount(&server)
        .await;
    let client = Client::new(Duration::from_secs(2), 0);
    let err = client
        .do_json::<Ok>(req(&server, "/empty"))
        .await
        .expect_err("empty 2xx body must error when a decode target is expected");
    assert_eq!(err.code, Code::Unavailable);
}

// ---- Criterion 11: invalid JSON --------------------------------------------

#[tokio::test]
async fn invalid_json_2xx_maps_to_unavailable() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/badjson"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json at all"))
        .mount(&server)
        .await;
    let client = Client::new(Duration::from_secs(2), 0);
    let err = client
        .do_json::<Ok>(req(&server, "/badjson"))
        .await
        .expect_err("invalid JSON on 2xx must error");
    assert_eq!(err.code, Code::Unavailable);
}

// ---- Criterion 12: discard (out == nil) ------------------------------------

#[tokio::test]
async fn discard_returns_headers_on_2xx_with_empty_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/discard"))
        .respond_with(
            ResponseTemplate::new(204).insert_header("x-id", "42"), // empty body, no decode required
        )
        .mount(&server)
        .await;
    let client = Client::new(Duration::from_secs(2), 0);
    let headers = client
        .do_json_discard(req(&server, "/discard"))
        .await
        .expect("status-only request must succeed without a body");
    assert_eq!(headers.get("x-id").map(|v| v.to_str().unwrap()), Some("42"));
}

#[tokio::test]
async fn discard_still_maps_error_statuses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/discard-auth"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let client = Client::new(Duration::from_secs(2), 0);
    let err = client
        .do_json_discard(req(&server, "/discard-auth"))
        .await
        .expect_err("403 must error even with no decode target");
    assert_eq!(err.code, Code::Auth);
}

// ---- Criterion 13: do_body_json --------------------------------------------

#[tokio::test]
async fn do_body_json_posts_body_with_json_content_type() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/post"))
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .mount(&server)
        .await;

    let client = Client::new(Duration::from_secs(2), 0);
    let url = format!("{}/post", server.uri());
    let resp = do_body_json::<Ok>(
        &client,
        reqwest::Method::POST,
        &url,
        Some(br#"{"q":1}"#.to_vec()),
        &HashMap::new(),
    )
    .await
    .expect("body POST should set Content-Type and decode the response");
    assert_eq!(resp.value, Ok { ok: true });
}

#[tokio::test]
async fn do_body_json_applies_caller_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/post-auth"))
        .and(header("authorization", "Bearer xyz"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .mount(&server)
        .await;

    let client = Client::new(Duration::from_secs(2), 0);
    let url = format!("{}/post-auth", server.uri());
    let mut headers = HashMap::new();
    headers.insert("Authorization".to_string(), "Bearer xyz".to_string());
    do_body_json::<Ok>(
        &client,
        reqwest::Method::POST,
        &url,
        Some(br#"{"q":1}"#.to_vec()),
        &headers,
    )
    .await
    .expect("caller-supplied headers must be applied to the request");
}

#[tokio::test]
async fn do_body_json_caller_header_overrides_default_content_type() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/post-ct"))
        .and(header("content-type", "application/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .mount(&server)
        .await;

    let client = Client::new(Duration::from_secs(2), 0);
    let url = format!("{}/post-ct", server.uri());
    let mut headers = HashMap::new();
    headers.insert(
        "Content-Type".to_string(),
        "application/graphql".to_string(),
    );
    do_body_json::<Ok>(
        &client,
        reqwest::Method::POST,
        &url,
        Some(br#"query{}"#.to_vec()),
        &headers,
    )
    .await
    .expect("a caller Content-Type header must override the default");
}
