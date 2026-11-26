// proxydock.exe: the tray + core process. Runs the cards (proxydock_core's manager through the controller) and
// links no GUI toolkit, so it stays small; the window is proxydock-ui.exe, started while it is wanted and exiting
// when it closes.
//   proxydock.exe [--hidden | --logs]   tray (and the window unless --hidden; --logs: on the log page)
//   proxydock.exe --version             print the version
//   proxydock.exe --restart             (internal) wait for the old copy to end, then start
//   proxydock.exe --tun-helper ...      the elevated TUN helper (started by the app itself through UAC)
//   PROXYDOCK_HOME=<folder>             settings folder instead of %APPDATA%\ProxyDock (tests, portable)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod menu;

use proxydock::controller::{self, Handle};
use proxydock_core::i18n::tr;
use proxydock_core::manager::State;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

static CORE: OnceLock<Handle> = OnceLock::new();
static UI_CHILD: Mutex<Option<std::process::Child>> = Mutex::new(None);
/// the tray icon's window (its notifications go through it)
static TRAY_HWND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A notification from the tray icon (Python `tray.showMessage(APP_NAME, text, Information)`).
fn tray_message(text: &str) {
    use windows_sys::Win32::UI::Shell::{NIF_INFO, NIIF_INFO, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW};
    let hwnd = TRAY_HWND.load(std::sync::atomic::Ordering::Relaxed);
    if hwnd == 0 {
        return;
    }
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd as _;
    // tray-icon numbers its icons from 1; this process has one
    nid.uID = 1;
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO;
    let copy = |dst: &mut [u16], text: &str| {
        let w: Vec<u16> = text.encode_utf16().take(dst.len() - 1).collect();
        dst[..w.len()].copy_from_slice(&w);
    };
    copy(&mut nid.szInfoTitle, "ProxyDock");
    copy(&mut nid.szInfo, text);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

/// Bring the first visible top-level window of process `pid` to the front. True if one was found.
fn focus_process_window(pid: u32) -> bool {
    use windows_sys::Win32::{Foundation::*, UI::WindowsAndMessaging::*};
    struct Find {
        pid: u32,
        hwnd: HWND,
    }
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
        let find = unsafe { &mut *(lparam as *mut Find) };
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        if owner == find.pid && unsafe { IsWindowVisible(hwnd) } != 0 && unsafe { GetWindow(hwnd, GW_OWNER) }.is_null()
        {
            find.hwnd = hwnd;
            return 0;
        }
        1
    }
    let mut find = Find { pid, hwnd: std::ptr::null_mut() };
    unsafe {
        EnumWindows(Some(each), &mut find as *mut Find as LPARAM);
        if find.hwnd.is_null() {
            return false;
        }
        if IsIconic(find.hwnd) != 0 {
            ShowWindow(find.hwnd, SW_RESTORE);
        }
        SetForegroundWindow(find.hwnd);
    }
    true
}

/// the window is drawn at this much of the display's scale when "App scale" is 100%
const BASE_SCALE: f64 = 0.9;

/// start (or bring up) the window process; `page`: 0 canvas, 1 proxy cards, 2 log
fn open_ui(page: u32) {
    let Some(core) = CORE.get() else { return };
    let mut child = UI_CHILD.lock().unwrap();
    if let Some(c) = child.as_mut()
        && matches!(c.try_wait(), Ok(None))
    {
        focus_process_window(c.id());
        return;
    }
    let exe = std::env::current_exe().unwrap().with_file_name("proxydock-ui.exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.env(proxydock::ipc::ENV, core.ipc_env()).env("PROXYDOCK_PAGE", page.to_string());
    // "App scale": on top of the display's own scale (Python QT_SCALE_FACTOR)
    // (100% is drawn a little smaller than the display's scale: the M3 sizes read large next to other apps)
    let scale = core.setting("ui_scale").as_f64().unwrap_or(100.0) / 100.0 * BASE_SCALE;
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForSystem() } as f64 / 96.0;
    // (a SLINT_SCALE_FACTOR already set, as for the README pictures, is kept)
    if std::env::var_os("SLINT_SCALE_FACTOR").is_none() {
        cmd.env("SLINT_SCALE_FACTOR", format!("{:.3}", dpi * scale));
    }
    *child = cmd.spawn().ok();
}

