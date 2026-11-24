// proxydock-ui.exe: the ProxyDock window. Started by proxydock.exe (the tray + core process) with
// PROXYDOCK_IPC="<port> <token>"; it draws what the core sends and sends the user's commands back. Closing the
// window ends this process; the cards keep running in the core.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use proxydock::ipc;
use proxydock_core::i18n::{set_language, tr};
use proxydock_core::log::fmt_size;
use serde_json::{json, Value};
use slint::{Color, ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::net::TcpStream;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

slint::include_modules!();

const LOG_CAP: usize = 5000;

fn state_text(state: &str) -> String {
    match state {
        "running" => tr("Running"),
        "starting" => tr("Starting…"),
        "error" => tr("error"),
        _ => tr("Stopped"),
    }
}

fn card_row(c: &Value) -> CardRow {
    let s = |k: &str| SharedString::from(c[k].as_str().unwrap_or(""));
    let state = c["state"].as_str().unwrap_or("stopped");
    let on = matches!(state, "running" | "starting");
    CardRow {
        id: s("id"),
        name: s("name"),
        summary: s("summary"),
        state: state.into(),
        state_text: state_text(state).into(),
        error: if state == "error" { s("error") } else { SharedString::new() },
        traffic: SharedString::new(),
        action: if on { tr("Stop") } else { tr("Run") }.into(),
    }
}

fn log_row(e: &Value) -> LogRow {
    let s = |k: &str| e[k].as_str().unwrap_or("");
    let name = if s("name").is_empty() { String::new() } else { format!("[{}] ", s("name")) };
    let level = s("level");
    let tone = match level {
        "FAIL" | "AUTH" | "BLOCK" | "ERROR" => 2,
        "WARN" => 1,
        "TCP" | "UDP" => 3,
        _ => 0,
    };
    LogRow { text: format!("[{}] {:<5} {name}{}", s("time"), level, s("message")).into(), tone }
}

fn traffic_text(st: &Value) -> String {
    let n = |k: &str| st[k].as_u64().unwrap_or(0);
    if n("total") == 0 && n("up") == 0 && n("down") == 0 {
        return String::new();
    }
    format!("↑ {}  ↓ {}  ·  TCP {}  UDP {}", fmt_size(n("up") as f64), fmt_size(n("down") as f64), n("tcp"), n("udp"))
}

/// Windows 11: caption, border and caption text in the app's colours (ignored before Windows 11)
fn apply_title_bar(w: &App) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
        let slint_handle = w.window().window_handle();
        let Ok(handle) = slint_handle.window_handle() else { return };
        let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
        let hwnd = h.hwnd.get() as *mut core::ffi::c_void;
        let colorref = |c: Color| (c.red() as u32) | ((c.green() as u32) << 8) | ((c.blue() as u32) << 16);
        let set = |attr: u32, value: u32| unsafe {
            DwmSetWindowAttribute(hwnd, attr, &value as *const u32 as *const _, 4);
        };
        set(20, w.get_dark() as u32); // USE_IMMERSIVE_DARK_MODE
        set(35, colorref(w.get_caption_color())); // CAPTION_COLOR
        set(34, colorref(w.get_caption_color())); // BORDER_COLOR
        set(36, colorref(w.get_caption_text())); // TEXT_COLOR
    }
}

struct Models {
    cards: Rc<VecModel<CardRow>>,
    logs: Rc<VecModel<LogRow>>,
}

impl Models {
    fn set_card(&self, c: &Value) {
        let row = card_row(c);
        match (0..self.cards.row_count()).find(|i| self.cards.row_data(*i).is_some_and(|r| r.id == row.id)) {
            Some(i) => {
                let mut row = row;
                row.traffic = self.cards.row_data(i).map(|r| r.traffic).unwrap_or_default();
                self.cards.set_row_data(i, row);
            }
            None => self.cards.push(row),
        }
    }

    fn set_stats(&self, stats: &Value) {
        for i in 0..self.cards.row_count() {
            let Some(mut r) = self.cards.row_data(i) else { continue };
            let t: SharedString = traffic_text(&stats[r.id.as_str()]).into();
            if t != r.traffic {
                r.traffic = t;
                self.cards.set_row_data(i, r);
            }
        }
    }

