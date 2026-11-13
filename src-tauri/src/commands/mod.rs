//! Modular Tauri IPC command handlers grouped by domain.

pub mod browser;
pub mod downloads;
pub mod media;
pub mod settings;
pub mod subscriptions;

pub type CmdResult<T> = Result<T, String>;
