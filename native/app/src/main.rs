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

/// 32x32 RGBA: blue rounded square with three white boxes on a dock, like make_icon() in proxydock/ui/common.py
fn icon_rgba() -> Vec<u8> {
    let s = 32i32;
    let inside = |x: i32, y: i32, x0: i32, y0: i32, x1: i32, y1: i32, r: i32| {
        if x < x0 || y < y0 || x >= x1 || y >= y1 {
            return false;
        }
        let cx = x.clamp(x0 + r, x1 - 1 - r);
        let cy = y.clamp(y0 + r, y1 - 1 - r);
        (x - cx).pow(2) + (y - cy).pow(2) <= r * r
    };
    let mut out = Vec::with_capacity((s * s * 4) as usize);
    for y in 0..s {
        for x in 0..s {
            let white = inside(x, y, 6, 21, 26, 24, 1)
                || inside(x, y, 8, 13, 12, 20, 1)
                || inside(x, y, 14, 9, 18, 20, 1)
                || inside(x, y, 20, 12, 24, 20, 1);
            out.extend_from_slice(&if white {
                [255, 255, 255, 255]
            } else if inside(x, y, 2, 2, 30, 30, 7) {
                [0, 120, 212, 255]
            } else {
                [0, 0, 0, 0]
            });
        }
    }
    out
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
const ID_CARD: u32 = 100;
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
    entries.push(Entry::Divider);
    entries.push(Entry::Item { label: tr("Quit"), id: ID_QUIT });
    menu::show(x, y, entries, move |id| match id {
        ID_OPEN => open_ui(0),
        ID_LOG => open_ui(2),
        ID_STOP_ALL => core.stop_all(),
        ID_QUIT => quit(),
        id if id >= ID_CARD => {
            if let Some(c) = cards.get((id - ID_CARD) as usize) {
                core.toggle(&c.id);
            }
        }
        _ => {}
    });
}

/// only one core process per user (a second one would start the autostart cards again)
fn first_instance() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "Local\\ProxyDockRustCore".encode_utf16().chain(Some(0)).collect();
    let h = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    !h.is_null() && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS
}

fn run_tray(show_at_start: Option<u32>) {
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
    if !first_instance() {
        return;
    }
    let has = |f: &str| args.iter().any(|a| a == f);
    run_tray(if has("--hidden") { None } else if has("--logs") { Some(2) } else { Some(0) });
}
