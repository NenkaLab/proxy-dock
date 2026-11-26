// Tray menu in the app's Material 3 look: a layered popup window drawn with tiny-skia (shapes) and ab_glyph
// (text). No GUI toolkit, so the tray process stays a few MB; the font file is read only while the menu is open.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Transform};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
    UI::HiDpi::GetDpiForWindow,
    UI::Input::KeyboardAndMouse::*,
    UI::WindowsAndMessaging::*,
};

#[derive(Clone, PartialEq)]
pub enum Entry {
    Item {
        label: String,
        id: u32,
    },
    /// a small caption over the choices that follow (not clickable)
    Header {
        label: String,
    },
    /// one of several choices; `on` gets a check mark
    Choice {
        label: String,
        id: u32,
        on: bool,
    },
    Divider,
}

impl Entry {
    fn id(&self) -> Option<u32> {
        match self {
            Entry::Item { id, .. } | Entry::Choice { id, .. } => Some(*id),
            _ => None,
        }
    }
}

// sizes in logical px (scaled by the monitor DPI)
const WIDTH: f32 = 240.0;
/// at most this wide, however long a label is
const MAX_WIDTH: f32 = 480.0;
// M3 menus: 48 px items, Body Large labels; group names in Title Small
const HEADER_H: f32 = 36.0;
const ITEM_H: f32 = 48.0;
const DIVIDER_H: f32 = 17.0;
const PAD: f32 = 8.0; // above the first / below the last entry
const SHADOW: f32 = 14.0; // transparent margin for the shadow
const RADIUS: f32 = 16.0; // M3 Expressive menus: large corners
// em sizes (`em_scale` turns them into ab_glyph's line-height scale)
const FONT_PX: f32 = 16.0;
const HEADER_PX: f32 = 14.0;
// the label's start, and a choice's (after its check mark)
const TEXT_X: f32 = 24.0;
const CHOICE_X: f32 = 56.0;
const MOUSE_LEAVE: u32 = 0x02A3; // WM_MOUSELEAVE

struct State {
    hwnd: HWND,
    entries: Vec<Entry>,
    hover: Option<usize>,
    scale: f32,
    dark: bool,
    font: FontVec,
    /// the menu surface's width (physical px)
    width: f32,
    on_pick: Box<dyn Fn(u32)>,
    size: (i32, i32),
    tracking: bool,
    /// where it was opened (its corner follows a new size), and what makes its entries again
    anchor: (i32, i32),
    rebuild: Option<Rebuild>,
}

/// the menu's entries and what picking one does, made again while it is open
pub type Rebuild = Box<dyn Fn() -> (Vec<Entry>, Box<dyn Fn(u32)>)>;

const REFRESH_TIMER: usize = 1;

thread_local! {
    static MENU: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Windows "app mode" (Settings > Personalization > Colors)
fn apps_dark() -> bool {
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = wide("AppsUseLightTheme");
    let mut value: u32 = 1;
    let mut size = 4u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut value as *mut u32 as *mut _,
            &mut size,
        )
    };
    rc == 0 && value == 0
}

fn load_font() -> Option<FontVec> {
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    for name in ["malgun.ttf", "segoeui.ttf"] {
        if let Ok(data) = std::fs::read(format!(r"{dir}\Fonts\{name}"))
            && let Ok(font) = FontVec::try_from_vec(data)
        {
            return Some(font);
        }
    }
    None
}

/// (top, height) of each entry inside the menu surface, in physical px
fn layout(entries: &[Entry], s: f32) -> Vec<(f32, f32)> {
    let mut y = PAD * s;
    entries
        .iter()
        .map(|e| {
            let h = match e {
                Entry::Item { .. } | Entry::Choice { .. } => ITEM_H * s,
                Entry::Header { .. } => HEADER_H * s,
                Entry::Divider => DIVIDER_H * s,
            };
            let r = (y, h);
            y += h;
            r
        })
        .collect()
}

