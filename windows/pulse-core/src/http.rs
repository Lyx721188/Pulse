//! Shared HTTP plumbing. Every provider rides the same client — system
//! proxy settings included — and maps status codes to the same
//! unavailability vocabulary, with the same two-retry rule for connections
//! that stumble rather than refuse.

use crate::model::Unavailability;
use reqwest::blocking::{Client, ClientBuilder, Response};
use std::time::Duration;

pub struct HttpClient {
    client: Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl HttpClient {
    pub fn new() -> Self {
        let client = ClientBuilder::new()
            // URLSession's default, and the same figure the Swift services
            // set per request.
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(15))
            .user_agent("Pulse/1.1 (Windows)")
            // System proxy settings apply, as on macOS: a machine behind a
            // VPN or corporate proxy reaches these endpoints through it.
            .use_native_tls()
            .build()
            .unwrap_or_else(|_| Client::new());
        HttpClient { client }
    }

    /// Raw client access for the flows that drive requests by hand (the
    /// GitHub device login).
    pub fn client_for_login(&self) -> &Client {
        &self.client
    }

    /// One call, with the retry rule the Swift services share: two extra
    /// attempts, 600ms apart and growing, for errors that look like the
    /// connection tripping rather than the service refusing.
    pub fn fetch_json(
        &self,
        method: Method,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, Unavailability> {
        self.fetch_json_detailed(method, url, headers, body)
            .map_err(|failure| match failure {
                HttpFailure::NotFound => Unavailability::ServerError,
                HttpFailure::Unavailable(reason) => reason,
            })
    }

    /// The same call, keeping 404 distinct — MiniMax's older endpoint path
    /// answers 404 on accounts that are not failures at all.
    pub fn fetch_json_detailed(
        &self,
        method: Method,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, HttpFailure> {
        let mut last_error = Unavailability::Unreachable;
        for attempt in 0..=2 {
            let result = self.fetch_once(method, url, headers, body);
            match result {
                Ok(value) => return Ok(value),
                Err(Outcome::Unavailable(reason)) => {
                    return Err(HttpFailure::Unavailable(reason))
                }
                Err(Outcome::NotFound) => return Err(HttpFailure::NotFound),
                Err(Outcome::Stumble(reason)) => {
                    last_error = reason;
                    if attempt < 2 {
                        std::thread::sleep(Duration::from_millis(600 * (attempt as u64 + 1)));
                    }
                }
            }
        }
        Err(HttpFailure::Unavailable(last_error))
    }

    fn fetch_once(
        &self,
        method: Method,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, Outcome> {
        let mut request = match method {
            Method::Get => self.client.get(url),
            Method::Post => self.client.post(url),
        };
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if let Some(json) = body {
            request = request
                .header("Content-Type", "application/json")
                .json(json);
        }

        let response = match request.send() {
            Ok(r) => r,
            Err(_) => return Err(Outcome::Stumble(Unavailability::Unreachable)),
        };

        let status = response.status();
        if status.as_u16() == 404 {
            return Err(Outcome::NotFound);
        }
        if !status.is_success() {
            let reason = match status.as_u16() {
                401 | 403 => Unavailability::ApiKeyRefused,
                429 => Unavailability::RateLimited,
                _ => Unavailability::ServerError,
            };
            // A 401 from an OAuth endpoint means credentials, not a pasted
            // key; callers refine it, and an Unavailable here ends the call.
            return Err(Outcome::Unavailable(reason));
        }

        let text = match response.text() {
            Ok(t) => t,
            Err(_) => return Err(Outcome::Unavailable(Unavailability::UnreadableReply)),
        };
        serde_json::from_str(&text)
            .map_err(|_| Outcome::Unavailable(Unavailability::UnreadableReply))
    }
}

enum Outcome {
    /// The service answered and the answer is no. Do not retry.
    Unavailable(Unavailability),
    /// A connection stumble; worth a second attempt.
    Stumble(Unavailability),
    /// 404 — some providers treat an older path's absence as a signal.
    NotFound,
}

pub enum HttpFailure {
    NotFound,
    Unavailable(Unavailability),
}

/// Response post-processing shared by the callers that need the status
/// separately from the body (GitHub device flow answers 200 while it waits).
pub fn status_of(response: &Response) -> u16 {
    response.status().as_u16()
}

/// A number that arrived as a string or as a number, interchangeably — the
/// MiniMax rule, and several others drifted into the same shape.
pub fn number(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

pub fn string_field<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(|v| v.as_str())
}

pub fn number_field(value: &serde_json::Value, key: &str) -> Option<f64> {
    value.get(key).and_then(number)
}

pub fn bool_field(value: &serde_json::Value, key: &str) -> Option<bool> {
    value.get(key).and_then(|v| v.as_bool())
}

pub fn array_field<'a>(value: &'a serde_json::Value, key: &str) -> &'a [serde_json::Value] {
    value
        .get(key)
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

pub fn object_field<'a>(
    value: &'a serde_json::Value,
    key: &str,
) -> Option<&'a serde_json::Value> {
    value.get(key).filter(|v| v.is_object())
}
