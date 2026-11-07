#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use boost_core::{Device, Engine, Meter};
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, WindowEvent};

#[derive(Default)]
struct App(Mutex<Option<Engine>>);

#[derive(serde::Serialize)]
struct Status {
    running: bool,
    output: Option<String>,
    platform: &'static str,
}

fn status(e: &Option<Engine>) -> Status {
    Status {
        running: e.as_ref().map(|e| e.is_running()).unwrap_or(false),
        output: e.as_ref().map(|e| e.output_id().to_string()),
        platform: std::env::consts::OS,
    }
}

#[tauri::command]
fn outputs() -> Vec<Device> {
    Engine::outputs()
}

#[tauri::command]
fn start(app: State<App>, output: Option<String>, gain: f32) -> Result<Status, String> {
    let mut g = app.0.lock().unwrap();
    if let Some(mut old) = g.take() {
        old.stop();
    }
    *g = Some(Engine::start(output, gain)?);
    Ok(status(&g))
}

#[tauri::command]
fn stop(app: State<App>) -> Status {
    let mut g = app.0.lock().unwrap();
    if let Some(mut e) = g.take() {
        e.stop();
    }
    status(&g)
}

#[tauri::command]
fn set_gain(app: State<App>, gain: f32) {
    if let Some(e) = app.0.lock().unwrap().as_ref() {
        e.set_gain(gain);
    }
}

#[tauri::command]
fn set_muted(app: State<App>, muted: bool) {
    if let Some(e) = app.0.lock().unwrap().as_ref() {
        e.set_muted(muted);
    }
}

#[tauri::command]
fn meter(app: State<App>) -> Option<Meter> {
    app.0.lock().unwrap().as_ref().map(|e| e.meter())
}

#[tauri::command]
fn state(app: State<App>) -> Status {
    status(&app.0.lock().unwrap())
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn main() {
    tauri::Builder::default()
        .manage(App::default())
        .setup(|app| {
            let show = MenuItem::with_id(app, "show", "Show Audio Boost", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit (restore normal audio)", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Audio Boost")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| match ev.id.as_ref() {
                    "show" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        // closing the window only hides it; the audio engine keeps running
        .on_window_event(|w, ev| {
            if let WindowEvent::CloseRequested { api, .. } = ev {
                api.prevent_close();
                let _ = w.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![outputs, start, stop, set_gain, set_muted, meter, state])
        .build(tauri::generate_context!())
        .expect("error while building Audio Boost")
        .run(|handle, event| {
            // always hand audio back to the real device on exit
            if let tauri::RunEvent::Exit = event {
                if let Some(mut e) = handle.state::<App>().0.lock().unwrap().take() {
                    e.stop();
                }
            }
        });
}
