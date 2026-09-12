//! Encrypted local storage for keys Pulse holds itself, via DPAPI
//! (`CryptProtectData`) — the Windows-native owner-only encryption, the
//! counterpart of the macOS keychain-backed `keys.dat`.
//!
//! One file, `%APPDATA%\Pulse\keys.dat`: a magic header, then a DPAPI blob
//! protecting a JSON object of `provider -> key`. Only this Windows user can
//! decrypt it.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const MAGIC: &[u8; 8] = b"PULSEK1\0";

#[derive(Serialize, Deserialize, Default)]
struct KeyStore {
    /// provider raw value -> pasted key.
    #[serde(flatten)]
    keys: HashMap<String, String>,
}

pub fn key_path() -> std::path::PathBuf {
    crate::data_dir().join("keys.dat")
}

pub fn key_for(provider: &str) -> Option<String> {
    let store = load()?;
    store.keys.get(provider).cloned()
}

pub fn set_key(provider: &str, key: &str) {
    let mut store = load().unwrap_or_default();
    if key.is_empty() {
        store.keys.remove(provider);
    } else {
        store.keys.insert(provider.to_string(), key.to_string());
    }
    save(&store);
}

fn load() -> Option<KeyStore> {
    let data = std::fs::read(key_path()).ok()?;
    let rest = data.strip_prefix(MAGIC)?;
    let plain = dpapi_unprotect(rest)?;
    serde_json::from_slice(&plain).ok()
}

fn save(store: &KeyStore) {
    let dir = crate::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let plain = match serde_json::to_vec(store) {
        Ok(v) => v,
        Err(_) => return,
    };
    let Some(blob) = dpapi_protect(&plain) else {
        return;
    };

    let mut out = Vec::with_capacity(MAGIC.len() + blob.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&blob);
    let tmp = key_path().with_extension("dat.tmp");
    if std::fs::write(&tmp, &out).is_ok() {
        let _ = std::fs::rename(&tmp, key_path());
    }
}

// --- DPAPI via windows-rs is done in the shell crate; the core stays
// platform-neutral by shelling through the two functions below, which
// `pulse-win` wires up at startup.

type CryptFn = fn(&[u8]) -> Option<Vec<u8>>;

static PROTECT: std::sync::OnceLock<CryptFn> = std::sync::OnceLock::new();
static UNPROTECT: std::sync::OnceLock<CryptFn> = std::sync::OnceLock::new();

fn crypt_fallback(_: &[u8]) -> Option<Vec<u8>> {
    None
}

pub fn install_dpapi(
    protect: fn(&[u8]) -> Option<Vec<u8>>,
    unprotect: fn(&[u8]) -> Option<Vec<u8>>,
) {
    let _ = PROTECT.set(protect);
    let _ = UNPROTECT.set(unprotect);
}

fn dpapi_protect(data: &[u8]) -> Option<Vec<u8>> {
    PROTECT.get().unwrap_or(&(crypt_fallback as CryptFn))(data)
}

fn dpapi_unprotect(data: &[u8]) -> Option<Vec<u8>> {
    UNPROTECT.get().unwrap_or(&(crypt_fallback as CryptFn))(data)
}
