// Tray process on its own: links no Slint, so no OpenGL / GPU driver in it. Starts slint-proto.exe --ui (next to
// this file) while the window is wanted; that process exits when its window closes.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod menu;

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

static UI_CHILD: std::sync::Mutex<Option<std::process::Child>> = std::sync::Mutex::new(None);

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

/// start (or bring up) the UI process; `page`: open on that page (fresh start only)
fn open_ui(page: Option<i32>) {
    let mut child = UI_CHILD.lock().unwrap();
    if let Some(c) = child.as_mut() {
        if matches!(c.try_wait(), Ok(None)) {
            focus_process_window(c.id());
            return;
        }
    }
    let exe = std::env::current_exe().unwrap().with_file_name("slint-proto.exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--ui");
    if let Some(p) = page {
        cmd.env("PROTO_PAGE", p.to_string());
    }
    *child = cmd.spawn().ok();
}

fn quit() {
    if let Some(mut c) = UI_CHILD.lock().unwrap().take() {
        let _ = c.kill();
    }
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::PostQuitMessage(0) };
}

fn run_tray(show_at_start: bool) {
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use windows_sys::Win32::UI::{HiDpi::*, WindowsAndMessaging::*};

    // physical pixels everywhere (tray click positions, the menu's own drawing)
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };

    let _tray = TrayIconBuilder::new()
        .with_tooltip("ProxyDock prototype")
        .with_icon(Icon::from_rgba(icon_rgba(), 32, 32).unwrap())
        .build()
        .unwrap();

    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| match e {
        TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => open_ui(None),
        TrayIconEvent::Click { button: MouseButton::Right, button_state: MouseButtonState::Up, position, .. } => {
            use menu::Entry;
            let entries = vec![
                Entry::Item { label: "창 열기".into(), id: 1 },
                Entry::Item { label: "로그 보기".into(), id: 2 },
                Entry::Divider,
                Entry::Item { label: "종료".into(), id: 3 },
            ];
            menu::show(position.x as i32, position.y as i32, entries, |id| match id {
                1 => open_ui(None),
                2 => open_ui(Some(1)),
                3 => quit(),
                _ => {}
            });
        }
        _ => {}
    }));

    if show_at_start {
        open_ui(None);
    }
    if std::env::args().any(|a| a == "--menu-test") {
        // test hook: the menu at the middle of the primary screen, as if the tray icon were right-clicked there
        let (cx, cy) = unsafe { (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2) };
        menu::show(cx, cy, vec![
            menu::Entry::Item { label: "창 열기".into(), id: 1 },
            menu::Entry::Item { label: "로그 보기".into(), id: 2 },
            menu::Entry::Divider,
            menu::Entry::Item { label: "종료".into(), id: 3 },
        ], |id| if id == 3 { quit() });
    }
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn main() {
    run_tray(!std::env::args().any(|a| a == "--hidden"));
}
