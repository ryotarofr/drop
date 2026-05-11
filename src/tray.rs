//! System-tray integration.
//!
//! - Builds a small droplet icon procedurally (no PNG to ship).
//! - Right-click on the tray icon opens a menu with a single "終了" item.
//! - Left-click on the icon raises the window via `TrayEvent::Activated`.
//!
//! Events are pushed to crate-static channels; the main event loop polls
//! them on the same cadence as the hotkey.

use anyhow::{Context, Result};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuId, MenuItem},
};

pub enum TrayEvent {
    /// Left-click on the tray icon — bring the window forward.
    Activated,
    /// User picked the "終了" menu item.
    QuitRequested,
}

pub struct Tray {
    _tray: TrayIcon,
    quit_id: MenuId,
}

impl Tray {
    pub fn install() -> Result<Self> {
        let quit_item = MenuItem::new("Drop を終了", true, None);
        let quit_id = quit_item.id().clone();

        let menu = Menu::new();
        menu.append(&quit_item).context("tray menu append")?;

        let tray = TrayIconBuilder::new()
            .with_icon(make_icon()?)
            .with_menu(Box::new(menu))
            .with_tooltip("Drop")
            .build()
            .context("tray build")?;

        Ok(Self {
            _tray: tray,
            quit_id,
        })
    }

    /// Non-blocking. Returns the first interesting event, if any.
    pub fn poll(&self) -> Option<TrayEvent> {
        // Drain menu events first — quit takes precedence.
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == self.quit_id {
                return Some(TrayEvent::QuitRequested);
            }
        }

        // Then look for a left-button-up click on the icon itself.
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                return Some(TrayEvent::Activated);
            }
        }

        None
    }
}

/// Procedurally build a 16x16 RGBA droplet so we don't have to ship an icon
/// file. Tries to evoke the brand droplet without going full pixel-art.
fn make_icon() -> Result<Icon> {
    const SIZE: u32 = 16;
    let mut pixels = vec![0u8; (SIZE * SIZE * 4) as usize];

    let w = SIZE as f32;
    let cx = w / 2.0 - 0.5;
    let cy = w * 0.6;

    for y in 0..SIZE {
        for x in 0..SIZE {
            let fx = x as f32;
            let fy = y as f32;

            // Lower part: filled circle.
            let body_dx = (fx - cx) / (w * 0.42);
            let body_dy = (fy - cy) / (w * 0.42);
            let body_r2 = body_dx * body_dx + body_dy * body_dy;

            // Upper part: triangle-ish taper toward the top.
            let top_taper = ((cy - fy) / cy).clamp(0.0, 1.0);
            let top_width = w * (0.42 - 0.30 * top_taper);
            let in_top = fy < cy && (fx - cx).abs() < top_width;

            let alpha = if body_r2 < 1.0 {
                if body_r2 < 0.75 { 255 } else { 180 }
            } else if in_top {
                220
            } else {
                0
            };

            if alpha > 0 {
                let idx = ((y * SIZE + x) * 4) as usize;
                pixels[idx] = 95; // R
                pixels[idx + 1] = 110; // G
                pixels[idx + 2] = 140; // B
                pixels[idx + 3] = alpha;
            }
        }
    }

    Icon::from_rgba(pixels, SIZE, SIZE).context("tray icon from rgba")
}
