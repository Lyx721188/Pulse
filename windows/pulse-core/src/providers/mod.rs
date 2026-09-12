//! One service per product. Each fetches a `ProviderUsage` by whatever route
//! that product actually offers — documented or borrowed — and invents
//! nothing on the way.

pub mod claude_code;
pub mod codex;
pub mod command_code;
pub mod copilot;
pub mod deepseek;
pub mod device_login;
pub mod grok;
pub mod kimi;
pub mod minimax;
pub mod opencode;
pub mod zai;

use crate::http::HttpClient;
use crate::model::{AccountKey, Provider, ProviderUsage, Unavailability};
use std::sync::Arc;

/// The credentials a pass may need, resolved once per pass rather than once
/// per request.
#[derive(Default)]
pub struct KeyRing {
    pub api_keys: std::collections::HashMap<String, String>,
    pub copilot_token: Option<String>,
}

impl KeyRing {
    pub fn load() -> KeyRing {
        let mut api_keys = std::collections::HashMap::new();
        for provider in crate::model::ALL_PROVIDERS {
            if provider.uses_api_key() {
                if let Some(key) = crate::secrets::key_for(provider.raw()) {
                    api_keys.insert(provider.raw().to_string(), key);
                }
            }
        }
        KeyRing {
            api_keys,
            copilot_token: crate::secrets::key_for("copilot"),
        }
    }

    pub fn api_key(&self, provider: Provider) -> Option<String> {
        self.api_keys.get(provider.raw()).cloned()
    }
}

/// A deep-read of DeepSeek's settings, passed to its service.
#[derive(Clone, Debug, Default)]
pub struct DeepSeekBasis {
    pub basis: String,
    pub budget: Option<f64>,
    pub currency: Option<String>,
}

pub trait ProviderService: Send + Sync {
    fn provider(&self) -> Provider;

    /// The stable `--json` source token for what this service's route is.
    fn origin_token(&self) -> &'static str;

    fn fetch(&self, keys: &KeyRing) -> ProviderUsage;

    /// One account, not the primary one. Not ported yet — added accounts
    /// need Pulse's own OAuth logins.
    fn fetch_account(&self, _keys: &KeyRing, _account: &AccountKey) -> Option<ProviderUsage> {
        None
    }
}

/// A key the user pasted, or the empty string treated as absent — the rule
/// every key service opens with.
pub fn pasted_or_none(key: Option<String>) -> Option<String> {
    key.filter(|k| !k.is_empty())
}

/// What every HTTP-backed service maps a status code to. 401 and 403 mean
/// the credential; the caller refines which credential it was.
pub fn status_reason(status: u16) -> Option<Unavailability> {
    match status {
        401 | 403 => Some(Unavailability::ApiKeyRefused),
        429 => Some(Unavailability::RateLimited),
        s if (500..600).contains(&s) => Some(Unavailability::ServerError),
        _ => Some(Unavailability::ServerError),
    }
}

pub struct Services {
    pub list: Vec<Arc<dyn ProviderService>>,
    pub http: Arc<HttpClient>,
    /// Held concretely because its fetch carries the user's chosen
    /// denominator — the one service whose reading changes meaning with a
    /// setting.
    pub deepseek: Arc<deepseek::DeepSeekService>,
}

impl Services {
    pub fn new() -> Services {
        let http = Arc::new(HttpClient::new());
        let deepseek = Arc::new(deepseek::DeepSeekService::new(http.clone()));
        let list: Vec<Arc<dyn ProviderService>> = vec![
            Arc::new(claude_code::ClaudeCodeService::new(http.clone())),
            Arc::new(codex::CodexService::new(http.clone())),
            Arc::new(copilot::CopilotService::new(http.clone())),
            Arc::new(grok::GrokService::new(http.clone())),
            Arc::new(opencode::OpenCodeService::new(http.clone())),
            Arc::new(kimi::KimiService::new(http.clone())),
            Arc::new(zai::ZaiService::new(http.clone())),
            Arc::new(zai::ZaiService::for_mainland(http.clone())),
            Arc::new(minimax::MinimaxService::new(http.clone())),
            Arc::new(minimax::MinimaxService::for_mainland(http.clone())),
            deepseek.clone(),
            Arc::new(command_code::CommandCodeService::new(http.clone())),
        ];
        Services {
            list,
            http,
            deepseek,
        }
    }

    pub fn for_provider(&self, provider: Provider) -> Option<&Arc<dyn ProviderService>> {
        self.list.iter().find(|s| s.provider() == provider)
    }
}