    fn add_logs(&self, entries: &[Value]) {
        for e in entries {
            self.logs.push(log_row(e));
        }
        let over = self.logs.row_count().saturating_sub(LOG_CAP);
        if over > 0 {
            // drop the oldest lines in one go
            let keep: Vec<LogRow> = self.logs.iter().skip(over).collect();
            self.logs.set_vec(keep);
        }
    }
}

fn labels(app: &App) {
    app.set_t_proxy(tr("Proxy").into());
    app.set_t_logs(tr("Logs").into());
    app.set_t_stop_all(tr("Stop all proxies").into());
    app.set_t_empty(tr("No proxies yet. Create one with 'New proxy'.").into());
}

fn main() -> Result<(), slint::PlatformError> {
    // Slint draws its own context menus; the software renderer keeps the GPU driver out of this process
    unsafe { std::env::set_var("SLINT_NO_MUDA", "1") };
    if std::env::var_os("SLINT_BACKEND").is_none() {
        let _ = slint::BackendSelector::new().backend_name("winit".into()).renderer_name("software".into()).select();
    }
    let Some((port, token)) = std::env::var(ipc::ENV).ok().and_then(|v| {
        let mut it = v.split_whitespace();
        Some((it.next()?.parse::<u16>().ok()?, it.next()?.to_string()))
    }) else {
        eprintln!("proxydock-ui is started by proxydock.exe");
        std::process::exit(2);
    };
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| slint::PlatformError::Other(e.to_string()))?;
    ipc::send(&mut stream, &json!({"cmd": "hello", "token": token})).map_err(|e| slint::PlatformError::Other(e.to_string()))?;
    let writer = Arc::new(Mutex::new(stream.try_clone().map_err(|e| slint::PlatformError::Other(e.to_string()))?));

    let app = App::new()?;
    let models = Rc::new(Models { cards: Rc::new(VecModel::default()), logs: Rc::new(VecModel::default()) });
    app.set_cards(ModelRc::from(models.cards.clone()));
    app.set_logs(ModelRc::from(models.logs.clone()));
    if let Some(p) = std::env::var("PROXYDOCK_PAGE").ok().and_then(|p| p.parse::<i32>().ok()) {
        app.set_page(p);
    }
    labels(&app);
    {
        let w = writer.clone();
        app.on_toggle(move |id| {
            // the card's own state decides: the core starts a stopped card and stops a running one
            let _ = ipc::send(&mut w.lock().unwrap(), &json!({"cmd": "toggle", "id": id.as_str()}));
        });
    }
    {
        let w = writer.clone();
        app.on_stop_all(move || {
            let _ = ipc::send(&mut w.lock().unwrap(), &json!({"cmd": "stop_all"}));
        });
    }

    // the core's events, read on a thread and applied on the UI thread
    let weak = app.as_weak();
    std::thread::spawn(move || {
        for v in ipc::messages(stream) {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = weak.upgrade() else { return };
                MODELS.with(|m| {
                    let Some(m) = m.borrow().clone() else { return };
                    match v["ev"].as_str().unwrap_or("") {
                        "hello" => {
                            set_language(v["language"].as_str().unwrap_or("en"));
                            labels(&app);
                            m.cards.set_vec(v["cards"].as_array().into_iter().flatten().map(card_row).collect::<Vec<_>>());
                            m.add_logs(v["logs"].as_array().map(Vec::as_slice).unwrap_or(&[]));
                            m.set_stats(&v["stats"]);
                        }
                        "card" => m.set_card(&v["card"]),
                        "stats" => m.set_stats(&v["stats"]),
                        "log" => m.add_logs(v["entries"].as_array().map(Vec::as_slice).unwrap_or(&[])),
                        _ => {}
                    }
                    app.set_log_count(m.logs.row_count() as i32);
                });
            });
        }
        // the core quit (or crashed): nothing left to show
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
    });
    MODELS.with(|m| *m.borrow_mut() = Some(models.clone()));

    app.show()?;
    // the native window exists once the event loop runs: colour its title bar then, and again on theme changes
    {
        let weak = app.as_weak();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if let Some(a) = weak.upgrade() {
                apply_title_bar(&a);
            }
        });
        let weak = app.as_weak();
        app.on_theme_changed(move || {
            if let Some(a) = weak.upgrade() {
                apply_title_bar(&a);
            }
        });
    }
    slint::run_event_loop()
}

thread_local! {
    static MODELS: std::cell::RefCell<Option<Rc<Models>>> = const { std::cell::RefCell::new(None) };
}
