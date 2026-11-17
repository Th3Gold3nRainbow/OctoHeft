use crate::shell_integration::ShellIntegration;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Wry,
};

pub struct TrayMenuState {
    pub show_item: MenuItem<Wry>,
    pub hide_item: MenuItem<Wry>,
    pub autostart_item: CheckMenuItem<Wry>,
    pub quit_item: MenuItem<Wry>,
}

struct TrayLabels {
    show: &'static str,
    hide: &'static str,
    autostart: &'static str,
    quit: &'static str,
}

fn tray_labels_for_language(language: &str) -> TrayLabels {
    match language.trim().to_ascii_lowercase().as_str() {
        "fr" => TrayLabels {
            show: "Ouvrir OctoHeft",
            hide: "Réduire dans la zone de notification",
            autostart: "Démarrer avec Windows",
            quit: "Quitter OctoHeft",
        },
        "es" => TrayLabels {
            show: "Mostrar OctoHeft",
            hide: "Ocultar en la bandeja",
            autostart: "Iniciar con Windows",
            quit: "Salir de OctoHeft",
        },
        "de" => TrayLabels {
            show: "OctoHeft anzeigen",
            hide: "In Infoleiste minimieren",
            autostart: "Mit Windows starten",
            quit: "OctoHeft beenden",
        },
        "ar" => TrayLabels {
            show: "إظهار OctoHeft",
            hide: "إخفاء في شريط النظام",
            autostart: "التشغيل تلقائياً مع Windows",
            quit: "إغلاق OctoHeft",
        },
        "ru" => TrayLabels {
            show: "Показать OctoHeft",
            hide: "Свернуть в трей",
            autostart: "Запускать с Windows",
            quit: "Выйти из OctoHeft",
        },
        "zh" => TrayLabels {
            show: "显示 OctoHeft",
            hide: "最小化到托盘",
            autostart: "开机自动启动",
            quit: "退出 OctoHeft",
        },
        _ => TrayLabels {
            show: "Show OctoHeft",
            hide: "Hide to Tray",
            autostart: "Start with Windows",
            quit: "Quit OctoHeft",
        },
    }
}

pub fn sync_tray_autostart_checked(app: &AppHandle, checked: bool) {
    if let Some(state) = app.try_state::<TrayMenuState>() {
        let _ = state.autostart_item.set_checked(checked);
    }
}

pub fn update_tray_language(app: &AppHandle, language: &str) {
    if let Some(state) = app.try_state::<TrayMenuState>() {
        let labels = tray_labels_for_language(language);
        let _ = state.show_item.set_text(labels.show);
        let _ = state.hide_item.set_text(labels.hide);
        let _ = state.autostart_item.set_text(labels.autostart);
        let _ = state.quit_item.set_text(labels.quit);
    }
}

fn restore_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn setup_tray(app: &AppHandle, initial_language: &str) -> Result<(), Box<dyn std::error::Error>> {
    let labels = tray_labels_for_language(initial_language);
    let autostart_enabled = ShellIntegration::is_autostart_enabled();

    let show_i = MenuItem::with_id(app, "show", labels.show, true, None::<&str>)?;
    let hide_i = MenuItem::with_id(app, "hide", labels.hide, true, None::<&str>)?;
    let sep_1 = PredefinedMenuItem::separator(app)?;
    let autostart_i = CheckMenuItem::with_id(
        app,
        "autostart",
        labels.autostart,
        true,
        autostart_enabled,
        None::<&str>,
    )?;
    let sep_2 = PredefinedMenuItem::separator(app)?;
    let quit_i = MenuItem::with_id(app, "quit", labels.quit, true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&show_i, &hide_i, &sep_1, &autostart_i, &sep_2, &quit_i])?;

    app.manage(TrayMenuState {
        show_item: show_i.clone(),
        hide_item: hide_i.clone(),
        autostart_item: autostart_i.clone(),
        quit_item: quit_i.clone(),
    });

    let autostart_for_event = autostart_i.clone();
    let mut builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "show" => {
                restore_main_window(app);
            }
            "hide" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            "autostart" => {
                let current_state = ShellIntegration::is_autostart_enabled();
                let new_state = !current_state;
                match ShellIntegration::set_autostart(new_state) {
                    Ok(_) => {
                        let verified = ShellIntegration::is_autostart_enabled();
                        let _ = autostart_for_event.set_checked(verified);
                        tracing::info!(
                            autostart = verified,
                            "Updated Windows auto-startup state via system tray"
                        );
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed toggling Windows auto-startup from tray");
                        let _ = autostart_for_event.set_checked(current_state);
                    }
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            let should_restore = matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );
            if should_restore {
                restore_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    let _tray = builder.build(app)?;

    Ok(())
}