/// the window process's top-level window, if it is open
fn ui_window() -> Option<usize> {
    use windows_sys::Win32::{Foundation::*, UI::WindowsAndMessaging::*};
    let pid = {
        let mut child = UI_CHILD.lock().unwrap();
        let c = child.as_mut()?;
        if !matches!(c.try_wait(), Ok(None)) {
            return None;
        }
        c.id()
    };
    struct Find {
        pid: u32,
        hwnd: HWND,
    }
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
        let find = unsafe { &mut *(lparam as *mut Find) };
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        if owner == find.pid && unsafe { IsWindowVisible(hwnd) } != 0 && unsafe { GetWindow(hwnd, GW_OWNER) }.is_null()
        {
            find.hwnd = hwnd;
            return 0;
        }
        1
    }
    let mut find = Find { pid, hwnd: std::ptr::null_mut() };
    unsafe { EnumWindows(Some(each), &mut find as *mut Find as LPARAM) };
    (!find.hwnd.is_null()).then_some(find.hwnd as usize)
}

/// close the window (it saves its place and ends; the tray and the cards go on, whatever "Quit with the X button")
fn close_ui() {
    if let Some(core) = CORE.get() {
        core.close_window();
    }
}

/// Python `restart`: a new copy (told to wait for this one) and this one quits; `ui_only`: only the window opens
/// again (a new app scale takes effect when it starts)
fn restart(ui_only: bool) {
    if ui_only {
        // (the controller already told the window to close)
        std::thread::spawn(|| {
            // (the old window ends first: one window at a time)
            for _ in 0..50 {
                std::thread::sleep(Duration::from_millis(100));
                let alive = UI_CHILD.lock().unwrap().as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)));
                if !alive {
                    break;
                }
            }
            open_ui(0);
        });
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let mut args: Vec<String> = std::env::args().skip(1).filter(|a| a != "--restart").collect();
    args.push("--restart".into());
    let mut cmd = std::process::Command::new(exe);
    cmd.args(&args);
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008 | 0x0000_0200); // detached, its own group
    }
    match cmd.spawn() {
        Ok(_) => quit(),
        Err(e) => {
            if let Some(core) = CORE.get() {
                core.warn(&tr("Restart failed: {e}").replace("{e}", &e.to_string()));
            }
        }
    }
}

fn quit() {
    if let Some(mut c) = UI_CHILD.lock().unwrap().take() {
        let _ = c.kill();
    }
    if let Some(core) = CORE.get() {
        core.quit(Duration::from_secs(10));
    }
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::PostQuitMessage(0) };
}

/// a thread message: quit the app (sent by the controller)
const WM_QUIT_APP: u32 = 0x8000 + 1;
/// a thread message: open the window (wparam: the page), from a second start
const WM_SHOW_UI: u32 = 0x8000 + 2;
/// a thread message: restart (wparam 1: the whole app, 0: the window)
const WM_RESTART: u32 = 0x8000 + 3;
/// a thread message: the tray tooltip changed (in TOOLTIP)
const WM_TOOLTIP: u32 = 0x8000 + 4;
static TOOLTIP: Mutex<String> = Mutex::new(String::new());
/// named events a second start sets (the window / the log page)
const SHOW_EVENTS: [&str; 2] = ["Local\\ProxyDockRustShow", "Local\\ProxyDockRustShowLogs"];

/// A second start: ask the running ProxyDock to show its window (the log page with --logs), then end
fn wake_first_instance(logs: bool) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
    let name: Vec<u16> = SHOW_EVENTS[logs as usize].encode_utf16().chain(Some(0)).collect();
    unsafe {
        let h = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
        if !h.is_null() {
            SetEvent(h);
            CloseHandle(h);
        }
    }
}

