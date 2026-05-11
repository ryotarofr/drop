//! Entry point.
//!
//! Wires together the four moving parts:
//!   * `Store` — on-disk SQLite plus an in-memory cache for the view models.
//!   * `MainWindow` — the Slint UI, driven by view-model properties.
//!   * `Hotkey` — global `Ctrl+Shift+J` that brings the window forward.
//!   * `Tray` — system-tray icon with a Quit menu item.
//!
//! Hotkey and tray events are queued on crate-static channels; a 50 ms
//! Slint timer drains them on the UI thread.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod classifier;
mod command;
mod hotkey;
mod resurface;
mod store;
mod tray;

use anyhow::{Context, Result};
use chrono::{Local, Utc};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use command::Command;
use store::{Entry, Store};
use tray::{Tray, TrayEvent};

slint::include_modules!();

fn main() -> Result<()> {
    // --- Open the store on disk ---------------------------------------
    let path = Store::default_path().context("resolving DB path")?;
    let mut store = Store::open(&path).with_context(|| format!("opening {}", path.display()))?;
    eprintln!("[drop] db = {}", path.display());

    // --- Build initial view models ------------------------------------
    let today_model: Rc<VecModel<TodayView>> = Rc::new(VecModel::from(
        store
            .today(Local::now())
            .into_iter()
            .map(today_view)
            .collect::<Vec<_>>(),
    ));
    let all_model: Rc<VecModel<EntryView>> = Rc::new(VecModel::from(
        store.all().iter().map(entry_view).collect::<Vec<_>>(),
    ));

    let now_utc = Utc::now();
    let resurfaced = store
        .pick_resurface(now_utc)
        .context("picking resurface candidate")?;

    // --- Build the window ---------------------------------------------
    let window = MainWindow::new()?;
    window.set_today_entries(ModelRc::from(today_model.clone()));
    window.set_all_entries(ModelRc::from(all_model.clone()));
    window.set_hotkey_hint(hotkey::hotkey_hint().into());
    window.set_dev_mode(false);
    window.set_status_message("".into());

    if let Some(ref r) = resurfaced {
        window.set_resurface_text(r.text.clone().into());
        window.set_resurface_elapsed(resurface::format_elapsed(r.created_at, now_utc).into());
        window.set_resurface_id_text(id_label(r.id).into());
    } else {
        window.set_resurface_text("".into());
        window.set_resurface_elapsed("".into());
        window.set_resurface_id_text("".into());
    }

    window
        .window()
        .on_close_requested(|| slint::CloseRequestResponse::HideWindow);

    // --- Shared state across callbacks --------------------------------
    // dev_mode lives on the Slint side (`window.get_dev_mode()`); no Rust
    // mirror to keep in sync.
    let store = Rc::new(RefCell::new(store));
    let status_timer = Rc::new(slint::Timer::default());

    // --- Submit (commands + insert) -----------------------------------
    {
        let store = store.clone();
        let today_model = today_model.clone();
        let all_model = all_model.clone();
        let status_timer = status_timer.clone();
        let weak = window.as_weak();

        window.on_submit(move |text| {
            let (dev, search_active, search_query) = match weak.upgrade() {
                Some(w) => (
                    w.get_dev_mode(),
                    w.get_search_active(),
                    w.get_search_query().to_string(),
                ),
                None => (false, false, String::new()),
            };

            match command::parse(&text, dev) {
                Command::Insert(s) => match store.borrow_mut().insert(&s) {
                    Ok(entry) => {
                        // If a filter is active, only show this entry in
                        // the live list when it actually matches.
                        let shown = !search_active
                            || entry
                                .text
                                .to_lowercase()
                                .contains(&search_query.to_lowercase());
                        if shown {
                            all_model.insert(0, entry_view(&entry));
                        }
                        if entry.is_due_today_now(Local::now()) {
                            today_model.insert(0, today_view(entry));
                        }
                        clear_input(&weak);
                    }
                    Err(e) => {
                        flash_status(&weak, &status_timer, &format!("保存失敗: {e}"));
                    }
                },

                Command::DevOn => {
                    if let Some(w) = weak.upgrade() {
                        w.set_dev_mode(true);
                    }
                    clear_input(&weak);
                }

                Command::DevOff => {
                    if let Some(w) = weak.upgrade() {
                        w.set_dev_mode(false);
                    }
                    clear_input(&weak);
                }

                Command::Delete(id) => match store.borrow_mut().delete(id, Utc::now()) {
                    Ok(Some(_entry)) => {
                        let target = id_label(id);
                        remove_by_id_text(&all_model, &target, |e| e.id_text.as_str());
                        remove_by_id_text(&today_model, &target, |e| e.id_text.as_str());
                        if let Some(w) = weak.upgrade() {
                            if w.get_resurface_id_text().as_str() == target {
                                w.set_resurface_text("".into());
                                w.set_resurface_elapsed("".into());
                                w.set_resurface_id_text("".into());
                            }
                        }
                        clear_input(&weak);
                    }
                    Ok(None) => {
                        flash_status(
                            &weak,
                            &status_timer,
                            &format!("{} が見つかりません", id_label(id)),
                        );
                    }
                    Err(e) => {
                        flash_status(&weak, &status_timer, &format!("削除失敗: {e}"));
                    }
                },

                Command::DelUsage => {
                    flash_status(&weak, &status_timer, "/del <ID> で削除");
                }

                Command::DelBadId => {
                    flash_status(&weak, &status_timer, "ID は数字");
                }

                Command::Search(q) => {
                    let views: Vec<EntryView> =
                        store.borrow().search(&q).iter().map(entry_view).collect();
                    all_model.set_vec(views);
                    if let Some(w) = weak.upgrade() {
                        w.set_search_active(true);
                        w.set_search_query(q.into());
                    }
                    // Keep the input as-is so the user can refine the
                    // query by editing in place (e.g. テスト → テス).
                }

                Command::SearchClear => {
                    let views: Vec<EntryView> =
                        store.borrow().all().iter().map(entry_view).collect();
                    all_model.set_vec(views);
                    if let Some(w) = weak.upgrade() {
                        w.set_search_active(false);
                        w.set_search_query("".into());
                    }
                    clear_input(&weak);
                }
            }
        });
    }

    // --- Esc: clear and hide ------------------------------------------
    {
        let weak = window.as_weak();
        window.on_dismiss(move || {
            if let Some(w) = weak.upgrade() {
                w.set_input_text("".into());
                let _ = w.window().hide();
            }
        });
    }

    // --- Hotkey + tray integration ------------------------------------
    let hotkey = match hotkey::Hotkey::install() {
        Ok(h) => {
            eprintln!("[drop] global hotkey: Ctrl+Shift+J");
            Some(h)
        }
        Err(e) => {
            eprintln!("[drop] hotkey install failed (continuing): {e}");
            None
        }
    };
    let tray = match Tray::install() {
        Ok(t) => {
            eprintln!("[drop] tray icon installed");
            Some(t)
        }
        Err(e) => {
            eprintln!("[drop] tray install failed (continuing): {e}");
            None
        }
    };

    let poll_timer = slint::Timer::default();
    {
        let weak = window.as_weak();
        poll_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(50),
            move || {
                if let Some(h) = hotkey.as_ref() {
                    if h.poll() {
                        if let Some(w) = weak.upgrade() {
                            show_window(&w);
                        }
                    }
                }
                if let Some(t) = tray.as_ref() {
                    if let Some(ev) = t.poll() {
                        match ev {
                            TrayEvent::Activated => {
                                if let Some(w) = weak.upgrade() {
                                    show_window(&w);
                                }
                            }
                            TrayEvent::QuitRequested => {
                                let _ = slint::quit_event_loop();
                            }
                        }
                    }
                }
            },
        );
    }

    window.run()?;
    drop(poll_timer);
    Ok(())
}

