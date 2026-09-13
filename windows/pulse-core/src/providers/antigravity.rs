//! Antigravity's limits, read from a language server running on this PC.
//!
//! The odd one out of the seventeen. There is no account endpoint to ask and
//! no stored login to borrow: Antigravity starts a `language_server` process
//! of its own and talks to it over HTTPS on the loopback interface, and that
//! process is the only thing that knows the quota. So this is the one
//! provider whose figures exist **only while something of Antigravity's is
//! running** — which is what `AntigravityNotRunning` says, rather than
//! dressing it up as a failure.
//!
//! Three things have to be found, and not one of them can be assumed:
//! - **the process**, which lives inside the install rather than on `PATH`;
//! - **the port**, because the server is started with `--https_server_port 0`,
//!   meaning "take any free one" — it is a different port on every launch, so
//!   anything hardcoded is wrong by the next restart;
//! - **the CSRF token**, a per-launch UUID passed on the command line. Without
//!   it the server answers `unauthenticated`, and it is the reason this can
//!   only ever read the quota of the Antigravity running as this same user.
//!
//! **More than one process can match, and most of them are the wrong one.**
//! Antigravity runs two language servers and only one answers; the other
//! refuses this RPC outright. Taking the first match and giving up if it did
//! not work was a real fault upstream — measured, the one that answers was
//! second. Every candidate is tried, and every port each of them listens on.
//!
//! Windows route, in place of the Mac's `ps` and `lsof`: the process table
//! for candidates, `NtQueryInformationProcess` for the command line that
//! carries the token, and the owned-TCP table for the ports each pid holds.

use super::{KeyRing, ProviderService};
use crate::model::{AccountKey, Kind, Provider, ProviderUsage, Unavailability, UsageWindow};
use std::time::Duration;

/// The RPCs this uses. Antigravity is built on Codeium's language server,
/// hence the `exa.` package and the `x-codeium-` header.
///
/// Those two are all there is: the quota summary and the plan's name. The
/// spending history and the window estimate other providers get are built
/// from per-model token counts, which do not exist here to be read.
const QUOTA_METHOD: &str = "exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const STATUS_METHOD: &str = "exa.language_server_pb.LanguageServerService/GetUserStatus";
const CSRF_HEADER: &str = "x-codeium-csrf-token";

pub struct AntigravityService;

impl Default for AntigravityService {
    fn default() -> Self {
        Self::new()
    }
}

impl AntigravityService {
    pub fn new() -> Self {
        AntigravityService
    }
}

impl ProviderService for AntigravityService {
    fn provider(&self) -> Provider {
        Provider::Antigravity
    }

    fn origin_token(&self) -> &'static str {
        "languageServer"
    }

    fn fetch(&self, _keys: &KeyRing) -> ProviderUsage {
        let account = AccountKey::primary(Provider::Antigravity);
        let servers = locate_servers();
        if servers.is_empty() {
            return ProviderUsage::unavailable(account, Unavailability::AntigravityNotRunning);
        }

        let http = loopback_client();

        // An answer with no limits in it is a real answer, but not a reason to
        // stop: with two servers up it is what the wrong one says. Held, and
        // reported only if nothing better turns up.
        let mut answered_empty = false;
        // Whether anything answered at all. A server that refused this RPC is
        // still Antigravity running — reporting `Unreachable` for it would say
        // the app is not there while it is, and `Unreachable` is a failure the
        // notifications count while `AntigravityNotRunning` is not.
        let mut something_answered = false;

        for server in &servers {
            // A server listens on more than one port and only one of them
            // speaks this. Which is which isn't advertised, so they are tried.
            for port in &server.ports {
                match ask(&http, *port, &server.token) {
                    Ok(windows) if !windows.is_empty() => {
                        let mut usage = ProviderUsage::live_now(account, windows);
                        usage.origin = Some(self.origin_token().to_string());
                        // A second call, because the quota reply doesn't name
                        // the plan. Its absence is not worth failing over.
                        usage.plan = plan(&http, *port, &server.token);
                        // Antigravity reports a monthly credit *allowance*,
                        // never a balance. Putting an allowance here would
                        // read as "this is what you have left" — the one thing
                        // it isn't.
                        return usage;
                    }
                    Ok(_) => {
                        answered_empty = true;
                        something_answered = true;
                    }
                    Err(AskError::WrongPort) => {}
                    Err(AskError::Refused) | Err(AskError::Unreadable) => {
                        // The other of Antigravity's two servers answers 401 to
                        // this RPC. That is this process saying "not me", not
                        // the account being refused — worth no more than a
                        // closed port, and just as little reason to stop.
                        something_answered = true;
                    }
                }
            }
        }

        if answered_empty {
            return ProviderUsage::unavailable(account, Unavailability::NoLimitsReported);
        }
        ProviderUsage::unavailable(
            account,
            if something_answered {
                Unavailability::AntigravityNotAnswering
            } else {
                Unavailability::AntigravityNotRunning
            },
        )
    }
}