/// The first start: a thread waits for a second start's request and passes it to the tray's thread
fn listen_for_second_start(main_thread: u32) {
    use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, WaitForMultipleObjects};
    let handles: Vec<_> = SHOW_EVENTS
        .iter()
        .map(|n| {
            let name: Vec<u16> = n.encode_utf16().chain(Some(0)).collect();
            unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) }
        })
        .collect();
    if handles.iter().any(|h| h.is_null()) {
        return;
    }
    let handles: Vec<usize> = handles.into_iter().map(|h| h as usize).collect();
    std::thread::spawn(move || {
        let hs: Vec<windows_sys::Win32::Foundation::HANDLE> = handles.iter().map(|h| *h as _).collect();
        loop {
            let r = unsafe { WaitForMultipleObjects(hs.len() as u32, hs.as_ptr(), 0, INFINITE) };
            if (r as usize) >= hs.len() {
                return;
            }
            // the window, or the log page (page 2)
            let page = if r == 1 { 2 } else { 0 };
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW(main_thread, WM_SHOW_UI, page, 0)
            };
        }
    });
}

const ID_OPEN: u32 = 1;
const ID_LOG: u32 = 2;
const ID_STOP_ALL: u32 = 3;
const ID_QUIT: u32 = 4;
const ID_PC_OFF: u32 = 5;
const ID_STARTUP: u32 = 6;
const ID_STARTUP_HIDDEN: u32 = 7;
const ID_RESTART: u32 = 8;
const ID_CLOSE: u32 = 9;
const ID_CARD: u32 = 100;
/// the IF switches' and groups' choices
const ID_QUICK: u32 = 1000;
/// as many cards as the Python tray lists
const MENU_CARDS: usize = 15;

fn state_text(s: &State) -> String {
    match s {
        State::Running => tr("Running"),
        State::Starting => tr("Starting…"),
        State::Stopped => tr("Stopped"),
        State::Error(_) => tr("error"),
    }
}

fn show_menu(x: i32, y: i32) {
    if CORE.get().is_none() {
        return;
    }
    menu::show_live(x, y, Box::new(menu_entries));
}

/// the tray menu as things are now, and what each entry does
fn menu_entries() -> (Vec<menu::Entry>, Box<dyn Fn(u32)>) {
    use menu::Entry;
    let core = CORE.get().unwrap();
    let cards = core.cards();
    // (Python: the first item opens or closes the window)
    let open = ui_window().is_some();
    let mut entries = vec![
        if open {
            Entry::Item { label: tr("Close window"), id: ID_CLOSE }
        } else {
            Entry::Item { label: tr("Open window"), id: ID_OPEN }
        },
        Entry::Item { label: tr("Logs"), id: ID_LOG },
    ];
    if !cards.is_empty() {
        entries.push(Entry::Divider);
        for (i, c) in cards.iter().take(MENU_CARDS).enumerate() {
            entries.push(Entry::Item {
                label: format!("{}  ·  {}", c.name, state_text(&c.state)),
                id: ID_CARD + i as u32,
            });
        }
        entries.push(Entry::Item { label: tr("Stop all proxies"), id: ID_STOP_ALL });
    }
    // quick controls without the window (Python `_tray_quick_actions`, the startup items)
    let quick = core.quick();
    let mut actions: Vec<serde_json::Value> = vec![];
    for (id, name, on) in &quick.switches {
        entries.push(Entry::Divider);
        entries.push(Entry::Header {
            label: tr("{name}: switch {state}")
                .replace("{name}", name)
                .replace("{state}", &tr(if *on { "Switched on" } else { "Switched off" })),
        });
        for state in [true, false] {
            entries.push(Entry::Choice {
                label: tr(if state { "On" } else { "Off" }),
                id: ID_QUICK + actions.len() as u32,
                on: *on == state,
            });
            actions.push(serde_json::json!({"cmd": "set_switch", "id": id, "on": state}));
        }
    }
    for (gid, name, members, cur) in &quick.groups {
        entries.push(Entry::Divider);
        entries.push(Entry::Header { label: tr("{name}: use member").replace("{name}", name) });
        for (mid, mname) in members {
            entries.push(Entry::Choice { label: mname.clone(), id: ID_QUICK + actions.len() as u32, on: mid == cur });
            actions.push(serde_json::json!({"cmd": "select_member", "group": gid, "member": mid}));
        }
    }
    if let Some(name) = core.pc_card() {
        entries.push(Entry::Divider);
        entries.push(Entry::Item { label: tr("PC proxy: {name} - turn off").replace("{name}", &name), id: ID_PC_OFF });
    }
    let (registered, hidden) = proxydock_core::platform::startup_state();
    let on_off = |b: bool| tr(if b { "On" } else { "Off" });
    entries.push(Entry::Divider);
    entries.push(Entry::Item {
        label: tr("Start at login  ·  {value}").replace("{value}", &on_off(registered)),
        id: ID_STARTUP,
    });
    if registered {
        entries.push(Entry::Item {
            label: tr("Hide startup notification  ·  {value}").replace("{value}", &on_off(hidden)),
            id: ID_STARTUP_HIDDEN,
        });
    }
    entries.push(Entry::Divider);
    entries.push(Entry::Item { label: tr("Restart"), id: ID_RESTART });
    entries.push(Entry::Item { label: tr("Quit"), id: ID_QUIT });
    let pick = move |id: u32| match id {
        ID_OPEN => open_ui(0),
        ID_CLOSE => close_ui(),
        ID_RESTART => restart(false),
        ID_LOG => open_ui(2),
        ID_STOP_ALL => core.stop_all(),
        ID_QUIT => quit(),
        ID_PC_OFF => core.tray(serde_json::json!({"cmd": "pc_off"})),
        ID_STARTUP => core.tray(serde_json::json!({"cmd": "startup", "on": !registered})),
        ID_STARTUP_HIDDEN => core.tray(serde_json::json!({"cmd": "startup_hidden", "on": !hidden})),
        id if id >= ID_QUICK => {
            if let Some(a) = actions.get((id - ID_QUICK) as usize) {
                core.tray(a.clone());
            }
        }
        id if id >= ID_CARD => {
            if let Some(c) = cards.get((id - ID_CARD) as usize) {
                core.toggle(&c.id);
            }
        }
        _ => {}
    };
    (entries, Box::new(pick))
}

