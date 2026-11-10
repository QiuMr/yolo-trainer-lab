//! System tray: the app keeps living after the window is closed.
//!
//! A training run can take an hour. Tying the process lifetime to a window means the X
//! button silently kills an hour of work, so X hides instead and the real quit lives in
//! this menu. The tray icon is also the only way back, which is why left-click restores
//! the window rather than opening the menu.

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

/// Label of the single application window, as declared in tauri.conf.json.
pub const MAIN_WINDOW: &str = "main";

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("YOLO Trainer")
        .menu(&menu)
        // Windows convention for a tray of this kind: left click acts, right click opens
        // the menu. Showing the menu on left click too would make "click to restore" a
        // two-step interaction.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => focus_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                focus_main(tray.app_handle());
            }
        });

    // Reuse the window icon rather than shipping a second asset. It is already the
    // generated 32px mark, which is the size Windows asks the tray for.
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}

/// Restore, un-minimise and raise the main window. Safe to call when it is already
/// visible: a second launch of the exe is routed here by the single-instance plugin.
pub fn focus_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