// --- Finding it ---------------------------------------------------------

/// One language server that might answer: the ports it listens on and the
/// per-launch token it demands.
struct Server {
    ports: Vec<u16>,
    token: String,
}

/// Every language server on this PC that might be able to answer, best first.
///
/// Plural, and that is the point: two can be running and only one answers
/// this RPC. A process listening on nothing cannot be asked anything, so it
/// is dropped here rather than being tried and timing out.
fn locate_servers() -> Vec<Server> {
    language_server_processes()
        .into_iter()
        .filter_map(|candidate| {
            let ports = listening_ports(candidate.pid)?;
            Some(Server {
                ports,
                token: candidate.token,
            })
        })
        .collect()
}

struct Candidate {
    pid: u32,
    token: String,
}

/// Every matching process's pid and CSRF token, best product first.
///
/// The candidate's own install has to say Antigravity: `language_server` is
/// Codeium's binary and other editors ship the same one, which would
/// otherwise be asked for Antigravity's quota and answer for something else.
fn language_server_processes() -> Vec<Candidate> {
    let mut found: Vec<(Candidate, bool)> = Vec::new();
    for (pid, _name) in processes_named("language_server") {
        let Some(path) = process_image(pid) else {
            continue;
        };
        let path = path.to_string_lossy().to_lowercase();
        // The app is the product these limits belong to; an "Antigravity IDE"
        // install carries a copy of the same server, and both were measured
        // to answer the same payload — so this ordering costs nothing when
        // only one is running and settles it when both are.
        let is_ide = path.contains("antigravity ide");
        if !path.contains("antigravity") {
            continue;
        }
        let Some(command_line) = command_line(pid) else {
            continue;
        };
        let Some(token) = csrf_token(&command_line) else {
            continue;
        };
        found.push((Candidate { pid, token }, is_ide));
    }
    found.sort_by_key(|(_, is_ide)| *is_ide as u8);
    found.into_iter().map(|(candidate, _)| candidate).collect()
}

/// Splitting on whitespace survives an install path that has spaces in it:
/// the token is still whatever follows the flag.
fn csrf_token(command_line: &str) -> Option<String> {
    let mut fields = command_line.split_whitespace();
    while let Some(field) = fields.next() {
        if field == "--csrf_token" {
            return fields.next().map(|t| t.to_string());
        }
        // `--csrf_token=<token>` is the same flag written joined.
        if let Some(token) = field.strip_prefix("--csrf_token=") {
            return Some(token.to_string());
        }
    }
    None
}