/// Only one core process per user (a second one would start the autostart cards again): the mutex that says so,
/// or None when another one has it.
fn first_instance() -> Option<windows_sys::Win32::Foundation::HANDLE> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "Local\\ProxyDockRustCore".encode_utf16().chain(Some(0)).collect();
    let h = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if h.is_null() {
        return None;
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(h) };
        return None;
    }
    Some(h)
}

/// "Run as administrator" (Python `relaunch_elevated`): an elevated copy through UAC takes over. Ok(true): it
/// started, this one ends; Err: the prompt was cancelled (this one goes on without the rights).
fn relaunch_elevated(args: &[String], mutex: windows_sys::Win32::Foundation::HANDLE) -> Result<bool, String> {
    let glob =
        proxydock_core::config::load_from(&proxydock_core::paths::config_path()).map(|l| l.global).unwrap_or_default();
    let elevated = args.iter().any(|a| a == "--elevated");
    if glob.get("run_as_admin") != Some(&serde_json::json!(true)) || elevated || proxydock_core::manager::is_admin() {
        return Ok(false);
    }
    // (the copy must get the single-instance mutex; on an error this one takes it again)
    unsafe { windows_sys::Win32::Foundation::CloseHandle(mutex) };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut extra = vec!["--elevated".to_string()];
    extra.extend(args.iter().filter(|a| *a == "--hidden" || *a == "--logs").cloned());
    proxydock_core::singbox::helper::runas(&exe, &extra).map(|()| true)
}