// --- Window helpers --------------------------------------------------

/// Bring the window to front, restore if minimised, and focus the input.
/// Safe to call whether the window is currently hidden or visible.
fn show_window(w: &MainWindow) {
    let _ = w.window().show();
    #[cfg(target_os = "windows")]
    raise_to_front_windows(w);
    w.invoke_focus_input();
}

/// Slint's `show()` alone doesn't change the OS-level Z-order or
/// foreground state, so reach down to the Win32 API to actually raise
/// the window when responding to a global hotkey or tray click.
#[cfg(target_os = "windows")]
fn raise_to_front_windows(w: &MainWindow) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SW_RESTORE, SetForegroundWindow, ShowWindow,
    };

    // `Window::window_handle()` here returns Slint's wrapper, which in
    // turn implements `HasWindowHandle` — hence the double call.
    let slint_wh = w.window().window_handle();
    let raw_wh = match slint_wh.window_handle() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("[drop] window_handle failed: {e}");
            return;
        }
    };
    if let RawWindowHandle::Win32(win32) = raw_wh.as_raw() {
        let hwnd = win32.hwnd.get() as *mut std::ffi::c_void;
        unsafe {
            // `SW_RESTORE` also un-minimises if needed.
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
    }
}

fn clear_input(weak: &slint::Weak<MainWindow>) {
    if let Some(w) = weak.upgrade() {
        w.set_input_text("".into());
    }
}

/// Set a transient status message in the input footer; clears after 3s.
/// Does NOT touch the input text — callers keep what the user typed on
/// failure so they can edit it.
fn flash_status(weak: &slint::Weak<MainWindow>, timer: &slint::Timer, msg: &str) {
    if let Some(w) = weak.upgrade() {
        w.set_status_message(msg.into());
    }
    let weak2 = weak.clone();
    timer.start(
        slint::TimerMode::SingleShot,
        Duration::from_secs(3),
        move || {
            if let Some(w) = weak2.upgrade() {
                w.set_status_message("".into());
            }
        },
    );
}

/// Find the first row whose extracted key equals `target` and remove it.
fn remove_by_id_text<T, F>(model: &VecModel<T>, target: &str, key_of: F)
where
    T: Clone + 'static,
    F: Fn(&T) -> &str,
{
    let n = model.row_count();
    for i in 0..n {
        if let Some(item) = model.row_data(i) {
            if key_of(&item) == target {
                model.remove(i);
                return;
            }
        }
    }
}

// --- View-model conversion helpers -----------------------------------

/// Short human-typeable label for an entry, e.g. `"#5"`.
fn id_label(id: i64) -> String {
    format!("#{id}")
}

fn entry_view(e: &Entry) -> EntryView {
    EntryView {
        id_text: id_label(e.id).into(),
        time_text: e.time_label().into(),
        body_text: e.text.clone().into(),
    }
}

fn today_view(e: Entry) -> TodayView {
    TodayView {
        id_text: id_label(e.id).into(),
        body_text: e.text.into(),
    }
}
