//! Shared HTTP client with retry/backoff behavior.
//!
//! Mirrors `internal/httpx`. Async via `tokio`/`reqwest`.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use defi_errors::{Code, Error};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE, USER_AGENT as UA};
use serde::de::DeserializeOwned;

/// The User-Agent sent on every request when the caller hasn't set one.
///
/// Part of the wire behavior (Go `defi-cli/1.0`).
pub const USER_AGENT: &str = "defi-cli/1.0";

/// A shared HTTP client that retries transient failures with jittered backoff
/// and maps provider HTTP statuses onto the stable [`defi_errors::Code`] set.
///
/// Mirrors `internal/httpx.Client`.
pub struct Client {
    inner: reqwest::Client,
    retries: u32,
    user_agent: String,
}

/// A decoded JSON response plus the response headers.
///
/// Go returns `(http.Header, error)` and decodes into an out-param; the
/// idiomatic Rust shape carries both the headers and the decoded value.
#[derive(Debug)]
pub struct JsonResponse<T> {
    pub headers: HeaderMap,
    pub value: T,
}

/// The successful outcome of the shared send loop: a 2xx response's headers and
/// raw body bytes. Callers decide whether to decode the body.
struct RawResponse {
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Client {
    /// Build a client with a per-request `timeout` and a retry budget.
    ///
    /// `retries` is the number of *additional* attempts after the first
    /// (Go clamps negatives to 0; the Rust signature uses `u32`, so the clamp
    /// is implicit). Sets the default `User-Agent` to [`USER_AGENT`].
    pub fn new(timeout: Duration, retries: u32) -> Self {
        let inner = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Client {
            inner,
            retries,
            user_agent: USER_AGENT.to_string(),
        }
    }

    /// Perform a request, retrying transient failures, and decode the 2xx body
    /// as JSON into `T`.
    ///
    /// Mirrors Go `(*Client).DoJSON` with a non-nil out-param.
    pub async fn do_json<T: DeserializeOwned>(
        &self,
        req: reqwest::Request,
    ) -> Result<JsonResponse<T>, Error> {
        let raw = self.send_with_retries(req).await?;
        if raw.body.iter().all(|b| b.is_ascii_whitespace()) {
            return Err(Error::new(
                Code::Unavailable,
                "provider returned empty response",
            ));
        }
        let value = serde_json::from_slice::<T>(&raw.body)
            .map_err(|e| Error::wrap(Code::Unavailable, "decode provider JSON", e))?;
        Ok(JsonResponse {
            headers: raw.headers,
            value,
        })
    }

    /// Perform a request, retrying transient failures, and return only the
    /// response headers on 2xx (no body decode).
    ///
    /// Mirrors Go `(*Client).DoJSON` with a nil out-param (status check only).
    pub async fn do_json_discard(&self, req: reqwest::Request) -> Result<HeaderMap, Error> {
        let raw = self.send_with_retries(req).await?;
        Ok(raw.headers)
    }

    /// Drive the retry loop: apply default headers, send the request (cloning it
    /// per attempt), map the HTTP status onto [`Code`], and retry transient
    /// failures (network errors, 429, >=500) until the budget is exhausted.
    ///
    /// On a 2xx response returns the headers and raw body bytes; the caller
    /// decides whether to decode. Mirrors the shared loop in Go `(*Client).DoJSON`.
    async fn send_with_retries(&self, mut req: reqwest::Request) -> Result<RawResponse, Error> {
        apply_default_headers(req.headers_mut(), &self.user_agent);

        let mut last_err: Option<Error> = None;
        for attempt in 0..=self.retries {
            if attempt > 0 {
                tokio::time::sleep(backoff(attempt)).await;
            }

            // Clone the request for this attempt so a retry can re-send it.
            // `try_clone` only returns `None` for streaming bodies, which this
            // client never produces (bodies are always in-memory `Vec<u8>`), so
            // a clone failure is a real internal invariant violation.
            let send_req = match req.try_clone() {
                Some(cloned) => cloned,
                None => {
                    return Err(Error::new(
                        Code::Internal,
                        "cannot retry request with non-clonable body",
                    ))
                }
            };

            match self.inner.execute(send_req).await {
                Err(e) => {
                    last_err = Some(map_net_error(e));
                    if attempt < self.retries {
                        continue;
                    }
                    return Err(last_err.unwrap_or_else(|| {
                        Error::new(Code::Unavailable, "provider request failed")
                    }));
                }
                Ok(resp) => {
                    let status = resp.status();
                    let headers = resp.headers().clone();
                    let code = status.as_u16();

                    // 429: rate limited — retryable.
                    if code == 429 {
                        last_err = Some(Error::new(
                            Code::RateLimited,
                            "provider rate limited request",
                        ));
                        if attempt < self.retries {
                            continue;
                        }
                        return Err(last_err.unwrap_or_else(|| {
                            Error::new(Code::RateLimited, "provider rate limited request")
                        }));
                    }

                    // 401 / 403: auth — terminal, never retried.
                    if code == 401 || code == 403 {
                        return Err(Error::new(Code::Auth, "provider authentication failed"));
                    }

                    // >= 500: unavailable — retryable.
                    if code >= 500 {
                        last_err = Some(Error::new(
                            Code::Unavailable,
                            format!("provider unavailable (status {code})"),
                        ));
                        if attempt < self.retries {
                            continue;
                        }
                        return Err(last_err.unwrap_or_else(|| {
                            Error::new(
                                Code::Unavailable,
                                format!("provider unavailable (status {code})"),
                            )
                        }));
                    }

                    // Other non-2xx (e.g. 3xx, 400, 404): unsupported — terminal.
                    if !(200..300).contains(&code) {
                        return Err(Error::new(
                            Code::Unsupported,
                            format!("provider returned unexpected status {code}"),
                        ));
                    }

                    // 2xx: read the body and hand it back to the caller.
                    let body = resp
                        .bytes()
                        .await
                        .map_err(|e| Error::wrap(Code::Unavailable, "read provider response", e))?;
                    return Ok(RawResponse {
                        headers,
                        body: body.to_vec(),
                    });
                }
            }
        }

        Err(last_err.unwrap_or_else(|| Error::new(Code::Unavailable, "request failed")))
    }
}

/// Apply the default `Accept` and `User-Agent` headers when the caller has not
/// already set them. Mirrors the header defaults in Go `(*Client).DoJSON`.
fn apply_default_headers(headers: &mut HeaderMap, user_agent: &str) {
    if !headers.contains_key(ACCEPT) {
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    }
    if !headers.contains_key(UA) {
        if let Ok(value) = HeaderValue::from_str(user_agent) {
            headers.insert(UA, value);
        }
    }
}

/// Map a `reqwest` transport error onto a transient [`Code::Unavailable`]
/// error. Mirrors Go `mapNetError`: timeouts and other transport failures both
/// surface as `Unavailable` (timeouts get a distinct message).
fn map_net_error(err: reqwest::Error) -> Error {
    if err.is_timeout() {
        Error::wrap(Code::Unavailable, "provider timeout", err)
    } else {
        Error::wrap(Code::Unavailable, "provider request failed", err)
    }
}

/// Compute the jittered exponential backoff for a retry `attempt` (1-based).
///
/// Mirrors Go `backoff`: `120ms * 2^(attempt-1)` capped at `2s`, plus up to
/// `74ms` of random jitter.
fn backoff(attempt: u32) -> Duration {
    let base = Duration::from_millis(120);
    let shift = attempt.saturating_sub(1);
    let mut d = base.saturating_mul(1u32.checked_shl(shift).unwrap_or(u32::MAX));
    let cap = Duration::from_secs(2);
    if d > cap {
        d = cap;
    }
    d + Duration::from_millis(jitter_millis())
}

/// Up to `74ms` of pseudo-random jitter, mirroring Go's `rand.Intn(75)`.
///
/// The jitter is an internal scheduling detail (not part of the wire contract),
/// so it is derived from the wall clock rather than pulling in an RNG crate.
fn jitter_millis() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    nanos % 75
}

/// Build a request from `method`/`url`/`body`/`headers`, send it through
/// `client`, and decode the 2xx body as JSON into `T`.
///
/// Mirrors Go `httpx.DoBodyJSON`: when `body` is `Some`, sets
/// `Content-Type: application/json` (callers may override via `headers`).
pub async fn do_body_json<T: DeserializeOwned>(
    client: &Client,
    method: reqwest::Method,
    url: &str,
    body: Option<Vec<u8>>,
    headers: &HashMap<String, String>,
) -> Result<JsonResponse<T>, Error> {
    let url =
        reqwest::Url::parse(url).map_err(|e| Error::wrap(Code::Internal, "build request", e))?;
    let mut req = reqwest::Request::new(method, url);

    if let Some(bytes) = body {
        req.headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        *req.body_mut() = Some(reqwest::Body::from(bytes));
    }

    for (k, v) in headers {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| Error::wrap(Code::Internal, "build request header name", e))?;
        let value = HeaderValue::from_str(v)
            .map_err(|e| Error::wrap(Code::Internal, "build request header value", e))?;
        req.headers_mut().insert(name, value);
    }

    client.do_json::<T>(req).await
}