fn surface_size(entries: &[Entry], s: f32, width: f32) -> (f32, f32) {
    let h: f32 = layout(entries, s).iter().map(|(_, h)| h).sum::<f32>() + PAD * 2.0 * s;
    (width, h)
}

/// ab_glyph's PxScale is the font's whole line height (ascent - descent), not its em size: a "14 px"
/// scale draws letters near 11 px. This gives the scale for an em size in px (as CSS / Slint sizes are).
fn em_scale(font: &FontVec, px: f32) -> PxScale {
    let height = font.height_unscaled();
    let upem = font.units_per_em().unwrap_or(height);
    PxScale::from(px * height / upem)
}

fn text_width(font: &FontVec, text: &str, px: f32) -> f32 {
    let sf = font.as_scaled(em_scale(font, px));
    text.chars().map(|c| sf.h_advance(font.glyph_id(c))).sum()
}

/// wide enough for the longest label (choices are indented), between WIDTH and MAX_WIDTH
fn menu_width(font: &FontVec, entries: &[Entry], s: f32) -> f32 {
    let need = entries
        .iter()
        .map(|e| match e {
            Entry::Item { label, .. } => text_width(font, label, FONT_PX * s) + (TEXT_X * 2.0) * s,
            Entry::Header { label } => text_width(font, label, HEADER_PX * s) + (TEXT_X * 2.0) * s,
            Entry::Choice { label, .. } => text_width(font, label, FONT_PX * s) + (CHOICE_X + TEXT_X) * s,
            Entry::Divider => 0.0,
        })
        .fold(WIDTH * s, f32::max);
    need.min(MAX_WIDTH * s).ceil()
}

fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let k = 0.552_284_8 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn fill(pm: &mut Pixmap, path: Option<Path>, rgba: (u8, u8, u8, u8)) {
    if let Some(path) = path {
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(rgba.0, rgba.1, rgba.2, rgba.3));
        paint.anti_alias = true;
        pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

fn draw_text(pm: &mut Pixmap, font: &FontVec, text: &str, x: f32, center_y: f32, px: f32, rgb: (u8, u8, u8)) {
    let scale = em_scale(font, px);
    let sf = font.as_scaled(scale);
    let baseline = center_y + (sf.ascent() + sf.descent()) / 2.0;
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let mut caret = x;
    let mut prev = None;
    for ch in text.chars() {
        let id = font.glyph_id(ch);
        if let Some(p) = prev {
            caret += sf.kern(p, id);
        }
        let glyph = id.with_scale_and_position(scale, point(caret, baseline));
        caret += sf.h_advance(id);
        prev = Some(id);
        let Some(outline) = font.outline_glyph(glyph) else { continue };
        let b = outline.px_bounds();
        outline.draw(|gx, gy, c| {
            let (px_, py_) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
            if px_ < 0 || py_ < 0 || px_ >= w || py_ >= h {
                return;
            }
            let i = ((py_ * w + px_) * 4) as usize;
            let a = c.clamp(0.0, 1.0);
            // premultiplied "source over"
            for (k, v) in [rgb.0, rgb.1, rgb.2].into_iter().enumerate() {
                data[i + k] = (v as f32 * a + data[i + k] as f32 * (1.0 - a)).round() as u8;
            }
            data[i + 3] = (255.0 * a + data[i + 3] as f32 * (1.0 - a)).round() as u8;
        });
    }
}

fn render(st: &State) {
    let s = st.scale;
    let (w, h) = st.size;
    let Some(mut pm) = Pixmap::new(w as u32, h as u32) else { return };
    let (sw, sh) = surface_size(&st.entries, s, st.width);
    let (ox, oy) = (SHADOW * s, SHADOW * s);

    // M3 roles for the blue seed (same as the UI): surface-container, on-surface, outline-variant
    let (surface, on_surface, outline) = if st.dark {
        ((0x1d, 0x20, 0x24), (0xe1, 0xe2, 0xe8), (0x43, 0x47, 0x4e))
    } else {
        ((0xec, 0xee, 0xf4), (0x19, 0x1c, 0x20), (0xc3, 0xc7, 0xcf))
    };

    // soft shadow: stacked translucent rounded rects, a little lower than the surface
    let layers = 10;
    for i in (1..=layers).rev() {
        let spread = i as f32 * 1.2 * s;
        let alpha = ((if st.dark { 26.0 } else { 14.0 }) * (1.0 - i as f32 / (layers as f32 + 1.0))) as u8;
        fill(
            &mut pm,
            rrect(ox - spread, oy - spread + 3.0 * s, sw + spread * 2.0, sh + spread * 2.0, RADIUS * s + spread),
            (0, 0, 0, alpha),
        );
    }
    fill(&mut pm, rrect(ox, oy, sw, sh, RADIUS * s), (surface.0, surface.1, surface.2, 255));

    for (idx, (e, (top, eh))) in st.entries.iter().zip(layout(&st.entries, s)).enumerate() {
        match e {
            Entry::Divider => {
                fill(
                    &mut pm,
                    rrect(ox + 16.0 * s, oy + top + eh / 2.0 - 0.5 * s, sw - 32.0 * s, 1.0 * s, 0.0),
                    (outline.0, outline.1, outline.2, 255),
                );
            }
            Entry::Item { label, .. } => {
                if st.hover == Some(idx) {
                    // state layer: on-surface at 10%, a pill inside the surface
                    fill(
                        &mut pm,
                        rrect(ox + 8.0 * s, oy + top + 2.0 * s, sw - 16.0 * s, eh - 4.0 * s, (eh - 4.0 * s) / 2.0),
                        (on_surface.0, on_surface.1, on_surface.2, 26),
                    );
                }
                draw_text(&mut pm, &st.font, label, ox + TEXT_X * s, oy + top + eh / 2.0, FONT_PX * s, on_surface);
            }
            Entry::Header { label } => {
                let muted = if st.dark { (0xc3, 0xc7, 0xcf) } else { (0x43, 0x47, 0x4e) };
                draw_text(
                    &mut pm,
                    &st.font,
                    label,
                    ox + TEXT_X * s,
                    oy + top + eh / 2.0 + 3.0 * s,
                    HEADER_PX * s,
                    muted,
                );
            }
            Entry::Choice { label, on, .. } => {
                if st.hover == Some(idx) {
                    fill(
                        &mut pm,
                        rrect(ox + 8.0 * s, oy + top + 2.0 * s, sw - 16.0 * s, eh - 4.0 * s, (eh - 4.0 * s) / 2.0),
                        (on_surface.0, on_surface.1, on_surface.2, 26),
                    );
                }
                if *on {
                    // a check mark (drawn: the menu's font may not have one)
                    let (cx, cy) = (ox + 32.0 * s, oy + top + eh / 2.0);
                    let mut pb = PathBuilder::new();
                    pb.move_to(cx - 6.0 * s, cy);
                    pb.line_to(cx - 2.0 * s, cy + 4.0 * s);
                    pb.line_to(cx + 6.5 * s, cy - 4.5 * s);
                    if let Some(path) = pb.finish() {
                        let mut paint = Paint::default();
                        paint.set_color(Color::from_rgba8(on_surface.0, on_surface.1, on_surface.2, 255));
                        paint.anti_alias = true;
                        let stroke = tiny_skia::Stroke {
                            width: 2.0 * s,
                            line_cap: tiny_skia::LineCap::Round,
                            line_join: tiny_skia::LineJoin::Round,
                            ..Default::default()
                        };
                        pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
                    }
                }
                draw_text(&mut pm, &st.font, label, ox + CHOICE_X * s, oy + top + eh / 2.0, FONT_PX * s, on_surface);
            }
        }
    }
    upload(st.hwnd, &pm);
}

/// premultiplied RGBA pixmap -> layered window
fn upload(hwnd: HWND, pm: &Pixmap) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(screen);
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h; // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if !dib.is_null() && !bits.is_null() {
            let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
            for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(pm.data().as_chunks::<4>().0) {
                d[0] = s[2];
                d[1] = s[1];
                d[2] = s[0];
                d[3] = s[3];
            }
            let old = SelectObject(mem, dib);
            let mut rc: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut rc);
            let pos = POINT { x: rc.left, y: rc.top };
            let size = SIZE { cx: w, cy: h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            UpdateLayeredWindow(hwnd, screen, &pos, &size, mem, &src, 0, &blend, ULW_ALPHA);
            SelectObject(mem, old);
            DeleteObject(dib);
        }
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
    }
}

