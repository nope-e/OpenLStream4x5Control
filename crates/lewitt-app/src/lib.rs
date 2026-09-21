//! Application state and platform-neutral adapters for the Iced daemon.

pub mod daemon;
pub mod lifecycle;
pub mod localization;
pub mod logging;
pub mod onboarding;
pub mod single_instance;
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub mod tray;
