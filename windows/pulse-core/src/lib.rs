//! Pulse core for Windows — a port of the macOS Pulse data layer.
//!
//! Everything here is platform-independent: the data model, the provider
//! routes, the refresh loop, the cache, the forecast and the `--json`
//! contract. The Windows shell (`pulse-win`) draws on top of it.

pub mod alerts;
pub mod cache;
pub mod http;
pub mod localization;
pub mod model;
pub mod providers;
pub mod report;
pub mod secrets;
pub mod settings;
pub mod store;
pub mod timeutil;

/// The application data directory: `%APPDATA%\Pulse`.
pub fn data_dir() -> std::path::PathBuf {
    let base = std::env::var("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("USERPROFILE").unwrap_or_default();
            std::path::PathBuf::from(home)
                .join("AppData")
                .join("Roaming")
        });
    base.join("Pulse")
}

/// The user's home directory (`%USERPROFILE%`).
pub fn home_dir() -> std::path::PathBuf {
    std::env::var("USERPROFILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("C:\\Users"))
}