/// Every loopback port the process is listening on, from the owned-TCP table.
fn listening_ports(pid: u32) -> Option<Vec<u16>> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows::Win32::Networking::WinSock::AF_INET;

    let mut size: u32 = 0;
    // The first call only measures. Anything it answers but "buffer too
    // small" is the same as a refusal to answer.
    let _ = unsafe {
        GetExtendedTcpTable(
            None,
            &mut size,
            false,
            AF_INET.0 as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if size == 0 {
        return None;
    }
    // Read as plain DWORDs rather than as `MIB_TCPTABLE_OWNER_PID`: the
    // binding's `table` field is a fixed-size-one array standing in for a
    // variable-length tail, and indexing it by the real entry count walks
    // straight off the end.
    let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
    let code = unsafe {
        GetExtendedTcpTable(
            Some(buffer.as_mut_ptr().cast()),
            &mut size,
            false,
            AF_INET.0 as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if code != 0 {
        return None;
    }

    /// One row is six DWORDs: state, local address, local port, remote
    /// address, remote port, owning pid.
    const ROW_DWORDS: usize = 6;

    let count = ((*buffer.first().unwrap_or(&0)) as usize)
        // The measured size is the contract; the clamp is what a table that
        // grew between the two calls would otherwise walk past.
        .min(buffer.len().saturating_sub(1) / ROW_DWORDS);
    let mut ports = Vec::new();
    for row in buffer[1..1 + count * ROW_DWORDS].chunks_exact(ROW_DWORDS) {
        if row[5] != pid {
            continue;
        }
        // The port arrives in network byte order in the low half.
        ports.push(((row[2] >> 8) | (row[2] << 8)) as u16);
    }
    if ports.is_empty() {
        None
    } else {
        Some(ports)
    }
}

/// The pids of every process whose image name starts with `prefix`.
fn processes_named(prefix: &str) -> Vec<(u32, String)> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut found = Vec::new();
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    let Ok(snapshot) = snapshot else {
        return found;
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut alive = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while alive {
        let name =
            String::from_utf16_lossy(entry.szExeFile.split(|c| *c == 0).next().unwrap_or(&[]));
        if name.to_lowercase().starts_with(prefix) {
            found.push((entry.th32ProcessID, name));
        }
        alive = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(snapshot) };
    found
}

/// The executable's own path — the test that keeps other editors' copies of
/// this binary from being asked Antigravity's question.
fn process_image(pid: u32) -> Option<std::path::PathBuf> {
    use windows::core::PWSTR;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()? };
    let _guard = HandleGuard(handle);
    let mut buffer = [0u16; 1024];
    let mut length = buffer.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
        .ok()?;
    }
    Some(std::path::PathBuf::from(String::from_utf16_lossy(
        &buffer[..length as usize],
    )))
}

/// The process's full command line, read out of its PEB.
///
/// This is where the per-launch CSRF token lives, and there is no friendlier
/// window onto it: `NtQueryInformationProcess` yields the PEB address, and
/// the parameters — command line included — are read out of the process's
/// own memory. Works for anything running as this user, which is exactly the
/// set of servers this provider is allowed to talk to.
fn command_line(pid: u32) -> Option<String> {
    use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_BASIC_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
        RTL_USER_PROCESS_PARAMETERS,
    };

    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
            false,
            pid,
        )
        .ok()?
    };
    let _guard = HandleGuard(handle);

    let mut info = PROCESS_BASIC_INFORMATION::default();
    let mut returned: u32 = 0;
    let status = unsafe {
        NtQueryInformationProcess(
            handle,
            ProcessBasicInformation,
            &mut info as *mut _ as _,
            std::mem::size_of::<PROCESS_BASIC_INFORMATION>() as u32,
            &mut returned,
        )
    };
    if status.is_err() || info.PebBaseAddress.is_null() {
        return None;
    }

    // The parameters pointer sits at offset 0x20 of the PEB on every 64-bit
    // Windows; that is the only byte of the PEB this needs, so it is read as
    // bytes rather than as the (much larger) struct.
    let mut peb_prefix = [0u8; 0x28];
    unsafe {
        ReadProcessMemory(
            handle,
            info.PebBaseAddress.cast(),
            peb_prefix.as_mut_ptr().cast(),
            peb_prefix.len(),
            None,
        )
        .ok()?;
    }
    let parameters_address = usize::from_ne_bytes(peb_prefix[0x20..0x28].try_into().ok()?);
    if parameters_address == 0 {
        return None;
    }

    // The struct's own layout puts `CommandLine` last, which is where the
    // real one sits too — the read is safe to the end of the struct.
    let mut parameters = RTL_USER_PROCESS_PARAMETERS::default();
    unsafe {
        ReadProcessMemory(
            handle,
            parameters_address as *const _,
            &mut parameters as *mut _ as _,
            std::mem::size_of::<RTL_USER_PROCESS_PARAMETERS>(),
            None,
        )
        .ok()?;
    }

    let line = parameters.CommandLine;
    let mut text = vec![0u16; line.Length as usize / 2];
    if text.is_empty() {
        return None;
    }
    unsafe {
        ReadProcessMemory(
            handle,
            line.Buffer.0.cast(),
            text.as_mut_ptr().cast(),
            text.len() * 2,
            None,
        )
        .ok()?;
    }
    Some(String::from_utf16_lossy(&text))
}

