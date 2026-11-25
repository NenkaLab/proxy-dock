// proxydock.exe: the tray + core process. Runs the cards (proxydock_core's manager through the controller) and
// links no GUI toolkit, so it stays small; the window is proxydock-ui.exe, started while it is wanted and exiting
// when it closes.
//   proxydock.exe [--hidden | --logs]   tray (and the window unless --hidden; --logs: on the log page)
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
    use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIF_INFO, NIIF_INFO, NIM_MODIFY, NOTIFYICONDATAW};
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

/// 32x32 RGBA of the app icon (icon.svg at the project root: a blue square, three bars on a dock),
/// drawn with anti-aliasing
fn icon_rgba() -> Vec<u8> {
    use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};
    const SIZE: u32 = 32;
    // icon.svg is 163 units wide
    let k = SIZE as f32 / 163.0;
    let rounded = |x: f32, y: f32, w: f32, h: f32, r: f32| {
        // a rounded rectangle (cubic corners)
        let c = r * 0.552_284_8;
        let mut pb = PathBuilder::new();
        pb.move_to(x + r, y);
        pb.line_to(x + w - r, y);
        pb.cubic_to(x + w - r + c, y, x + w, y + r - c, x + w, y + r);
        pb.line_to(x + w, y + h - r);
        pb.cubic_to(x + w, y + h - r + c, x + w - r + c, y + h, x + w - r, y + h);
        pb.line_to(x + r, y + h);
        pb.cubic_to(x + r - c, y + h, x, y + h - r + c, x, y + h - r);
        pb.line_to(x, y + r);
        pb.cubic_to(x, y + r - c, x + r - c, y, x + r, y);
        pb.close();
        pb.finish()
    };
    let mut pm = Pixmap::new(SIZE, SIZE).unwrap();
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color_rgba8(0x00, 0x78, 0xD4, 255);
    pm.fill_rect(Rect::from_xywh(0.0, 0.0, SIZE as f32, SIZE as f32).unwrap(), &paint, Transform::identity(), None);
    paint.set_color_rgba8(0xD9, 0xD9, 0xD9, 255);
    for (x, y, w, h) in [(24.0, 106.0, 116.0, 20.0), (104.0, 65.0, 20.0, 35.0), (72.0, 49.0, 20.0, 51.0), (40.0, 37.0, 20.0, 63.0)] {
        if let Some(path) = rounded(x * k, y * k, w * k, h * k, 6.0 * k) {
            pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }
    // (every pixel is opaque: premultiplied and straight alpha are the same)
    pm.take()
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
        if owner == find.pid && unsafe { IsWindowVisible(hwnd) } != 0 && unsafe { GetWindow(hwnd, GW_OWNER) }.is_null() {
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

/// start (or bring up) the window process; `page`: 0 canvas, 1 proxy cards, 2 log
fn open_ui(page: u32) {
    let Some(core) = CORE.get() else { return };
    let mut child = UI_CHILD.lock().unwrap();
    if let Some(c) = child.as_mut() {
        if matches!(c.try_wait(), Ok(None)) {
            focus_process_window(c.id());
            return;
        }
    }
    let exe = std::env::current_exe().unwrap().with_file_name("proxydock-ui.exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.env(proxydock::ipc::ENV, core.ipc_env()).env("PROXYDOCK_PAGE", page.to_string());
    // "App scale": on top of the display's own scale (Python QT_SCALE_FACTOR)
    let scale = core.setting("ui_scale").as_f64().unwrap_or(100.0) / 100.0;
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForSystem() } as f64 / 96.0;
    if (scale - 1.0).abs() > 1e-6 {
        cmd.env("SLINT_SCALE_FACTOR", format!("{:.3}", dpi * scale));
    }
    *child = cmd.spawn().ok();
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

const ID_OPEN: u32 = 1;
const ID_LOG: u32 = 2;
const ID_STOP_ALL: u32 = 3;
const ID_QUIT: u32 = 4;
const ID_PC_OFF: u32 = 5;
const ID_STARTUP: u32 = 6;
const ID_STARTUP_HIDDEN: u32 = 7;
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
    use menu::Entry;
    let Some(core) = CORE.get() else { return };
    let cards = core.cards();
    let mut entries = vec![Entry::Item { label: tr("Open window"), id: ID_OPEN }, Entry::Item { label: tr("Logs"), id: ID_LOG }];
    if !cards.is_empty() {
        entries.push(Entry::Divider);
        for (i, c) in cards.iter().take(MENU_CARDS).enumerate() {
            entries.push(Entry::Item { label: format!("{}  ·  {}", c.name, state_text(&c.state)), id: ID_CARD + i as u32 });
        }
        entries.push(Entry::Item { label: tr("Stop all proxies"), id: ID_STOP_ALL });
    }
    // quick controls without the window (Python `_tray_quick_actions`, the startup items)
    let quick = core.quick();
    let mut actions: Vec<serde_json::Value> = vec![];
    for (id, name, on) in &quick.switches {
        entries.push(Entry::Divider);
        entries.push(Entry::Header { label: tr("{name}: switch {state}").replace("{name}", name).replace("{state}", &tr(if *on { "Switched on" } else { "Switched off" })) });
        for state in [true, false] {
            entries.push(Entry::Choice { label: tr(if state { "On" } else { "Off" }), id: ID_QUICK + actions.len() as u32, on: *on == state });
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
    entries.push(Entry::Item { label: tr("Start at login  ·  {value}").replace("{value}", &on_off(registered)), id: ID_STARTUP });
    if registered {
        entries.push(Entry::Item { label: tr("Hide startup notification  ·  {value}").replace("{value}", &on_off(hidden)), id: ID_STARTUP_HIDDEN });
    }
    entries.push(Entry::Divider);
    entries.push(Entry::Item { label: tr("Quit"), id: ID_QUIT });
    menu::show(x, y, entries, move |id| match id {
        ID_OPEN => open_ui(0),
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
    });
}

/// Only one core process per user (a second one would start the autostart cards again): the mutex that says so,
/// or None when another one has it.
fn first_instance() -> Option<windows_sys::Win32::Foundation::HANDLE> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
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
    let glob = proxydock_core::config::load_from(&proxydock_core::paths::config_path()).map(|l| l.global).unwrap_or_default();
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

    let _tray = TrayIconBuilder::new()
        .with_tooltip("ProxyDock")
        .with_icon(Icon::from_rgba(icon_rgba(), 32, 32).unwrap())
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
    let Some(mutex) = first_instance() else { return };
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
    run_tray(if has("--hidden") { None } else if has("--logs") { Some(2) } else { Some(0) }, note);
}