fn hit(st: &State, y_px: i32) -> Option<usize> {
    let y = y_px as f32 - SHADOW * st.scale;
    layout(&st.entries, st.scale)
        .iter()
        .position(|&(top, h)| y >= top && y < top + h)
        .filter(|&i| st.entries[i].id().is_some())
}

fn close() {
    let st = MENU.with(|m| m.try_borrow_mut().ok().and_then(|mut m| m.take()));
    if let Some(st) = st {
        unsafe { DestroyWindow(st.hwnd) };
    }
}

fn pick(index: usize) {
    let st = MENU.with(|m| m.try_borrow_mut().ok().and_then(|mut m| m.take()));
    if let Some(st) = st {
        unsafe { DestroyWindow(st.hwnd) };
        if let Some(id) = st.entries.get(index).and_then(Entry::id) {
            (st.on_pick)(id);
        }
    }
}

fn with_state(f: impl FnOnce(&mut State)) {
    MENU.with(|m| {
        if let Ok(mut m) = m.try_borrow_mut()
            && let Some(st) = m.as_mut()
        {
            f(st);
        }
    });
}

fn move_hover(st: &mut State, step: i32) {
    let n = st.entries.len() as i32;
    let mut i = st.hover.map(|h| h as i32).unwrap_or(if step > 0 { -1 } else { n });
    for _ in 0..n {
        i = (i + step).rem_euclid(n);
        if st.entries[i as usize].id().is_some() {
            st.hover = Some(i as usize);
            return;
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let y = ((lparam >> 16) & 0xffff) as i16 as i32;
            with_state(|st| {
                if !st.tracking {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe { TrackMouseEvent(&mut tme) };
                    st.tracking = true;
                }
                let h = hit(st, y);
                if h != st.hover {
                    st.hover = h;
                    render(st);
                }
            });
            0
        }
        MOUSE_LEAVE => {
            with_state(|st| {
                st.tracking = false;
                if st.hover.is_some() {
                    st.hover = None;
                    render(st);
                }
            });
            0
        }
        WM_LBUTTONUP => {
            let y = ((lparam >> 16) & 0xffff) as i16 as i32;
            let mut target = None;
            with_state(|st| target = hit(st, y));
            if let Some(i) = target {
                pick(i);
            }
            0
        }
        WM_KEYDOWN => {
            match wparam as u16 {
                VK_ESCAPE => close(),
                VK_DOWN | VK_UP => with_state(|st| {
                    move_hover(st, if wparam as u16 == VK_DOWN { 1 } else { -1 });
                    render(st);
                }),
                VK_RETURN | VK_SPACE => {
                    let mut target = None;
                    with_state(|st| target = st.hover);
                    if let Some(i) = target {
                        pick(i);
                    }
                }
                _ => {}
            }
            0
        }
        // once a second: the entries again (a card started, a switch moved); drawn again only if they changed
        WM_TIMER if wparam == REFRESH_TIMER => {
            refresh();
            0
        }
        WM_ACTIVATE => {
            // clicked somewhere else
            if (wparam & 0xffff) as u32 == WA_INACTIVE {
                unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
            }
            0
        }
        WM_CLOSE => {
            close();
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

static REGISTERED: AtomicBool = AtomicBool::new(false);
const CLASS: &str = "ProxyDockTrayMenu";

/// where the menu goes for cursor point (x, y): up and to the left like Windows' own tray menus, flipped where
/// the screen ends; (left, top) of the surface
unsafe fn place(x: i32, y: i32, sw: f32, sh: f32) -> (i32, i32) {
    let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    unsafe { GetMonitorInfoW(MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST), &mut mi) };
    let work = mi.rcWork;
    let (sw_i, sh_i) = (sw.ceil() as i32, sh.ceil() as i32);
    let mut left = x - sw_i;
    if left < work.left {
        left = x;
    }
    left = left.min(work.right - sw_i);
    let mut top = y - sh_i;
    if top < work.top {
        top = y;
    }
    (left, top.min(work.bottom - sh_i))
}

/// the open menu with its entries made again (kept as it is when they did not change)
fn refresh() {
    let Some(rebuild) = MENU.with(|m| m.borrow_mut().as_mut().and_then(|st| st.rebuild.take())) else { return };
    let (entries, on_pick) = rebuild();
    MENU.with(|m| {
        let mut m = m.borrow_mut();
        let Some(st) = m.as_mut() else { return };
        st.rebuild = Some(rebuild);
        if st.entries == entries {
            return;
        }
        st.width = menu_width(&st.font, &entries, st.scale);
        let (sw, sh) = surface_size(&entries, st.scale, st.width);
        st.entries = entries;
        st.on_pick = on_pick;
        st.hover = None;
        let margin = SHADOW * st.scale;
        let (w, h) = ((sw + margin * 2.0).ceil() as i32, (sh + margin * 2.0).ceil() as i32);
        unsafe {
            let (left, top) = place(st.anchor.0, st.anchor.1, sw, sh);
            SetWindowPos(st.hwnd, HWND_TOPMOST, left - margin as i32, top - margin as i32, w, h, SWP_NOACTIVATE);
        }
        st.size = (w, h);
        render(st);
    });
}

/// `show`, drawn again once a second from `rebuild` while it is open
pub fn show_live(x: i32, y: i32, rebuild: Rebuild) {
    let (entries, on_pick) = rebuild();
    show(x, y, entries, on_pick);
    MENU.with(|m| {
        if let Some(st) = m.borrow_mut().as_mut() {
            st.rebuild = Some(rebuild);
            unsafe { SetTimer(st.hwnd, REFRESH_TIMER, 1000, None) };
        }
    });
}

/// Show the menu with its corner at the cursor point (x, y) (physical px), kept inside the monitor's work area.
pub fn show(x: i32, y: i32, entries: Vec<Entry>, on_pick: impl Fn(u32) + 'static) {
    close();
    let Some(font) = load_font() else { return };
    let class = wide(CLASS);
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        if !REGISTERED.swap(true, Ordering::SeqCst) {
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinst,
                hIcon: std::ptr::null_mut(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc);
        }
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            x,
            y,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
        let width = menu_width(&font, &entries, scale);
        let (sw, sh) = surface_size(&entries, scale, width);
        let margin = SHADOW * scale;
        let (w, h) = ((sw + margin * 2.0).ceil() as i32, (sh + margin * 2.0).ceil() as i32);

        let (left, top) = place(x, y, sw, sh);
        SetWindowPos(hwnd, HWND_TOPMOST, left - margin as i32, top - margin as i32, w, h, SWP_NOACTIVATE);

        let st = State {
            hwnd,
            entries,
            hover: None,
            scale,
            dark: apps_dark(),
            font,
            width,
            on_pick: Box::new(on_pick),
            size: (w, h),
            tracking: false,
            anchor: (x, y),
            rebuild: None,
        };
        render(&st);
        MENU.with(|m| *m.borrow_mut() = Some(st));
        ShowWindow(hwnd, SW_SHOW);
        // the tray click gave this process the right to take the foreground: clicking elsewhere then
        // deactivates the menu, which closes it
        SetForegroundWindow(hwnd);
    }
}
