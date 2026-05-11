//! Global hotkey registration.
//!
//! Windows binds `Ctrl + Shift + J`. The macOS branch (`Cmd + Shift + J`)
//! is only used for the footer hint string — the macOS build path itself
//! is not exercised here.
//!
//! `global-hotkey` exposes events on a `&'static Receiver`. The Slint
//! event loop polls it on a short interval from `main.rs`.

use anyhow::{Context, Result};
use global_hotkey::{
    GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};

/// Returns the platform-appropriate hint shown in the input footer.
pub fn hotkey_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘⇧J"
    } else {
        "Ctrl+Shift+J"
    }
}

/// Owns the hotkey registration for the lifetime of the app.
/// Dropping the value lets `GlobalHotKeyManager` unregister automatically.
pub struct Hotkey {
    // Held only so its Drop fires when `Hotkey` is dropped.
    _manager: GlobalHotKeyManager,
    id: u32,
}

impl Hotkey {
    pub fn install() -> Result<Self> {
        let manager = GlobalHotKeyManager::new().context("global hotkey manager")?;

        let modifiers = if cfg!(target_os = "macos") {
            Modifiers::META | Modifiers::SHIFT
        } else {
            Modifiers::CONTROL | Modifiers::SHIFT
        };
        let hk = HotKey::new(Some(modifiers), Code::KeyJ);
        let id = hk.id();
        manager
            .register(hk)
            .context("registering global hotkey (Ctrl+Shift+J)")?;

        Ok(Self { _manager: manager, id })
    }

    /// Non-blocking. Drains the receiver; returns true if the hotkey was
    /// pressed (any number of times) since the previous poll.
    pub fn poll(&self) -> bool {
        let receiver = global_hotkey::GlobalHotKeyEvent::receiver();
        let mut fired = false;
        while let Ok(ev) = receiver.try_recv() {
            if ev.id == self.id && ev.state == HotKeyState::Pressed {
                fired = true;
            }
        }
        fired
    }
}
