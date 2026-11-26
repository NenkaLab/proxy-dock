// The tray on Linux and macOS: tao's event loop (GTK / Cocoa underneath, which the tray icon needs), the
// system's own menu (muda) made from the same entries as the Windows menu and made again as things change,
// one copy per user (a lock file; a second start asks the first through a local socket to show its window).

use super::{CORE, Entry, menu_entries, open_ui, quit, restart};
use proxydock::controller;
use std::cell::RefCell;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

/// what other threads ask the tray's (main) thread to do
#[derive(Debug, Clone)]
pub enum Ask {
    Quit,
    Exit,
    Show(u32),
    Restart(bool),
    Tooltip(String),
    Menu(u32),
}

/// what a menu entry does (by its id)
type Pick = Box<dyn Fn(u32)>;

static PROXY: OnceLock<Mutex<EventLoopProxy<Ask>>> = OnceLock::new();

pub fn ask(a: Ask) {
    if let Some(p) = PROXY.get() {
        let _ = p.lock().unwrap().send_event(a);
    }
}

thread_local! {
    /// what the menu's entries do (made with the menu)
    static PICK: RefCell<Option<Pick>> = const { RefCell::new(None) };
    /// the entries the menu shows now (it is made again only when they change)
    static SHOWN: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
}

/// where the lock file and the socket live: per user, and per settings folder (PROXYDOCK_HOME)
fn instance_path(ext: &str) -> std::path::PathBuf {
    let home = std::env::var("PROXYDOCK_HOME").unwrap_or_default();
    let h = home.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let user = std::env::var("USER").unwrap_or_default();
    dir.join(format!("proxydock-{user}-{h:016x}.{ext}"))
}

/// One core process per user: the lock that says so, or None when another one has it
pub fn first_instance() -> Option<std::fs::File> {
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(instance_path("lock")).ok()?;
    f.try_lock().ok()?;
    Some(f)
}

/// A second start: ask the running copy to show its window (the log page with --logs)
pub fn wake_first_instance(logs: bool) {
    if let Ok(mut s) = UnixStream::connect(instance_path("sock")) {
        let _ = s.write_all(if logs { b"2" } else { b"0" });
    }
}

fn listen_for_second_start() {
    let path = instance_path("sock");
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else { return };
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            let mut b = [0u8; 1];
            if s.read_exact(&mut b).is_ok() {
                ask(Ask::Show(if b[0] == b'2' { 2 } else { 0 }));
            }
        }
    });
}

/// A system notification (Python's tray message): macOS through AppleScript, Linux through notify-send (libnotify;
/// nothing when it is not installed)
pub fn notify(text: &str) {
    let text = text.to_string();
    std::thread::spawn(move || {
        if cfg!(target_os = "macos") {
            let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
            let script = format!("display notification \"{}\" with title \"ProxyDock\"", q(&text));
            let _ = std::process::Command::new("osascript").args(["-e", &script]).status();
        } else {
            let _ = std::process::Command::new("notify-send").args(["-a", "ProxyDock", "ProxyDock", &text]).status();
        }
    });
}

/// the system menu from the entries: items, check items for choices, headers as disabled items
fn build_menu(entries: &[Entry]) -> Menu {
    let menu = Menu::new();
    for e in entries {
        let _ = match e {
            Entry::Item { label, id } => menu.append(&MenuItem::with_id(id.to_string(), label, true, None)),
            Entry::Choice { label, id, on } => {
                menu.append(&CheckMenuItem::with_id(id.to_string(), label, true, *on, None))
            }
            Entry::Header { label } => menu.append(&MenuItem::new(label, false, None)),
            Entry::Divider => menu.append(&PredefinedMenuItem::separator()),
        };
    }
    menu
}

pub fn run_tray(show_at_start: Option<u32>, note: Option<String>) {
    let event_loop = EventLoopBuilder::<Ask>::with_user_event().build();
    let _ = PROXY.set(Mutex::new(event_loop.create_proxy()));
    let hooks = controller::Hooks {
        notify: std::sync::Arc::new(|text: &str| notify(text)),
        quit: std::sync::Arc::new(|| ask(Ask::Quit)),
        restart: std::sync::Arc::new(|app: bool| ask(Ask::Restart(!app))),
        tooltip: std::sync::Arc::new(|text: &str| ask(Ask::Tooltip(text.to_string()))),
    };
    let core = match controller::spawn_with(proxydock_core::paths::config_path(), hooks) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("ProxyDock: {e}");
            return;
        }
    };
    if let Some(n) = note {
        core.warn(&n);
    }
    let _ = CORE.set(core);
    listen_for_second_start();
    MenuEvent::set_event_handler(Some(|e: MenuEvent| {
        if let Ok(id) = e.id.0.parse::<u32>() {
            ask(Ask::Menu(id));
        }
    }));

    let mut tray = None;
    let mut next_menu = Instant::now();
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::WaitUntil(next_menu);
        // the menu follows the cards (a second at most behind them)
        let mut remake = false;
        match event {
            // (macOS: the tray icon can only be made once the application runs)
            Event::NewEvents(StartCause::Init) => {
                let (entries, pick) = menu_entries();
                PICK.with(|p| *p.borrow_mut() = Some(pick));
                let t = TrayIconBuilder::new()
                    .with_tooltip("ProxyDock")
                    .with_icon(Icon::from_rgba(proxydock::icon::rgba(32), 32, 32).unwrap())
                    .with_menu(Box::new(build_menu(&entries)))
                    .build();
                SHOWN.with(|s| *s.borrow_mut() = entries);
                match t {
                    Ok(t) => tray = Some(t),
                    Err(e) => eprintln!("ProxyDock: no tray icon ({e})"),
                }
                if let Some(page) = show_at_start {
                    open_ui(page);
                    // started by the user (not at login with --hidden): say what runs (Python `show_start_notice`)
                    let auto = CORE.get().map_or(0, |c| c.cards().iter().filter(|c| c.autostart).count());
                    notify(
                        &proxydock_core::i18n::tr("Running · {len} autostart proxies")
                            .replace("{len}", &auto.to_string()),
                    );
                }
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => remake = true,
            Event::UserEvent(Ask::Quit) => quit(),
            Event::UserEvent(Ask::Exit) => *flow = ControlFlow::Exit,
            Event::UserEvent(Ask::Show(page)) => open_ui(page),
            Event::UserEvent(Ask::Restart(ui_only)) => restart(ui_only),
            Event::UserEvent(Ask::Tooltip(text)) => {
                if let Some(t) = tray.as_ref() {
                    let _ = t.set_tooltip(Some(text));
                }
                remake = true;
            }
            Event::UserEvent(Ask::Menu(id)) => {
                PICK.with(|p| {
                    if let Some(pick) = p.borrow().as_ref() {
                        pick(id);
                    }
                });
                remake = true;
            }
            _ => {}
        }
        if remake && let Some(t) = tray.as_ref() {
            let (entries, pick) = menu_entries();
            PICK.with(|p| *p.borrow_mut() = Some(pick));
            if SHOWN.with(|s| *s.borrow() != entries) {
                t.set_menu(Some(Box::new(build_menu(&entries))));
                SHOWN.with(|s| *s.borrow_mut() = entries);
            }
            next_menu = Instant::now() + Duration::from_secs(1);
            *flow = ControlFlow::WaitUntil(next_menu);
        }
    });
}
