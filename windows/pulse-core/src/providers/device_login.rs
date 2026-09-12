//! Signing in to GitHub for Copilot's quota, by device code (RFC 8628).
//!
//! **A sign-in rather than a pasted token, and that is a security decision.**
//! The endpoint accepts any GitHub OAuth token, so asking someone to paste
//! the one `gh` already holds would work — and that token carries `repo` and
//! `workflow`. This asks for `read:user` and nothing else.
//!
//! The flow drives the same public client the VS Code plugin uses; the
//! consent page names the editor rather than Pulse, and this is not an
//! official integration.

use crate::http::HttpClient;
use std::time::Duration;

/// The Copilot plugin's public client. Not a secret — it is in every copy of
/// the extension — and the device flow is designed to be driven without one.
const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const SCOPE: &str = "read:user";
const CODE_URL: &str = "https://github.com/login/device/code";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

/// How long to keep polling before giving up. GitHub's codes last fifteen
/// minutes; stopping sooner would report a failure while the code on screen
/// was still good.
const PATIENCE: Duration = Duration::from_secs(900);

pub struct Prompt {
    pub user_code: String,
    /// Where to send the browser. **The code is not in it, and must not be.**
    /// Pre-filling is the device-code phishing attack: typing the code is
    /// what makes the person consent to *this* device.
    pub verification_url: String,
    pub device_code: String,
    pub interval: Duration,
}

pub enum DeviceLoginError {
    Refused(String),
    Declined,
    TimedOut,
}

impl std::fmt::Display for DeviceLoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceLoginError::Refused(said) => write!(f, "{said}"),
            DeviceLoginError::Declined => {
                write!(f, "{}", crate::localization::t("Sign-in was cancelled."))
            }
            DeviceLoginError::TimedOut => {
                write!(f, "{}", crate::localization::t("The browser didn't come back."))
            }
        }
    }
}

fn post(http: &HttpClient, url: &str, body: &serde_json::Value) -> Result<serde_json::Value, DeviceLoginError> {
    let headers = [
        ("Accept", "application/json"),
        // Without this GitHub answers form-encoded, which parses as nothing.
        ("Content-Type", "application/json"),
    ];
    let header_refs: Vec<(&str, &str)> = headers.iter().copied().collect();

    let text = http
        .client_for_login()
        .post(url)
        .headers(to_header_map(&header_refs))
        .json(body)
        .send();

    let response = text.map_err(|_| {
        DeviceLoginError::Refused(crate::localization::t("The service didn't respond.").to_string())
    })?;

    // The status is read before the body: a refusal served as an HTML error
    // page has plenty to say, and parsing first threw all of it away.
    let status = response.status().as_u16();
    let json: Option<serde_json::Value> = response.json().ok();

    // A device flow answers 200 while it waits, so only a real failure
    // status is one — and even then the body usually names the reason.
    if status >= 400 {
        return Err(DeviceLoginError::Refused(described(
            json.as_ref().unwrap_or(&serde_json::Value::Null),
            url.rsplit('/').next().unwrap_or(url),
            Some(status),
            None,
        )));
    }
    json.ok_or_else(|| {
        DeviceLoginError::Refused(crate::localization::t("Couldn't read the reply.").to_string())
    })
}

fn to_header_map(headers: &[(&str, &str)]) -> reqwest::header::HeaderMap {
    let mut map = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        if let (Ok(n), Ok(v)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(value),
        ) {
            map.insert(n, v);
        }
    }
    map
}

/// Asks GitHub for a code to put on screen.
pub fn start(http: &HttpClient) -> Result<Prompt, DeviceLoginError> {
    let reply = post(
        http,
        CODE_URL,
        &serde_json::json!({ "client_id": CLIENT_ID, "scope": SCOPE }),
    )?;

    let user_code = reply
        .get("user_code")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DeviceLoginError::Refused("login/device/code: no user_code".into()))?
        .to_string();
    let device_code = reply
        .get("device_code")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DeviceLoginError::Refused("login/device/code: no device_code".into()))?
        .to_string();
    let verification = reply
        .get("verification_uri")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com/login/device")
        .to_string();
    let verification = reply
        .get("verification_uri_complete")
        .and_then(|v| v.as_str())
        .unwrap_or(&verification)
        .to_string();

    // Their floor, not ours: polling faster than this earns a `slow_down`.
    let interval = reply
        .get("interval")
        .and_then(|v| v.as_u64())
        .unwrap_or(5)
        .clamp(1, 60);

    Ok(Prompt {
        user_code,
        verification_url: verification,
        device_code,
        interval: Duration::from_secs(interval),
    })
}

/// Polls until the user has finished in the browser, and returns the token.
/// Runs on its own thread; cancellation is the caller dropping it.
pub fn await_token(http: &HttpClient, prompt: &Prompt) -> Result<String, DeviceLoginError> {
    let started = std::time::Instant::now();
    let mut wait = prompt.interval;

    while started.elapsed() < PATIENCE {
        std::thread::sleep(wait);

        let reply = post(
            http,
            TOKEN_URL,
            &serde_json::json!({
                "client_id": CLIENT_ID,
                "device_code": prompt.device_code,
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            }),
        )?;

        if let Some(token) = reply.get("access_token").and_then(|v| v.as_str()) {
            if !token.is_empty() {
                return Ok(token.to_string());
            }
        }

        // The specification's own words, which GitHub does use here.
        match reply.get("error").and_then(|v| v.as_str()) {
            None | Some("authorization_pending") => continue,
            Some("slow_down") => {
                wait = Duration::from_secs(
                    reply
                        .get("interval")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(10)
                        .clamp(1, 60),
                );
            }
            Some("access_denied") => return Err(DeviceLoginError::Declined),
            Some("expired_token") => return Err(DeviceLoginError::TimedOut),
            Some(error) => {
                return Err(DeviceLoginError::Refused(described(
                    &reply,
                    "login/oauth/access_token",
                    None,
                    Some(error),
                )))
            }
        }
    }

    Err(DeviceLoginError::TimedOut)
}

/// GitHub's own words where it has them: "device_flow_disabled" says
/// something a generic failure cannot.
fn described(
    reply: &serde_json::Value,
    step: &str,
    status: Option<u16>,
    error: Option<&str>,
) -> String {
    let said = reply
        .get("error_description")
        .and_then(|v| v.as_str())
        .or(error)
        .or_else(|| reply.get("error").and_then(|v| v.as_str()));
    let prefix = match status {
        Some(status) => format!("{step}: HTTP {status}"),
        None => step.to_string(),
    };
    match said {
        Some(said) => format!("{prefix} — {said}"),
        None => prefix,
    }
}