fn run_tray(show_at_start: Option<u32>, note: Option<String>) {
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use windows_sys::Win32::UI::{HiDpi::*, WindowsAndMessaging::*};

    // physical pixels everywhere (tray click positions, the menu's own drawing)
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    // the controller's thread asks this one to quit (it cannot wait for itself)
    let main_thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    let hooks = controller::Hooks {
        notify: std::sync::Arc::new(|text: &str| tray_message(text)),
        quit: std::sync::Arc::new(move || unsafe {
            PostThreadMessageW(main_thread, WM_QUIT_APP, 0, 0);
        }),
        restart: std::sync::Arc::new(move |app: bool| unsafe {
            PostThreadMessageW(main_thread, WM_RESTART, app as usize, 0);
        }),
        tooltip: std::sync::Arc::new(move |text: &str| {
            *TOOLTIP.lock().unwrap() = text.to_string();
            unsafe { PostThreadMessageW(main_thread, WM_TOOLTIP, 0, 0) };
        }),
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
    listen_for_second_start(main_thread);

    let _tray = TrayIconBuilder::new()
        .with_tooltip("ProxyDock")
        .with_icon(Icon::from_rgba(proxydock::icon::rgba(32), 32, 32).unwrap())
        .build()
        .unwrap();
    TRAY_HWND.store(_tray.window_handle() as usize, std::sync::atomic::Ordering::Relaxed);
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| match e {
        TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => open_ui(0),
        TrayIconEvent::Click { button: MouseButton::Right, button_state: MouseButtonState::Up, position, .. } => {
            show_menu(position.x as i32, position.y as i32)
        }
        _ => {}
    }));
    if let Some(page) = show_at_start {
        open_ui(page);
        // started by the user (not at login with --hidden): say what runs (Python `show_start_notice`)
        let auto = CORE.get().map_or(0, |c| c.cards().iter().filter(|c| c.autostart).count());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(800));
            tray_message(&tr("Running · {len} autostart proxies").replace("{len}", &auto.to_string()));
        });
    }
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.hwnd.is_null() && msg.message == WM_QUIT_APP {
                quit();
                continue;
            }
            if msg.hwnd.is_null() && msg.message == WM_SHOW_UI {
                open_ui(msg.wParam as u32);
                continue;
            }
            if msg.hwnd.is_null() && msg.message == WM_RESTART {
                restart(msg.wParam == 0);
                continue;
            }
            if msg.hwnd.is_null() && msg.message == WM_TOOLTIP {
                let tip = TOOLTIP.lock().unwrap().clone();
                let _ = _tray.set_tooltip(Some(tip));
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some(proxydock_core::singbox::helper::ARG) {
        std::process::exit(proxydock_core::singbox::helper::main(&args[1..]));
    }
    if args.iter().any(|a| a == "--version") {
        // (a GUI program: the console it was started from, if any)
        unsafe { windows_sys::Win32::System::Console::AttachConsole(u32::MAX) };
        println!("ProxyDock {}", proxydock_core::update::current_version());
        return;
    }
    // a restart: the old copy is still quitting (Python `become_primary(wait=True)`): wait up to 15 s
    let mut mutex = first_instance();
    if mutex.is_none() && args.iter().any(|a| a == "--restart") {
        for _ in 0..150 {
            std::thread::sleep(Duration::from_millis(100));
            mutex = first_instance();
            if mutex.is_some() {
                break;
            }
        }
    }
    let Some(mutex) = mutex else {
        // already running: show its window instead (Python `_on_second_instance`)
        wake_first_instance(args.iter().any(|a| a == "--logs"));
        return;
    };
    // the previous version's start-at-login entry becomes ours (Python `migrate_autostart`); not for a run on
    // another settings folder (PROXYDOCK_HOME: tests, portable), which must leave the user's login items alone
    let migrated = std::env::var_os("PROXYDOCK_HOME").is_none()
        && std::env::current_exe().is_ok_and(|exe| proxydock_core::platform::migrate_autostart(&exe));
    let note = match relaunch_elevated(&args, mutex) {
        Ok(true) => return,
        Ok(false) => None,
        Err(e) => {
            // the prompt was cancelled: carry on without the rights (the mutex again)
            if first_instance().is_none() {
                return;
            }
            Some(tr("Not running as administrator: {e}").replace("{e}", &e))
        }
    };
    let has = |f: &str| args.iter().any(|a| a == f);
    let note = match (note, migrated) {
        (Some(n), _) => Some(n),
        (None, true) => Some(tr("Moved the previous version's startup entry to ProxyDock")),
        (None, false) => None,
    };
    // (a restart opens the window again unless it was started hidden)
    run_tray(
        if has("--hidden") {
            None
        } else if has("--logs") {
            Some(2)
        } else {
            Some(0)
        },
        note,
    );
}