/// Opened handles are closed the moment the read is done; the process is
/// never held.
struct HandleGuard(windows::Win32::Foundation::HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

// --- Asking it ----------------------------------------------------------

enum AskError {
    /// This port answered, but not with this service — try the next one.
    WrongPort,
    /// The other of the two servers answers 401 to this RPC: "not me".
    Refused,
    /// A 200 whose body would not decode.
    Unreadable,
}

/// A client for one address only: `https://127.0.0.1`.
///
/// The language server signs its own certificate and nothing can vouch for
/// it, so the usual check is switched off — **on a client that is never
/// pointed anywhere but loopback**, which is the whole of the trust the Mac
/// build extends and all of it this one extends too. A proxy would only be a
/// way to miss a local server, so none is used.
fn loopback_client() -> reqwest::blocking::Client {
    reqwest::blocking::ClientBuilder::new()
        .timeout(Duration::from_secs(6))
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .no_proxy()
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

fn ask(
    http: &reqwest::blocking::Client,
    port: u16,
    token: &str,
) -> Result<Vec<UsageWindow>, AskError> {
    let root = post(http, QUOTA_METHOD, port, token)?;
    let windows = windows_from_reply(&root);
    Ok(windows)
}

/// The plan's name — "Pro", and whatever the other tiers are called.
///
/// `GetUserStatus` answers with a good deal more than this, the account's
/// name and email address among it. Only the plan's name is decoded: the
/// rest is the user's, not ours, and nothing here has any use for it.
fn plan(http: &reqwest::blocking::Client, port: u16, token: &str) -> Option<String> {
    let root = post(http, STATUS_METHOD, port, token).ok()?;
    let name = root
        .pointer("/userStatus/planStatus/planInfo/planName")?
        .as_str()?;
    Some(name.to_string()).filter(|n| !n.is_empty())
}

fn post(
    http: &reqwest::blocking::Client,
    method: &str,
    port: u16,
    token: &str,
) -> Result<serde_json::Value, AskError> {
    let response = http
        .post(format!("https://127.0.0.1:{port}/{method}"))
        .header("Content-Type", "application/json")
        .header(CSRF_HEADER, token)
        .body("{}")
        .send();
    let response = match response {
        Ok(r) => r,
        Err(_) => return Err(AskError::WrongPort),
    };
    match response.status().as_u16() {
        200 => {}
        401 | 403 => return Err(AskError::Refused),
        _ => return Err(AskError::WrongPort),
    }
    let text = response.text().map_err(|_| AskError::Unreadable)?;
    serde_json::from_str(&text).map_err(|_| AskError::Unreadable)
}

// --- Reading the reply --------------------------------------------------

/// The quota reply's shape: groups of buckets, each bucket one timed window
/// of one model group. Held against a payload captured from a real language
/// server — see `Docs/providers/antigravity.md`.
pub fn windows_from_reply(root: &serde_json::Value) -> Vec<UsageWindow> {
    let groups = root
        .pointer("/response/groups")
        .and_then(|g| g.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[]);

    let mut found = Vec::new();
    for group in groups {
        let scope = model_group(crate::http::string_field(group, "displayName"));

        let mut buckets: Vec<UsageWindow> = crate::http::array_field(group, "buckets")
            .iter()
            .filter_map(|bucket| {
                window_from_bucket(bucket, scope.as_deref().map(|s| s.to_string()))
            })
            .collect();
        // Shortest window first within a group, which is the order the other
        // providers' limits arrive in and the order they are useful in: the
        // one about to bite comes first.
        buckets.sort_by_key(|w| w.window_seconds);
        found.extend(buckets);
    }
    found
}

fn window_from_bucket(bucket: &serde_json::Value, scope: Option<String>) -> Option<UsageWindow> {
    let id = crate::http::string_field(bucket, "bucketId")?;
    let remaining = crate::http::number_field(bucket, "remainingFraction")?;
    let (kind, seconds) = length_of(crate::http::string_field(bucket, "window")?)?;

    // The only provider that reports what is *left* rather than what is
    // gone. Everything downstream is in terms of what is gone.
    let mut window = UsageWindow::new(
        id,
        kind,
        scope,
        (1.0 - remaining).clamp(0.0, 1.0),
        seconds,
        crate::http::string_field(bucket, "resetTime").and_then(crate::timeutil::parse_iso8601_ms),
    );
    window.is_exhausted = remaining <= 0.0;
    Some(window)
}

/// A bucket whose window can't be read is left out rather than guessed at.
///
/// `5h` and `weekly` are what the server sends today; the numbered forms are
/// there so a new window length is understood rather than dropped. A window
/// with no length can't be named or sorted, and inventing one would put a
/// figure under a heading that isn't true.
fn length_of(window: &str) -> Option<(Kind, i64)> {
    let window = window.to_lowercase();

    match window.as_str() {
        "5h" => return Some((Kind::FiveHour, 5 * 3_600)),
        "weekly" => return Some((Kind::Weekly, 7 * 86_400)),
        "daily" => return Some((Kind::Other(86_400), 86_400)),
        "monthly" => return Some((Kind::Other(30 * 86_400), 30 * 86_400)),
        _ => {}
    }

    let count: i64 = window[..window.len() - 1].parse().ok()?;
    if count <= 0 {
        return None;
    }
    match window.as_bytes()[window.len() - 1] {
        b'h' => {
            let seconds = count * 3_600;
            Some((
                if count == 5 {
                    Kind::FiveHour
                } else {
                    Kind::Other(seconds)
                },
                seconds,
            ))
        }
        b'd' => {
            let seconds = count * 86_400;
            Some((
                if count == 7 {
                    Kind::Weekly
                } else {
                    Kind::Other(seconds)
                },
                seconds,
            ))
        }
        _ => None,
    }
}

/// "Gemini Models" → "Gemini". The group's name is what the limit is scoped
/// to, and it is shown after the window's own name — "5-hour limit · Gemini"
/// — where the trailing "models" is a word the row can't spare.
fn model_group(name: Option<&str>) -> Option<String> {
    let name = name?.trim();
    if name.is_empty() {
        return None;
    }
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.len() > 1
        && words
            .last()
            .is_some_and(|w| w.eq_ignore_ascii_case("models"))
    {
        return Some(words[..words.len() - 1].join(" "));
    }
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a real language server answered to `RetrieveUserQuotaSummary` on
    /// 2026-09-06 — the same payload the Mac build's fixture test holds, so
    /// the two parsers are provably reading the same reply the same way.
    const CAPTURED: &str = r#"{
      "response": {
        "groups": [
          {
            "displayName": "Gemini Models",
            "description": "Models within this group: Gemini Flash, Gemini Pro",
            "buckets": [
              {
                "bucketId": "gemini-weekly",
                "displayName": "Weekly Limit Remaining",
                "window": "weekly",
                "remainingFraction": 0.9949068,
                "resetTime": "2026-09-11T06:11:46Z"
              },
              {
                "bucketId": "gemini-5h",
                "displayName": "Five Hour Limit Remaining",
                "window": "5h",
                "remainingFraction": 1,
                "resetTime": "2026-09-06T15:20:25Z"
              }
            ]
          },
          {
            "displayName": "Claude and GPT models",
            "buckets": [
              {
                "bucketId": "3p-weekly",
                "window": "weekly",
                "remainingFraction": 1,
                "resetTime": "2026-09-13T10:20:25Z"
              },
              {
                "bucketId": "3p-5h",
                "window": "5h",
                "remainingFraction": 1,
                "resetTime": "2026-09-06T15:20:25Z"
              }
            ]
          }
        ]
      }
    }"#;

    fn captured() -> serde_json::Value {
        serde_json::from_str(CAPTURED).unwrap()
    }

    #[test]
    fn the_captured_reply_becomes_four_windows_shortest_first_within_each_group() {
        let windows = windows_from_reply(&captured());
        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["gemini-5h", "gemini-weekly", "3p-5h", "3p-weekly"]);
        assert_eq!(
            windows.iter().map(|w| w.kind.clone()).collect::<Vec<_>>(),
            [Kind::FiveHour, Kind::Weekly, Kind::FiveHour, Kind::Weekly]
        );
    }

    #[test]
    fn a_groups_name_becomes_the_scope_with_a_trailing_models_trimmed() {
        let windows = windows_from_reply(&captured());
        let scopes: Vec<Option<&str>> = windows.iter().map(|w| w.scope.as_deref()).collect();
        assert_eq!(
            scopes,
            [
                Some("Gemini"),
                Some("Gemini"),
                Some("Claude and GPT"),
                Some("Claude and GPT")
            ]
        );
    }

    #[test]
    fn what_is_left_is_turned_into_what_is_gone_exactly_once() {
        let windows = windows_from_reply(&captured());
        let weekly = windows.iter().find(|w| w.id == "gemini-weekly").unwrap();
        // remainingFraction 0.9949068 in the reply.
        assert!((weekly.used_fraction - 0.0050932).abs() < 0.000_001);
        assert!(!weekly.is_exhausted);

        let untouched = windows.iter().find(|w| w.id == "3p-weekly").unwrap();
        assert_eq!(untouched.used_fraction, 0.0);
    }

    #[test]
    fn reset_times_are_read_as_real_dates() {
        let windows = windows_from_reply(&captured());
        let weekly = windows.iter().find(|w| w.id == "gemini-weekly").unwrap();
        assert_eq!(weekly.resets_at, Some(1_789_107_106_000)); // 2026-09-11T06:11:46Z
    }

    #[test]
    fn nothing_left_is_spent() {
        let reply: serde_json::Value = serde_json::from_str(
            r#"{"response":{"groups":[{"displayName":"Gemini Models","buckets":[
               {"bucketId":"gemini-5h","window":"5h","remainingFraction":0}]}]}}"#,
        )
        .unwrap();
        let window = &windows_from_reply(&reply)[0];
        assert!(window.is_exhausted);
    }

    #[test]
    fn a_bucket_whose_window_cannot_be_read_is_left_out_not_guessed_at() {
        let reply: serde_json::Value = serde_json::from_str(
            r#"{"response":{"groups":[{"displayName":"Gemini Models","buckets":[
               {"bucketId":"mystery","window":"fortnightly","remainingFraction":0.5},
               {"bucketId":"nameless","remainingFraction":0.5},
               {"bucketId":"gemini-5h","window":"5h","remainingFraction":0.5}]}]}}"#,
        )
        .unwrap();
        let windows = windows_from_reply(&reply);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, "gemini-5h");
    }

    #[test]
    fn a_window_length_not_seen_before_is_understood_rather_than_dropped() {
        let reply: serde_json::Value = serde_json::from_str(
            r#"{"response":{"groups":[{"displayName":"Gemini Models","buckets":[
               {"bucketId":"a","window":"3h","remainingFraction":1},
               {"bucketId":"b","window":"7d","remainingFraction":1},
               {"bucketId":"c","window":"30d","remainingFraction":1}]}]}}"#,
        )
        .unwrap();
        let windows = windows_from_reply(&reply);
        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(
            windows.iter().map(|w| w.kind.clone()).collect::<Vec<_>>(),
            [
                Kind::Other(3 * 3_600),
                Kind::Weekly,
                Kind::Other(30 * 86_400)
            ]
        );
    }

    #[test]
    fn an_empty_reply_is_no_windows_not_a_crash() {
        assert!(windows_from_reply(&serde_json::json!({})).is_empty());
        assert!(windows_from_reply(&serde_json::json!({"response":{"groups":[]}})).is_empty());
    }

    #[test]
    fn the_token_is_whatever_follows_the_flag() {
        assert_eq!(
            csrf_token(
                "x.exe --csrf_token 3f9c2a72-8e04-4b39-b6f6-90e33ba96846 --https_server_port 0"
            ),
            Some("3f9c2a72-8e04-4b39-b6f6-90e33ba96846".into())
        );
        assert_eq!(csrf_token("x.exe --csrf_token=abc"), Some("abc".into()));
        assert_eq!(csrf_token("x.exe --https_server_port 0"), None);
    }

    #[test]
    fn a_path_with_spaces_does_not_confuse_the_fields() {
        // The pid is first and the token follows its flag; nothing else about
        // the line matters.
        assert_eq!(
            csrf_token("C:\\Program Files\\Antigravity\\language_server.exe --csrf_token t1"),
            Some("t1".into())
        );
    }

    /// The live route, against whatever language server is running. Ignored
    /// by default — run with `--ignored --nocapture` while Antigravity is
    /// open to see what the real machine answers.
    #[test]
    #[ignore]
    fn live_fetch_against_a_running_language_server() {
        let candidates = language_server_processes();
        println!("candidates: {}", candidates.len());
        for c in &candidates {
            println!(
                "  pid {} token {:?} ports {:?}",
                c.pid,
                c.token,
                listening_ports(c.pid)
            );
        }
        let usage = AntigravityService::new().fetch(&KeyRing::default());
        println!("{usage:#?}");
    }
}
