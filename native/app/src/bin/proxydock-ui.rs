// proxydock-ui.exe: the ProxyDock window. Started by proxydock.exe (the tray + core process) with
// PROXYDOCK_IPC="<port> <token>"; it draws what the core sends and sends the user's commands back. Closing the
// window ends this process; the cards keep running in the core.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use proxydock::ipc;
use proxydock_core::i18n::{set_language, tr};
use proxydock_core::log::fmt_size;
use serde_json::{json, Value};
use slint::{Color, ComponentHandle, Model, ModelRc, SharedString, VecModel};
use slint_node_editor::{wire_node_editor, wire_selection, GraphLogic, LinkData, LinkPath, MovableNode, NodeEditorSetup};
use std::collections::HashMap;
use std::net::TcpStream;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

slint::include_modules!();

const LOG_CAP: usize = 5000;

impl MovableNode for NodeData {
    fn id(&self) -> i32 { self.id }
    fn x(&self) -> f32 { self.x }
    fn y(&self) -> f32 { self.y }
    fn selected(&self) -> bool { self.selected }
    fn set_x(&mut self, x: f32) { self.x = x; }
    fn set_y(&mut self, y: f32) { self.y = y; }
}

fn pin_in(node: i32, lane: i32) -> i32 {
    node * 64 + lane * 2
}

fn pin_out(node: i32, lane: i32) -> i32 {
    node * 64 + lane * 2 + 1
}

fn strings(v: &Value) -> ModelRc<SharedString> {
    let items: Vec<SharedString> = v.as_array().into_iter().flatten().map(|x| SharedString::from(x.as_str().unwrap_or(""))).collect();
    ModelRc::new(VecModel::from(items))
}

/// "#rrggbb" -> colour; links without one take the app's default wire colour
fn hex_color(text: &str) -> Option<Color> {
    let h = text.strip_prefix('#')?;
    let n = u32::from_str_radix(h, 16).ok().filter(|_| h.len() == 6)?;
    Some(Color::from_rgb_u8((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

const WIRE: Color = Color::from_rgb_u8(90, 160, 240);

/// bytes per second as "1.2 MB/s"
fn rate_text(bps: f64) -> String {
    if bps <= 0.0 {
        return String::new();
    }
    format!("{}/s", fmt_size(bps))
}

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
    nodes: Rc<VecModel<NodeData>>,
    links: Rc<VecModel<LinkData>>,
    /// per link (same order as `links`): its colour, and the link's two block ids (for the rate on blocks)
    link_meta: std::cell::RefCell<Vec<(Color, i32, i32)>>,
    /// canvas key of each block (node id - 1)
    keys: std::cell::RefCell<Vec<String>>,
}

impl Models {
    /// the canvas from the core's description (blocks keyed by block id, links by lanes)
    fn set_canvas(&self, c: &Value) {
        let mut ids: HashMap<String, i32> = HashMap::new();
        let mut nodes = vec![];
        for (i, n) in c["nodes"].as_array().into_iter().flatten().enumerate() {
            let id = i as i32 + 1;
            ids.insert(n["key"].as_str().unwrap_or("").to_string(), id);
            let wires: Vec<Wire> = n["wires"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|w| Wire { i: w[0].as_i64().unwrap_or(0) as i32, o: w[1].as_i64().unwrap_or(0) as i32 })
                .collect();
            nodes.push(NodeData {
                id,
                title: n["title"].as_str().unwrap_or("").into(),
                detail: n["detail"].as_str().unwrap_or("").into(),
                kind: n["kind"].as_i64().unwrap_or(2) as i32,
                x: n["x"].as_f64().unwrap_or(0.0) as f32,
                y: n["y"].as_f64().unwrap_or(0.0) as f32,
                selected: false,
                ins: n["ins"].as_i64().unwrap_or(0).min(31) as i32,
                outs: n["outs"].as_i64().unwrap_or(0).min(31) as i32,
                in_labels: strings(&n["in_labels"]),
                out_labels: strings(&n["out_labels"]),
                wires: ModelRc::new(VecModel::from(wires)),
                card: n["card"].as_str().unwrap_or("").into(),
                state: n["state"].as_str().unwrap_or("").into(),
                rate: SharedString::new(),
                badge: SharedString::new(),
                badge_tone: 0,
            });
        }
        let mut links = vec![];
        let mut meta = vec![];
        for (k, l) in c["links"].as_array().into_iter().flatten().enumerate() {
            let (Some(a), Some(b)) = (ids.get(l["from"].as_str().unwrap_or("")), ids.get(l["to"].as_str().unwrap_or(""))) else {
                continue;
            };
            let color = hex_color(l["color"].as_str().unwrap_or("")).unwrap_or(WIRE);
            let (out, inp) = (l["out"].as_i64().unwrap_or(0) as i32, l["in"].as_i64().unwrap_or(0) as i32);
            links.push(LinkData::new(k as i32 + 1, pin_out(*a, out), pin_in(*b, inp), color));
            meta.push((color, *a, *b));
        }
        let mut keys = vec![String::new(); ids.len()];
        for (k, id) in &ids {
            keys[*id as usize - 1] = k.clone();
        }
        self.nodes.set_vec(nodes);
        self.links.set_vec(links);
        *self.link_meta.borrow_mut() = meta;
        *self.keys.borrow_mut() = keys;
        self.apply_badges();
    }

    /// the latency badges (kept from the core's last "latency") on their blocks
    fn apply_badges(&self) {
        let keys = self.keys.borrow();
        BADGES.with(|b| {
            let b = b.borrow();
            for i in 0..self.nodes.row_count() {
                let Some(mut n) = self.nodes.row_data(i) else { continue };
                let key = keys.get(i).map(String::as_str).unwrap_or("");
                let (text, tone) = b.get(key).cloned().unwrap_or_default();
                if n.badge != text.as_str() || n.badge_tone != tone {
                    n.badge = text.into();
                    n.badge_tone = tone;
                    self.nodes.set_row_data(i, n);
                }
            }
        });
    }

    /// a card's state on its canvas block
    fn set_node_state(&self, card: &str, state: &str) {
        for i in 0..self.nodes.row_count() {
            let Some(mut n) = self.nodes.row_data(i) else { continue };
            if n.card == card && n.state != state {
                n.state = state.into();
                self.nodes.set_row_data(i, n);
            }
        }
    }

    /// bytes per second on each link: links with traffic get wider and brighter, blocks show what passes them
    fn set_flow(&self, rates: &[Value]) {
        let meta = self.link_meta.borrow();
        let mut per_node: HashMap<i32, (f64, f64)> = HashMap::new(); // (in, out)
        for i in 0..self.links.row_count() {
            let Some(mut l) = self.links.row_data(i) else { continue };
            let Some((color, a, b)) = meta.get(i).copied() else { continue };
            let r = rates.get(l.id as usize - 1).and_then(Value::as_f64).unwrap_or(0.0);
            per_node.entry(a).or_default().1 += r;
            per_node.entry(b).or_default().0 += r;
            // 2 px idle; with traffic 3..8 px on a log scale (1 KB/s .. 100 MB/s)
            let (width, c) = if r > 0.0 {
                let t = ((r.max(1024.0).log10() - 3.0) / 5.0).clamp(0.0, 1.0) as f32;
                (3.0 + 5.0 * t, color.brighter(0.35))
            } else {
                (2.0, color)
            };
            if l.line_width != width || l.color != c {
                l.line_width = width;
                l.color = c;
                self.links.set_row_data(i, l);
            }
        }
        for i in 0..self.nodes.row_count() {
            let Some(mut n) = self.nodes.row_data(i) else { continue };
            let (inp, out) = per_node.get(&n.id).copied().unwrap_or_default();
            // what passes a block: what comes in, or for a card (nothing comes in) what leaves it
            let text: SharedString = rate_text(if inp > 0.0 { inp } else { out }).into();
            if n.rate != text {
                n.rate = text;
                self.nodes.set_row_data(i, n);
            }
        }
    }

    fn set_card(&self, c: &Value) {
        self.set_node_state(c["id"].as_str().unwrap_or(""), c["state"].as_str().unwrap_or(""));
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

/// the open block editor: the block, the values typed so far, each choice field's option values
#[derive(Default)]
struct Editor {
    id: String,
    back_to: i32,
    values: serde_json::Map<String, Value>,
    options: HashMap<String, Vec<String>>,
    rerender: std::collections::HashSet<String>,
    kinds: HashMap<String, String>,
}

thread_local! {
    static EDIT_REQUEST: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    static EDITOR: std::cell::RefCell<Editor> = std::cell::RefCell::new(Editor::default());
}

fn field_rows(fields: &[Value]) -> Vec<FieldRow> {
    EDITOR.with(|e| {
        let mut e = e.borrow_mut();
        e.options.clear();
        e.rerender.clear();
        e.kinds.clear();
        fields
            .iter()
            .map(|f| {
                let key = f["key"].as_str().unwrap_or("").to_string();
                let kind = f["kind"].as_str().unwrap_or("text").to_string();
                let opts: Vec<(String, String)> = f["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|o| (o[0].as_str().unwrap_or("").to_string(), o[1].as_str().unwrap_or("").to_string()))
                    .collect();
                let value = &f["value"];
                let text = match value {
                    Value::String(t) => t.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                let index = opts.iter().position(|(v, _)| *v == text).map_or(-1, |i| i as i32);
                // an item's buttons: labels for the window, actions kept here
                let buttons: Vec<SharedString> = if kind == "item" { opts.iter().map(|(_, l)| SharedString::from(l.as_str())).collect() } else { vec![] };
                if f["rerender"] == json!(true) {
                    e.rerender.insert(key.clone());
                }
                e.options.insert(key.clone(), opts.iter().map(|(v, _)| v.clone()).collect());
                e.kinds.insert(key.clone(), kind.clone());
                FieldRow {
                    key: key.into(),
                    label: f["label"].as_str().unwrap_or("").into(),
                    desc: f["desc"].as_str().unwrap_or("").into(),
                    kind: kind.into(),
                    text: text.into(),
                    checked: value == &json!(true),
                    options: ModelRc::new(VecModel::from(opts.into_iter().map(|(_, l)| SharedString::from(l)).collect::<Vec<_>>())),
                    index,
                    visible: f["visible"] == json!(true),
                    buttons: ModelRc::new(VecModel::from(buttons)),
                    busy: f["busy"] == json!(true),
                    tone: f["tone"].as_i64().unwrap_or(0) as i32,
                }
            })
            .collect()
    })
}

thread_local! {
    /// latency badges by block key: (text, tone)
    static BADGES: std::cell::RefCell<HashMap<String, (String, i32)>> = std::cell::RefCell::new(HashMap::new());
    /// "Quit with the X button"
    static CLOSE_EXITS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static NOTICE_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static TOAST_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// the block whose title the rename box edits
    static RENAME_KEY: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// a message at the bottom of the canvas for `secs` (a newer one replaces it and its timer)
fn show_toast(app: &App, text: &str, info: bool, secs: f32) {
    app.set_toast(text.into());
    app.set_toast_info(info);
    TOAST_SEQ.with(|n| n.set(n.get() + 1));
    let seq = TOAST_SEQ.with(|n| n.get());
    let weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_secs_f32(secs), move || {
        if let Some(a) = weak.upgrade() {
            if TOAST_SEQ.with(|n| n.get()) == seq {
                a.set_toast(SharedString::new());
            }
        }
    });
}

/// the text on the clipboard ("" if there is none)
fn clipboard_text() -> String {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
        use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
        const CF_UNICODETEXT: u32 = 13;
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return String::new();
        }
        let mut out = String::new();
        let h = GetClipboardData(CF_UNICODETEXT);
        if !h.is_null() {
            let p = GlobalLock(h) as *const u16;
            if !p.is_null() {
                let mut n = 0;
                while *p.add(n) != 0 && n < 1 << 20 {
                    n += 1;
                }
                out = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
        out
    }
    #[cfg(not(windows))]
    String::new()
}

/// the settings page from the core's "settings" (values, choices, what it found)
fn show_settings(app: &App, v: &Value) {
    let st = app.global::<Settings>();
    let vals = &v["values"];
    let b = |k: &str| vals[k] == json!(true);
    let i = |k: &str| vals[k].as_i64().unwrap_or(0) as i32;
    let list = |k: &str| -> ModelRc<SharedString> { ModelRc::new(VecModel::from(strings_of(&v[k]))) };
    st.set_languages(list("languages"));
    st.set_themes(list("themes"));
    st.set_scales(list("scales"));
    st.set_allowed(vals["allowed_domains"].as_str().unwrap_or("").into());
    st.set_private(b("block_private"));
    st.set_timeout(i("timeout"));
    st.set_logconn(b("log_connections"));
    st.set_logclose(b("log_close"));
    st.set_dns(vals["dns_server"].as_str().unwrap_or("").into());
    st.set_dnsvia(b("dns_via_outbound"));
    st.set_logfile(b("log_to_file"));
    st.set_language(i("language"));
    st.set_theme(i("theme"));
    st.set_scale(i("ui_scale"));
    st.set_logsize(i("log_font_size"));
    st.set_close(b("close_exits"));
    st.set_latest(b("singbox_latest"));
    st.set_saved_allowed(vals["allowed_domains"].as_str().unwrap_or("").into());
    st.set_saved_private(b("block_private"));
    st.set_saved_timeout(i("timeout"));
    st.set_saved_logconn(b("log_connections"));
    st.set_saved_logclose(b("log_close"));
    st.set_saved_dns(vals["dns_server"].as_str().unwrap_or("").into());
    st.set_saved_dnsvia(b("dns_via_outbound"));
    st.set_saved_logfile(b("log_to_file"));
    st.set_saved_language(i("language"));
    st.set_saved_theme(i("theme"));
    st.set_saved_scale(i("ui_scale"));
    st.set_saved_logsize(i("log_font_size"));
    st.set_saved_close(b("close_exits"));
    st.set_saved_latest(b("singbox_latest"));
    st.set_saved_admin(b("run_as_admin"));
    st.set_check_updates(b("check_updates"));
    st.set_saved_check_updates(b("check_updates"));
    st.set_sizes(list("text_sizes"));
    st.set_text_size(i("text_size"));
    st.set_saved_text_size(i("text_size"));
    // the fonts: by name in this window's lists of the PC's fonts (0 = built in)
    let (ui_font, mono_font) = (vals["ui_font"].as_str().unwrap_or("").to_string(), vals["mono_font"].as_str().unwrap_or("").to_string());
    FONTS.with(|f| {
        let mut f = f.borrow_mut();
        f.ui_choice = ui_font.clone();
        let code = proxydock_core::i18n::current();
        let builtin = tr("Built-in · {builtin_ui}").replace("{builtin_ui}", if lang_font(&code).is_empty() { system_ui_font() } else { lang_font(&code) });
        let ui_names: Vec<SharedString> = std::iter::once(SharedString::from(builtin)).chain(f.ui.iter().map(|x| SharedString::from(x.as_str()))).collect();
        let mono_names: Vec<SharedString> = std::iter::once(SharedString::from(tr("Built-in · {builtin_mono}").replace("{builtin_mono}", "JetBrains Mono")))
            .chain(f.mono.iter().map(|x| SharedString::from(x.as_str())))
            .collect();
        st.set_ui_fonts(ModelRc::new(VecModel::from(ui_names)));
        st.set_mono_fonts(ModelRc::new(VecModel::from(mono_names)));
        let ui_i = f.ui.iter().position(|x| *x == ui_font).map_or(0, |k| k as i32 + 1);
        let mono_i = f.mono.iter().position(|x| *x == mono_font).map_or(0, |k| k as i32 + 1);
        st.set_ui_font(ui_i);
        st.set_saved_ui_font(ui_i);
        st.set_mono_font(mono_i);
        st.set_saved_mono_font(mono_i);
    });
    // text size: Python's 115% is this window's own size
    let size = proxydock_core::config::TEXT_SIZES.get(i("text_size").max(0) as usize).copied().unwrap_or(115);
    app.global::<Type>().set_scale(size as f32 / 115.0);
    app.global::<Type>().set_mono(if mono_font.is_empty() { "JetBrains Mono".into() } else { mono_font.as_str().into() });
    apply_fonts(app, &proxydock_core::i18n::current());
    st.set_admin(b("run_as_admin"));
    st.set_windows(v["windows"] == json!(true));
    st.set_login(v["startup"]["registered"] == json!(true));
    st.set_hidden(v["startup"]["hidden"] == json!(true));
    let fw = match v["firewall"].as_bool() {
        Some(true) => 1,
        Some(false) => 0,
        None => -1,
    };
    st.set_fw_state(fw);
    st.set_fw_text(tr(match fw { 1 => "On", 0 => "Off", _ => "Checking…" }).into());
    st.set_engine(v["engine"].as_str().unwrap_or("").into());
    st.set_log_dir(v["log_dir"].as_str().unwrap_or("").into());
    // what applies to this window right away
    app.set_theme(i("theme"));
    app.set_log_px(i("log_font_size") as f32 * 1.2);
    CLOSE_EXITS.with(|c| c.set(b("close_exits")));
}

/// the core's latency badges and the test's progress
fn show_latency(app: &App, m: &Models, v: &Value) {
    BADGES.with(|b| {
        let mut b = b.borrow_mut();
        b.clear();
        for (k, x) in v["badges"].as_object().into_iter().flatten() {
            b.insert(k.clone(), (x[0].as_str().unwrap_or("").to_string(), x[1].as_i64().unwrap_or(0) as i32));
        }
    });
    m.apply_badges();
    app.set_latency_progress(v["progress"].as_str().unwrap_or("").into());
}

/// The font a language's UI is usually set in, in its country: Korean Pretendard (bundled), Japanese and Chinese
/// the system's UI font for them (Python `_CJK_UI_FONTS`; the bundled ones would cost 13-17 MB each), the other
/// languages the system's own UI font (""). Characters a font lacks come from the system's fallback fonts.
fn lang_font(code: &str) -> &'static str {
    let windows = cfg!(windows);
    let mac = cfg!(target_os = "macos");
    match code {
        "ko" => "Pretendard Variable",
        "ja" => if windows { "Yu Gothic UI" } else if mac { "Hiragino Sans" } else { "Noto Sans CJK JP" },
        "zh_CN" => if windows { "Microsoft YaHei UI" } else if mac { "PingFang SC" } else { "Noto Sans CJK SC" },
        "zh_TW" => if windows { "Microsoft JhengHei UI" } else if mac { "PingFang TC" } else { "Noto Sans CJK TC" },
        // Segoe UI on Windows, San Francisco on macOS, the desktop's font elsewhere
        _ => "",
    }
}

/// the PC's font families: all, and the fixed-width ones (for the log); and the UI font the user chose
#[derive(Default)]
struct Fonts {
    ui: Vec<String>,
    mono: Vec<String>,
    ui_choice: String,
}

thread_local! {
    static FONTS: std::cell::RefCell<Fonts> = std::cell::RefCell::new(Fonts::default());
}

/// the font families installed on this PC (Windows GDI), sorted; the fixed-width ones apart
fn system_fonts() -> (Vec<String>, Vec<String>) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Graphics::Gdi::{EnumFontFamiliesExW, GetDC, ReleaseDC, DEFAULT_CHARSET, LOGFONTW, TEXTMETRICW};
        unsafe extern "system" fn each(lf: *const LOGFONTW, _tm: *const TEXTMETRICW, _ty: u32, data: isize) -> i32 {
            let (out, lf) = unsafe { (&mut *(data as *mut Vec<(String, bool)>), &*lf) };
            let n = lf.lfFaceName.iter().position(|c| *c == 0).unwrap_or(32);
            let name = String::from_utf16_lossy(&lf.lfFaceName[..n]);
            if !name.starts_with('@') && !name.is_empty() {
                out.push((name, lf.lfPitchAndFamily & 3 == 1));
            }
            1
        }
        let mut found: Vec<(String, bool)> = vec![];
        unsafe {
            let dc = GetDC(std::ptr::null_mut());
            let mut lf: LOGFONTW = std::mem::zeroed();
            lf.lfCharSet = DEFAULT_CHARSET as u8;
            EnumFontFamiliesExW(dc, &lf, Some(each), &mut found as *mut _ as isize, 0);
            ReleaseDC(std::ptr::null_mut(), dc);
        }
        found.sort_by_key(|(n, _)| n.to_lowercase());
        found.dedup_by(|a, b| a.0 == b.0);
        let mono = found.iter().filter(|(_, m)| *m).map(|(n, _)| n.clone()).collect();
        (found.into_iter().map(|(n, _)| n).collect(), mono)
    }
    #[cfg(not(windows))]
    (vec![], vec![])
}

/// the system's own UI font, for "Built-in" in languages that use it
fn system_ui_font() -> &'static str {
    if cfg!(windows) { "Segoe UI" } else if cfg!(target_os = "macos") { "SF Pro" } else { "Sans" }
}

/// the window's font for the language, and each language's name in its own font
fn apply_fonts(app: &App, code: &str) {
    // the user's UI font, else the language's own
    let chosen = FONTS.with(|f| f.borrow().ui_choice.clone());
    app.set_ui_font(if chosen.is_empty() { lang_font(code).into() } else { chosen.as_str().into() });
    let fonts: Vec<SharedString> = proxydock_core::i18n::LANGUAGES.iter().map(|(c, _)| SharedString::from(lang_font(c))).collect();
    app.global::<Settings>().set_language_fonts(ModelRc::new(VecModel::from(fonts)));
}

fn strings_of(v: &Value) -> Vec<SharedString> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str).map(SharedString::from).collect()
}

/// the settings page's texts (Python `SettingsInterface`)
fn settings_text() -> SettingsText {
    use proxydock_core::singbox::latest;
    let t = |s: &str| SharedString::from(tr(s));
    SettingsText {
        title: t("Settings"),
        apply: t("Apply"),
        changed: t("Settings have unsaved changes"),
        revert: t("Revert"),
        defaults: t("Proxy defaults (apply to items set to 'Use global' in each proxy)"),
        allowed: t("Allowed domains"),
        allowed_desc: t("Comma separated · * = all · *.example.com = including subdomains"),
        private: t("Block private network destinations"),
        private_desc: t("Refuses connections to loopback, private and link-local addresses"),
        timeout: t("Idle timeout (s)"),
        timeout_desc: t("Time after which an idle TCP connection is closed"),
        logconn: t("Log connections"),
        logconn_desc: t("When off, successful connections are not logged (errors always are)"),
        logclose: t("Log connection close details"),
        logclose_desc: t("Upload/download amount, duration and reason when a connection ends"),
        dns: t("DNS server"),
        dns_desc: SharedString::from(format!("{} {}", tr("Empty = the PC's DNS. Otherwise 1.1.1.1 (UDP), tcp://, tls:// (DoT), https://…/dns-query (DoH), quic:// or h3://. Used where ProxyDock looks names up itself (Direct, IP rules, TUN). Needs the sing-box engine"), tr("Several servers (separated by commas): all are asked at once and the first answer is used."))),
        dnsvia: t("DNS through the outbound"),
        dnsvia_desc: t("The card's own lookups go through its tunnel or upstream instead of leaving this PC directly (needs a DNS server above)"),
        engine: t("Proxy engine"),
        latest: t(latest::SETTING_LABEL),
        latest_badge: t(latest::SETTING_BADGE),
        latest_desc: t(latest::SETTING_HELP),
        logs: t("Logs"),
        logfile: t("Save to file"),
        open_folder: t("Open folder"),
        display: t("Display"),
        language: t("Language"),
        language_desc: t("Applies right away"),
        theme: t("Theme"),
        theme_desc: t("Follow the system or force light/dark"),
        scale: t("App scale"),
        scale_desc: t("Scales text and UI elements together (applies after the window is opened again)"),
        logsize: t("Log font size"),
        logsize_desc: t("Font size of the log and license views (pt)"),
        close: t("Quit with the X button"),
        close_desc: t("When off, the app keeps running in the tray after X"),
        text_size: t("Text size"),
        text_size_desc: t("Size of the text in menus and pages (applies right away)"),
        fonts: t("Fonts"),
        ui_font: t("UI font"),
        ui_font_desc: t("Menus, settings and the whole window"),
        mono_font: t("Log font"),
        mono_font_desc: t("Monospace font of the log view (CJK text uses the UI font)"),
        startup: t("Startup (applied immediately)"),
        login: t("Start at login"),
        login_desc: t("Runs ProxyDock automatically when you sign in"),
        hidden: t("Hide startup notification"),
        hidden_desc: t("Do not show the 'running' notification when started automatically"),
        admin: t("Run as administrator"),
        admin_desc: t("ProxyDock restarts itself with administrator rights (UAC prompt) when it starts, so TUN cards start without asking. Applies from the next start."),
        updates: t("Updates"),
        check_updates: t("Check for updates at startup"),
        check_updates_desc: t("Asks GitHub for the latest release at most once a day. Only the app version is sent (as the User-Agent)."),
        firewall: t("Windows Firewall"),
        fw_block: t("Block external access"),
        fw_block_desc: t("Blocks connections from other PCs to ProxyDock (all proxies, needs administrator rights). Turn off to use servers for other devices (Shadowsocks, Hysteria2, etc.)"),
        fw_on: t("Turn block on"),
        fw_off: t("Turn block off"),
    }
}

fn labels(app: &App) {
    app.global::<Settings>().set_text(settings_text());
    stats_labels(app);
    conn_labels(app);
    info_labels(app, &proxydock_core::i18n::current());
    let u = app.global::<Upd>();
    u.set_t_check(tr("Check for updates").into());
    u.set_t_details(tr("Details").into());
    u.set_t_later(tr("Later").into());
    u.set_t_downloading(tr("Downloading the update").into());
    u.set_t_cancel(tr("Cancel").into());
    u.set_t_install_title(tr("Install the update?").into());
    u.set_t_install_text(tr("ProxyDock closes, the installer updates it and then starts it again. Running proxies stop until then.").into());
    u.set_t_install(tr("Install").into());
    app.set_t_latency(tr("Test latency").into());
    app.set_t_close(tr("Close").into());
    app.set_t_save(tr("Save").into());
    app.set_t_cancel(tr("Cancel").into());
    app.set_t_add_inbound(tr("Inbound").into());
    app.set_t_add_router("Router".into());
    app.set_t_add_relay(tr("Pass-through").into());
    app.set_t_add_outbound(tr("Outbound").into());
    app.set_t_delete(tr("Delete").into());
    app.set_t_undo(tr("Undo").into());
    app.set_t_redo(tr("Redo").into());
    app.set_t_rename(tr("Rename").into());
    app.set_t_duplicate(tr("Duplicate").into());
    app.set_t_canvas(tr("Canvas").into());
    app.set_t_no_graph(tr("No proxies yet. Create one with 'New proxy'.").into());
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
    FONTS.with(|f| {
        let (ui, mono) = system_fonts();
        let mut f = f.borrow_mut();
        f.ui = ui;
        f.mono = mono;
    });
    let models = Rc::new(Models {
        cards: Rc::new(VecModel::default()),
        logs: Rc::new(VecModel::default()),
        nodes: Rc::new(VecModel::default()),
        links: Rc::new(VecModel::default()),
        link_meta: Default::default(),
        keys: Default::default(),
    });
    app.set_cards(ModelRc::from(models.cards.clone()));
    app.set_logs(ModelRc::from(models.logs.clone()));
    app.set_nodes(ModelRc::from(models.nodes.clone()));
    let send = {
        let w = writer.clone();
        move |v: Value| {
            let _ = ipc::send(&mut w.lock().unwrap(), &v);
        }
    };
    let key_of = {
        let models = models.clone();
        move |node: i32| models.keys.borrow().get((node - 1).max(0) as usize).cloned().unwrap_or_default()
    };
    app.set_links(ModelRc::from(models.links.clone()));
    // canvas: moving and selecting blocks (positions stay in this window until saving settings is ported)
    {
        let nodes = models.nodes.clone();
        let links = models.links.clone();
        let setup = NodeEditorSetup::new({
            let (nodes, send, key_of) = (nodes.clone(), send.clone(), key_of.clone());
            move |dragged, dx, dy| {
                GraphLogic::commit_drag(&nodes, dragged, dx, dy);
                let moved: Vec<Value> = nodes
                    .iter()
                    .filter(|n| n.id == dragged || n.selected)
                    .map(|n| json!({"key": key_of(n.id), "x": n.x, "y": n.y}))
                    .collect();
                send(json!({"cmd": "move", "blocks": moved}));
            }
        });
        let controller = setup.controller().clone();
        wire_node_editor!(app, setup);
        wire_selection!(app, setup, nodes, links);
        app.on_compute_link_at({
            let links = links.clone();
            move |x, y| {
                let rows = (0..links.row_count()).filter_map(|i| {
                    let l = links.row_data(i)?;
                    Some((l.id, l.start_pin_id, l.end_pin_id))
                });
                controller.cache().borrow().find_bezier_link_at_world(x, y, rows, controller.screen_distance_to_world(8.0), 50.0, 20)
            }
        });
        // a link dragged between two pins: the core decides (and answers with the new canvas or a reason)
        {
            let (send, key_of) = (send.clone(), key_of.clone());
            app.on_link_requested(move |a, b| {
                let (out, inp) = if a % 2 == 1 && b % 2 == 0 { (a, b) } else if b % 2 == 1 && a % 2 == 0 { (b, a) } else { return };
                if out / 64 == inp / 64 {
                    return;
                }
                send(json!({"cmd": "connect", "from": key_of(out / 64), "out": (out % 64) / 2, "to": key_of(inp / 64), "in": (inp % 64) / 2}));
            });
        }
    }
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
    // block editor
    {
        let (send1, key_of) = (send.clone(), key_of.clone());
        app.on_edit_block(move |node| send1(json!({"cmd": "edit_open", "id": key_of(node)})));
        let send2 = send.clone();
        app.on_edit_card(move |id| send2(json!({"cmd": "edit_open", "id": id.as_str()})));
    }
    {
        let send = send.clone();
        app.on_field_changed(move |key, value| {
            let msg = EDITOR.with(|e| {
                let mut e = e.borrow_mut();
                let key = key.to_string();
                let v = match e.kinds.get(&key).map(String::as_str) {
                    Some("bool") => json!(value == "true"),
                    Some("choice") => {
                        let i = value.parse::<usize>().unwrap_or(0);
                        json!(e.options.get(&key).and_then(|o| o.get(i)).cloned().unwrap_or_default())
                    }
                    _ => json!(value.as_str()),
                };
                e.values.insert(key.clone(), v);
                // the visible fields depend on this one: ask for the form again
                e.rerender.contains(&key).then(|| json!({"cmd": "edit_preview", "id": e.id, "values": e.values}))
            });
            if let Some(m) = msg {
                send(m);
            }
        });
    }
    {
        let send_s = send.clone();
        app.on_form_save(move || {
            let msg = EDITOR.with(|e| {
                let e = e.borrow();
                json!({"cmd": "edit_save", "id": e.id, "values": e.values})
            });
            send_s(msg);
        });
        let send_p = send.clone();
        app.on_form_pressed(move |key, i| {
            let msg = EDITOR.with(|e| {
                let e = e.borrow();
                // a plain button's key is its action; an item's button n has the action in its options
                let action = if i < 0 { key.to_string() } else { e.options.get(key.as_str()).and_then(|o| o.get(i as usize)).cloned().unwrap_or_default() };
                let mut values = e.values.clone();
                if action == "link.paste" {
                    values.insert("clipboard".into(), json!(clipboard_text()));
                }
                json!({"cmd": "edit_action", "id": e.id, "action": action, "values": values})
            });
            send_p(msg);
        });
        let send_c = send.clone();
        let weak = app.as_weak();
        app.on_form_cancel(move || {
            send_c(json!({"cmd": "edit_close"}));
            if let Some(a) = weak.upgrade() {
                a.set_page(EDITOR.with(|e| e.borrow().back_to));
            }
        });
    }
    {
        let (send, models, weak) = (send.clone(), models.clone(), app.as_weak());
        app.on_add_block(move |kind| {
            let Some(a) = weak.upgrade() else { return };
            // centred on the visible part of the canvas, a little staggered
            let k = models.nodes.row_count() as f32 % 5.0 * 24.0;
            send(json!({"cmd": "add", "kind": kind, "x": a.get_view_center_x() - 115.0 + k, "y": a.get_view_center_y() - 34.0 + k}));
        });
    }
    {
        let (send, models, key_of) = (send.clone(), models.clone(), key_of.clone());
        app.on_delete_selected(move || {
            // one message (one undo step): selected links are cut first, then selected blocks deleted
            let links: Vec<Value> = models
                .links
                .iter()
                .filter(|l| l.selected)
                .map(|l| {
                    let (a, b) = (l.start_pin_id, l.end_pin_id);
                    json!({"from": key_of(a / 64), "out": (a % 64) / 2, "to": key_of(b / 64), "in": (b % 64) / 2})
                })
                .collect();
            let keys: Vec<String> = models.nodes.iter().filter(|n| n.selected).map(|n| key_of(n.id)).collect();
            if !keys.is_empty() || !links.is_empty() {
                send(json!({"cmd": "delete", "keys": keys, "links": links}));
            }
        });
    }
    {
        let send = send.clone();
        app.on_undo({
            let send = send.clone();
            move || send(json!({"cmd": "undo"}))
        });
        app.on_redo({
            let send = send.clone();
            move || send(json!({"cmd": "redo"}))
        });
        let (models2, key_of2) = (models.clone(), key_of.clone());
        let send2 = send.clone();
        app.on_duplicate_selected(move || {
            let keys: Vec<String> = models2.nodes.iter().filter(|n| n.selected).map(|n| key_of2(n.id)).filter(|k| k != "pc").collect();
            if !keys.is_empty() {
                send2(json!({"cmd": "duplicate", "keys": keys}));
            }
        });
        let (models3, key_of3, weak) = (models.clone(), key_of.clone(), app.as_weak());
        app.on_rename_selected(move || {
            // one selected block (not the PC block): its title in the rename box
            let sel: Vec<NodeData> = models3.nodes.iter().filter(|n| n.selected).collect();
            let [n] = sel.as_slice() else { return };
            let key = key_of3(n.id);
            if key == "pc" {
                return;
            }
            let Some(a) = weak.upgrade() else { return };
            RENAME_KEY.with(|k| *k.borrow_mut() = key);
            a.set_rename_text(n.title.clone());
            a.set_rename_open(true);
        });
        app.on_rename_apply(move |text| {
            let key = RENAME_KEY.with(|k| k.borrow().clone());
            if !key.is_empty() {
                send(json!({"cmd": "rename", "key": key, "name": text.as_str()}));
            }
        });
    }
    {
        let (send, models, key_of) = (send.clone(), models.clone(), key_of.clone());
        app.on_test_latency(move || {
            // the selected outbound blocks, or (none selected) every one
            let keys: Vec<String> = models.nodes.iter().filter(|n| n.selected).map(|n| key_of(n.id)).collect();
            send(json!({"cmd": "latency", "keys": keys}));
        });
    }
    {
        let st = app.global::<Settings>();
        let send_l = send.clone();
        st.on_set_login(move |on| send_l(json!({"cmd": "startup", "on": on})));
        let send_h = send.clone();
        st.on_set_hidden(move |on| send_h(json!({"cmd": "startup_hidden", "on": on})));
        let send_f = send.clone();
        st.on_firewall(move |on| send_f(json!({"cmd": "firewall", "on": on})));
        let (send, weak) = (send.clone(), app.as_weak());
        st.on_apply(move || {
            let Some(a) = weak.upgrade() else { return };
            let st = a.global::<Settings>();
            send(json!({"cmd": "settings_save", "values": {
                "allowed_domains": st.get_allowed().as_str(), "block_private": st.get_private(), "timeout": st.get_timeout(),
                "log_connections": st.get_logconn(), "log_close": st.get_logclose(), "dns_server": st.get_dns().as_str(),
                "dns_via_outbound": st.get_dnsvia(), "log_to_file": st.get_logfile(), "language": st.get_language(),
                "theme": st.get_theme(), "ui_scale": st.get_scale(), "log_font_size": st.get_logsize(),
                "close_exits": st.get_close(), "singbox_latest": st.get_latest(), "run_as_admin": st.get_admin(), "check_updates": st.get_check_updates(),
                "text_size": st.get_text_size(),
                "ui_font": FONTS.with(|f| f.borrow().ui.get((st.get_ui_font() - 1).max(-1) as usize).cloned().filter(|_| st.get_ui_font() > 0).unwrap_or_default()),
                "mono_font": FONTS.with(|f| f.borrow().mono.get((st.get_mono_font() - 1).max(-1) as usize).cloned().filter(|_| st.get_mono_font() > 0).unwrap_or_default()),
            }}));
        });
        let weak = app.as_weak();
        st.on_open_log_dir(move || {
            let Some(a) = weak.upgrade() else { return };
            let dir = a.global::<Settings>().get_log_dir().to_string();
            let _ = std::fs::create_dir_all(&dir);
            if let Err(e) = std::process::Command::new("explorer").arg(&dir).spawn() {
                show_toast(&a, &format!("{}: {e}", tr("Cannot open the folder")), false, 5.0);
            }
        });
    }
    {
        // "Quit with the X button": the whole app quits, else only this window closes (the tray keeps running)
        let send = send.clone();
        app.window().on_close_requested(move || {
            if CLOSE_EXITS.with(|c| c.get()) {
                send(json!({"cmd": "quit"}));
            }
            slint::CloseRequestResponse::HideWindow
        });
    }
    {
        // statistics: another card or graph picked
        let (models, weak) = (models.clone(), app.as_weak());
        app.global::<Stats>().on_changed(move || {
            if let Some(a) = weak.upgrade() {
                stats_render(&a, &running_cards(&models));
            }
        });
        // connection test: start after a short pause (against rapid clicks), sites added and removed
        let c = app.global::<Conn>();
        let (send1, weak) = (send.clone(), app.as_weak());
        c.on_start(move || {
            let Some(a) = weak.upgrade() else { return };
            let busy = CONN.with(|s| {
                let s = s.borrow();
                s.running || s.preparing || s.cooldown > 0
            });
            if busy || conn_selected(&a).is_none() {
                return;
            }
            CONN.with(|s| s.borrow_mut().preparing = true);
            conn_sync(&a);
            let (send, weak) = (send1.clone(), a.as_weak());
            slint::Timer::single_shot(std::time::Duration::from_millis(300), move || {
                let Some(a) = weak.upgrade() else { return };
                match conn_selected(&a) {
                    Some((id, _)) => send(json!({"cmd": "conn_test", "id": id})),
                    None => {
                        CONN.with(|s| s.borrow_mut().preparing = false);
                        conn_sync(&a);
                    }
                }
            });
        });
        let (send2, weak) = (send.clone(), app.as_weak());
        c.on_add(move || {
            if let Some(a) = weak.upgrade() {
                send2(json!({"cmd": "test_site_add", "text": a.global::<Conn>().get_site().as_str()}));
            }
        });
        let send3 = send.clone();
        c.on_remove(move |host| send3(json!({"cmd": "test_site_remove", "host": host.as_str()})));
        let weak = app.as_weak();
        c.on_proxy_changed(move || {
            if let Some(a) = weak.upgrade() {
                conn_sync(&a);
            }
        });
    }
    {
        // how to use, about, licenses
        let i = app.global::<Info>();
        let weak = app.as_weak();
        i.on_open_url(move |url| {
            // only web addresses (the how-to's links and the project's pages)
            if url.starts_with("https://") {
                if let (Err(e), Some(a)) = (open_with_system(&url), weak.upgrade()) {
                    show_toast(&a, &e.to_string(), false, 5.0);
                }
            }
        });
        let weak = app.as_weak();
        i.on_open_dir(move |dir| {
            let _ = std::fs::create_dir_all(dir.as_str());
            if let (Err(e), Some(a)) = (open_with_system(&dir), weak.upgrade()) {
                show_toast(&a, &format!("{}: {e}", tr("Cannot open the folder")), false, 5.0);
            }
        });
        let weak = app.as_weak();
        i.on_show_licenses(move || {
            if let Some(a) = weak.upgrade() {
                show_license(&a);
                a.set_page(9);
            }
        });
        let weak = app.as_weak();
        i.on_lic_picked(move || {
            if let Some(a) = weak.upgrade() {
                show_license(&a);
            }
        });
        let weak = app.as_weak();
        i.on_lic_open_folder(move || {
            if let (Err(e), Some(a)) = (open_with_system(&license_dir().display().to_string()), weak.upgrade()) {
                show_toast(&a, &format!("{}: {e}", tr("Cannot open the folder")), false, 5.0);
            }
        });
    }
    {
        // updates: check, then download (or the release page), then install
        let u = app.global::<Upd>();
        let send_c = send.clone();
        u.on_check(move || send_c(json!({"cmd": "update_check"})));
        let (send_a, weak) = (send.clone(), app.as_weak());
        u.on_act(move || {
            match UPDATE_PAGE.with(|p| p.borrow().clone()) {
                Some(page) => {
                    let _ = open_with_system(&page);
                }
                None => {
                    if let Some(a) = weak.upgrade() {
                        let u = a.global::<Upd>();
                        u.set_progress(0.0);
                        u.set_progress_text(tr("Starting…").into());
                        u.set_downloading(true);
                    }
                    send_a(json!({"cmd": "update_download"}));
                }
            }
        });
        let send_x = send.clone();
        u.on_cancel(move || send_x(json!({"cmd": "update_cancel"})));
        let send_i = send.clone();
        u.on_install(move || {
            let path = UPDATE_FILE.with(|f| f.borrow().clone());
            send_i(json!({"cmd": "update_install", "path": path}));
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
                            apply_fonts(&app, v["language"].as_str().unwrap_or("en"));
                            labels(&app);
                            m.cards.set_vec(v["cards"].as_array().into_iter().flatten().map(card_row).collect::<Vec<_>>());
                            m.add_logs(v["logs"].as_array().map(Vec::as_slice).unwrap_or(&[]));
                            m.set_stats(&v["stats"]);
                            show_latency(&app, &m, &v["latency"]);
                            m.set_canvas(&v["canvas"]);
                            show_settings(&app, &v["settings"]);
                            stats_history(&v["history"]);
                            stats_push(&v["stats"], &Value::Null);
                            conn_sites(&app, &v["conn"]);
                            conn_cards(&app, &m);
                            show_about(&app, &v["about"]);
                            show_update(&app, &v["update"]);
                            stats_render(&app, &running_cards(&m));
                            app.set_can_undo(v["undo"] == json!(true));
                            app.set_can_redo(v["redo"] == json!(true));
                            // debug builds: PROXYDOCK_EDIT=<block id>[,<button>] opens its editor (screenshots without clicking)
                            if cfg!(debug_assertions) {
                                if let Ok(id) = std::env::var("PROXYDOCK_EDIT") {
                                    EDIT_REQUEST.with(|r| *r.borrow_mut() = Some(id));
                                }
                            }
                        }
                        "flow" => m.set_flow(v["links"].as_array().map(Vec::as_slice).unwrap_or(&[])),
                        "graph" => {
                            m.cards.set_vec(v["cards"].as_array().into_iter().flatten().map(card_row).collect::<Vec<_>>());
                            m.set_canvas(&v["canvas"]);
                            stats_meta(&v["stat_meta"]);
                            conn_cards(&app, &m);
                        }
                        "form" => {
                            let id = v["id"].as_str().unwrap_or("").to_string();
                            EDITOR.with(|e| {
                                let mut e = e.borrow_mut();
                                let fresh = e.id != id || app.get_page() != 3;
                                if fresh {
                                    // a fresh editor
                                    e.id = id;
                                    e.back_to = app.get_page();
                                }
                                // the core's draft has every value typed so far
                                e.values.clear();
                                if fresh {
                                    app.set_form_notice(SharedString::new());
                                }
                            });
                            let rows = field_rows(v["fields"].as_array().map(Vec::as_slice).unwrap_or(&[]));
                            app.set_form_fields(ModelRc::new(VecModel::from(rows)));
                            app.set_form_title(v["title"].as_str().unwrap_or("").into());
                            app.set_form_errors(ModelRc::default());
                            app.set_page(3);
                        }
                        "notice" => {
                            let (title, text) = (v["title"].as_str().unwrap_or(""), v["text"].as_str().unwrap_or(""));
                            let msg = if text.is_empty() { title.to_string() } else { format!("{title}\n{text}") };
                            app.set_form_notice(msg.as_str().into());
                            app.set_form_notice_ok(v["ok"] == json!(true));
                            // the same message twice in a row stays (each one restarts the timer)
                            NOTICE_SEQ.with(|n| n.set(n.get() + 1));
                            let seq = NOTICE_SEQ.with(|n| n.get());
                            let weak = app.as_weak();
                            slint::Timer::single_shot(std::time::Duration::from_secs(7), move || {
                                if let Some(a) = weak.upgrade() {
                                    if NOTICE_SEQ.with(|n| n.get()) == seq {
                                        a.set_form_notice(SharedString::new());
                                    }
                                }
                            });
                        }
                        "show_text" => {
                            app.set_popup_title(v["title"].as_str().unwrap_or("").into());
                            app.set_popup_text(v["text"].as_str().unwrap_or("").into());
                        }
                        "form_errors" => {
                            let errs: Vec<SharedString> = v["errors"].as_array().into_iter().flatten().filter_map(Value::as_str).map(SharedString::from).collect();
                            app.set_form_errors(ModelRc::new(VecModel::from(errs)));
                        }
                        "form_saved" => {
                            app.set_form_errors(ModelRc::default());
                            app.set_page(EDITOR.with(|e| e.borrow().back_to));
                        }
                        "error" => show_toast(&app, v["message"].as_str().unwrap_or(""), false, 5.0),
                        // a settings change (Python `on_notify` while the window is open: an InfoBar for 2.5 s)
                        "info" => show_toast(&app, v["message"].as_str().unwrap_or(""), true, 2.5),
                        "latency" => show_latency(&app, &m, &v["latency"]),
                        "settings" => show_settings(&app, &v["settings"]),
                        "language" => {
                            set_language(v["language"].as_str().unwrap_or("en"));
                            apply_fonts(&app, v["language"].as_str().unwrap_or("en"));
                            labels(&app);
                        }
                        "undo" => {
                            app.set_can_undo(v["undo"] == json!(true));
                            app.set_can_redo(v["redo"] == json!(true));
                        }
                        "card" => {
                            m.set_card(&v["card"]);
                            conn_cards(&app, &m);
                        }
                        "stats" => {
                            m.set_stats(&v["stats"]);
                            stats_push(&v["stats"], &v["point"]);
                            stats_render(&app, &running_cards(&m));
                        }
                        "conn_sites" => conn_sites(&app, &v["conn"]),
                        "about" => show_about(&app, &v["about"]),
                        "update" => show_update(&app, &v["update"]),
                        "update_dialog" => app.global::<Upd>().set_dialog(true),
                        "update_progress" => {
                            let (done, total) = (v["done"].as_f64().unwrap_or(0.0), v["total"].as_f64().unwrap_or(0.0));
                            let u = app.global::<Upd>();
                            u.set_progress(if total > 0.0 { (done / total) as f32 } else { 0.0 });
                            let mb = |n: f64| format!("{:.1} MB", n / (1024.0 * 1024.0));
                            u.set_progress_text(if total > 0.0 { format!("{} / {}", mb(done), mb(total)) } else { mb(done) }.into());
                        }
                        "update_ready" => {
                            let u = app.global::<Upd>();
                            u.set_downloading(false);
                            let path = v["path"].as_str().unwrap_or("");
                            UPDATE_FILE.with(|f| *f.borrow_mut() = path.to_string());
                            u.set_ask_install(!path.is_empty());
                        }
                        "conn_start" | "conn_result" | "conn_done" => conn_event(&app, &v),
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

    if cfg!(debug_assertions) {
        let send = send.clone();
        let t = slint::Timer::default();
        t.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(200), move || {
            if let Some(req) = EDIT_REQUEST.with(|r| r.borrow_mut().take()) {
                // "<id>" or "<id>,<button>": the button is pressed once the editor is open
                let (id, action) = req.split_once(',').unwrap_or((&req, ""));
                send(json!({"cmd": "edit_open", "id": id}));
                if !action.is_empty() {
                    send(json!({"cmd": "edit_action", "id": id, "action": action, "values": {}}));
                }
            }
        });
        std::mem::forget(t);
    }
    app.show()?;
    app.global::<NodeEditorComputations>().invoke_viewport_resized(app.get_width_(), app.get_height_());
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

// --- statistics page (Python ui/stats.py): the last minute kept here, the graph drawn as SVG paths ---

const STAT_POINTS: usize = 60;
/// line colours for cards and blocks without a colour of their own (Python `stats.LINE_COLORS`)
const LINE_COLORS: [&str; 10] = ["#4f8ef7", "#e8912d", "#57ab5a", "#b76de0", "#e5534b", "#39c5bb", "#d9b92f", "#e26ba5", "#7c6cf2", "#9aa0a6"];

#[derive(Default)]
struct StatState {
    /// card id ("" = all) -> (up, down) per second
    hist: HashMap<String, std::collections::VecDeque<(f64, f64)>>,
    /// outbound block id -> up + down per second
    exits: HashMap<String, std::collections::VecDeque<f64>>,
    cards: Vec<(String, String, String)>,
    exit_meta: Vec<(String, String, String)>,
    /// the core's latest counters (stats_json) and this second's rates
    latest: Value,
    point: Value,
    /// the ids behind the choice box (0 = all)
    choice_ids: Vec<String>,
}

thread_local! {
    static STATS: std::cell::RefCell<StatState> = std::cell::RefCell::new(StatState::default());
}

fn push_hist<T: Default + Clone>(h: &mut std::collections::VecDeque<T>, v: T) {
    if h.is_empty() {
        h.extend(std::iter::repeat_n(T::default(), STAT_POINTS));
    }
    h.push_back(v);
    while h.len() > STAT_POINTS {
        h.pop_front();
    }
}

fn triples(v: &Value) -> Vec<(String, String, String)> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|x| (x[0].as_str().unwrap_or("").to_string(), x[1].as_str().unwrap_or("").to_string(), x[2].as_str().unwrap_or("").to_string()))
        .collect()
}

fn stats_meta(v: &Value) {
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        s.cards = triples(&v["cards"]);
        s.exit_meta = triples(&v["exits"]);
    });
}

/// a newly opened window: the last minute the core kept
fn stats_history(v: &Value) {
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        s.hist = v["cards"].as_object().into_iter().flatten()
            .map(|(k, h)| (k.clone(), h.as_array().into_iter().flatten().map(|p| (p[0].as_f64().unwrap_or(0.0), p[1].as_f64().unwrap_or(0.0))).collect()))
            .collect();
        s.exits = v["exits"].as_object().into_iter().flatten()
            .map(|(k, h)| (k.clone(), h.as_array().into_iter().flatten().map(|x| x.as_f64().unwrap_or(0.0)).collect()))
            .collect();
    });
    stats_meta(&v["meta"]);
}

/// one second from the core (Python `StatsInterface.push` / `push_exits`)
fn stats_push(stats: &Value, point: &Value) {
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        s.latest = stats.clone();
        if point.is_null() {
            return;
        }
        s.point = point.clone();
        let mut seen = std::collections::HashSet::new();
        for (id, r) in point["cards"].as_object().into_iter().flatten() {
            push_hist(s.hist.entry(id.clone()).or_default(), (r[0].as_f64().unwrap_or(0.0), r[1].as_f64().unwrap_or(0.0)));
            seen.insert(id.clone());
        }
        s.hist.retain(|k, _| seen.contains(k));
        let rates: HashMap<String, f64> = point["exits"].as_object().into_iter().flatten().map(|(k, r)| (k.clone(), r.as_f64().unwrap_or(0.0))).collect();
        for (oid, r) in &rates {
            if !s.exits.contains_key(oid) {
                s.exits.insert(oid.clone(), Default::default());
            }
            push_hist(s.exits.get_mut(oid).unwrap(), *r);
        }
        for (oid, h) in s.exits.iter_mut() {
            if !rates.contains_key(oid) {
                push_hist(h, 0.0);
            }
        }
    });
}

/// "1,234" (Python f"{n:,}")
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// SVG path of values across a w x h px graph (y down), and the area under it
fn chart_path(values: &[f64], top: f64, w: f64, h: f64) -> (String, String) {
    let step = w / (values.len().max(2) - 1) as f64;
    let mut d = String::new();
    for (i, v) in values.iter().enumerate() {
        let y = h - h * (v / top).clamp(0.0, 1.0);
        d += &format!("{}{:.1} {:.1} ", if i == 0 { "M " } else { "L " }, i as f64 * step, y);
    }
    let area = format!("{d}L {:.1} {h:.1} L 0 {h:.1} Z", values.len().saturating_sub(1) as f64 * step);
    (d, area)
}

fn hex(c: &str) -> Color {
    hex_color(c).unwrap_or(Color::from_rgb_u8(0x9a, 0xa0, 0xa6))
}

/// the statistics page from what is kept (Python `StatsInterface._show`)
fn stats_render(app: &App, running: &std::collections::HashSet<String>) {
    let st = app.global::<Stats>();
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        // the choice box: all cards, then each card (kept on the same card when the list changes)
        let current = s.choice_ids.get(st.get_choice().max(0) as usize).cloned().unwrap_or_default();
        let mut ids = vec![String::new()];
        let mut names = vec![SharedString::from(tr("Total"))];
        for (id, name, _) in &s.cards {
            ids.push(id.clone());
            names.push(name.as_str().into());
        }
        let choice = ids.iter().position(|i| *i == current).unwrap_or(0);
        if ids != s.choice_ids {
            st.set_choices(ModelRc::new(VecModel::from(names)));
            s.choice_ids = ids;
        }
        if st.get_choice() != choice as i32 {
            st.set_choice(choice as i32);
        }
        let sel = s.choice_ids[choice].clone();
        let c = &s.latest[sel.as_str()];
        let n = |k: &str| c[k].as_f64().unwrap_or(0.0) as u64;
        let r = &s.point["cards"][sel.as_str()];
        let (up, down) = (r[0].as_f64().unwrap_or(0.0), r[1].as_f64().unwrap_or(0.0));
        let rate = |b: f64| format!("{}/s", fmt_size(b));
        let total = |b: u64| tr("total {fmt_size}").replace("{fmt_size}", &fmt_size(b as f64));
        let tile = |t: &str, v: String, sub: String| StatTileData { title: tr(t).into(), value: v.into(), sub: sub.into() };
        let tiles = vec![
            tile("Active TCP connections", n("tcp").to_string(), String::new()),
            tile("Active UDP sessions", n("udp").to_string(), String::new()),
            tile("Total connections", thousands(n("total")), String::new()),
            tile("Download", rate(down), total(n("down"))),
            tile("Upload", rate(up), total(n("up"))),
            tile("Blocked / failed", format!("{} / {}", thousands(n("blocked")), thousands(n("failed"))), String::new()),
        ];
        st.set_tiles(ModelRc::new(VecModel::from(tiles)));
        // the graph
        let mut series: Vec<(String, Color, Vec<f64>, bool)> = vec![];
        let mut empty = String::new();
        match st.get_mode() {
            1 => {
                let mut k = 0;
                for (id, name, color) in &s.cards {
                    let Some(h) = s.hist.get(id) else { continue };
                    if !running.contains(id) && h.iter().all(|(u, d)| u + d == 0.0) {
                        continue;
                    }
                    let col = if color.is_empty() {
                        k += 1;
                        LINE_COLORS[(k - 1) % LINE_COLORS.len()]
                    } else {
                        color.as_str()
                    };
                    series.push((name.clone(), hex(col), h.iter().map(|(u, d)| u + d).collect(), false));
                }
                if series.is_empty() {
                    empty = tr("No proxy is running");
                }
            }
            2 => {
                let mut k = 0;
                for (id, name, color) in &s.exit_meta {
                    let Some(h) = s.exits.get(id) else { continue };
                    if h.iter().all(|v| *v == 0.0) {
                        continue;
                    }
                    let col = if color.is_empty() {
                        k += 1;
                        LINE_COLORS[(k - 1) % LINE_COLORS.len()]
                    } else {
                        color.as_str()
                    };
                    series.push((name.clone(), hex(col), h.iter().copied().collect(), false));
                }
                if series.is_empty() {
                    empty = tr("No traffic in the last minute");
                }
            }
            _ => {
                let h = s.hist.get(&sel).cloned().unwrap_or_default();
                let dark = app.get_dark();
                let (down_c, up_c) = if dark { ("#9ecaff", "#e3008c") } else { ("#0061a4", "#b4009e") };
                series.push((tr("Download"), hex(down_c), h.iter().map(|(_, d)| *d).collect(), true));
                series.push((tr("Upload"), hex(up_c), h.iter().map(|(u, _)| *u).collect(), false));
            }
        }
        let top = series.iter().flat_map(|x| x.2.iter().copied()).fold(1024.0f64, f64::max);
        let lines: Vec<ChartLine> = series
            .iter()
            .map(|(name, color, values, fill)| {
                let (d, area) = chart_path(values, top, st.get_chart_w().max(1.0) as f64, st.get_chart_h().max(1.0) as f64);
                ChartLine { name: name.as_str().into(), color: *color, commands: d.into(), area: if *fill { area.into() } else { SharedString::new() } }
            })
            .collect();
        st.set_lines(ModelRc::new(VecModel::from(lines)));
        st.set_max_text(tr("max {fmt_rate}").replace("{fmt_rate}", &rate(top)).into());
        st.set_empty(empty.into());
    });
}

fn stats_labels(app: &App) {
    let st = app.global::<Stats>();
    st.set_title(tr("Statistics").into());
    st.set_chart_title(tr("Throughput (last 60 s)").into());
    let modes: Vec<SharedString> = ["Combined", "Per proxy", "Per exit"].iter().map(|m| SharedString::from(tr(m))).collect();
    st.set_modes(ModelRc::new(VecModel::from(modes)));
    STATS.with(|s| s.borrow_mut().choice_ids.clear());
    app.set_t_stats(tr("Statistics").into());
    app.set_t_conn(tr("Connection test").into());
}

/// the running cards (their ids), for the graph and the connection test
fn running_cards(m: &Models) -> std::collections::HashSet<String> {
    m.cards.iter().filter(|c| c.state == "running").map(|c| c.id.to_string()).collect()
}

// --- connection test page (Python ui/conntest.py) ---

#[derive(Default)]
struct ConnState {
    rows: Vec<ConnRow>,
    running: bool,
    preparing: bool,
    cooldown: u64,
    has_results: bool,
    /// the running cards in the choice box: (id, name)
    cards: Vec<(String, String)>,
    workers: u64,
    timeout: u64,
    cooldown_secs: u64,
    done: usize,
}

thread_local! {
    static CONN: std::cell::RefCell<ConnState> = std::cell::RefCell::new(ConnState::default());
    static CONN_TIMER: slint::Timer = slint::Timer::default();
}

fn conn_labels(app: &App) {
    let c = app.global::<Conn>();
    c.set_title(tr("Connection test").into());
    c.set_placeholder(tr("Add a site, e.g. example.com").into());
    c.set_add_text(tr("Add").into());
    let cols: Vec<SharedString> = ["Region", "Site", "Address", "Result", "Connect", "All"].iter().map(|x| SharedString::from(tr(x))).collect();
    c.set_columns(ModelRc::new(VecModel::from(cols)));
}

fn conn_rows(app: &App) {
    CONN.with(|s| app.global::<Conn>().set_rows(ModelRc::new(VecModel::from(s.borrow().rows.clone()))));
}

/// the sites (results cleared) from the core's "conn"
fn conn_sites(app: &App, v: &Value) {
    CONN.with(|s| {
        let mut s = s.borrow_mut();
        s.rows = v["sites"].as_array().into_iter().flatten()
            .map(|x| ConnRow {
                region: x[0].as_str().unwrap_or("").into(),
                name: x[1].as_str().unwrap_or("").into(),
                host: x[2].as_str().unwrap_or("").into(),
                added: x[3] == json!(true),
                result: "-".into(),
                conn: SharedString::new(),
                total: SharedString::new(),
                tone: 0,
            })
            .collect();
        s.running = v["running"] == json!(true);
        s.workers = v["workers"].as_u64().unwrap_or(6);
        s.timeout = v["timeout"].as_u64().unwrap_or(8);
        s.cooldown_secs = v["cooldown"].as_u64().unwrap_or(15);
        s.has_results = false;
    });
    app.global::<Conn>().set_site(SharedString::new());
    conn_rows(app);
    conn_sync(app);
}

/// the running cards in the choice box (kept on the same card)
fn conn_cards(app: &App, m: &Models) {
    let c = app.global::<Conn>();
    let running: Vec<(String, String)> = m.cards.iter().filter(|r| r.state == "running").map(|r| (r.id.to_string(), r.name.to_string())).collect();
    CONN.with(|s| {
        let mut s = s.borrow_mut();
        if s.cards == running {
            return;
        }
        let current = s.cards.get(c.get_proxy().max(0) as usize).map(|x| x.0.clone());
        let names: Vec<SharedString> = running.iter().map(|x| SharedString::from(x.1.as_str())).collect();
        c.set_proxies(ModelRc::new(VecModel::from(names)));
        let idx = current.and_then(|id| running.iter().position(|x| x.0 == id)).unwrap_or(0);
        c.set_proxy(idx as i32);
        s.cards = running;
    });
    conn_sync(app);
}

fn conn_selected(app: &App) -> Option<(String, String)> {
    let i = app.global::<Conn>().get_proxy().max(0) as usize;
    CONN.with(|s| s.borrow().cards.get(i).cloned())
}

/// the button and the summary while nothing runs (Python `_sync_button` / `_cool_tick`)
fn conn_sync(app: &App) {
    let c = app.global::<Conn>();
    let sel = conn_selected(app);
    CONN.with(|s| {
        let s = s.borrow();
        c.set_editable(!s.running);
        if s.running {
            c.set_button(tr("Testing…").into());
            c.set_button_enabled(false);
            return;
        }
        if s.preparing {
            c.set_button(tr("Preparing…").into());
            c.set_button_enabled(false);
            return;
        }
        if s.cooldown > 0 {
            c.set_button(tr("Test again ({cooldown}s)").replace("{cooldown}", &s.cooldown.to_string()).into());
            c.set_button_enabled(false);
            return;
        }
        c.set_button(tr(if s.has_results { "Test again" } else { "Start test" }).into());
        c.set_button_enabled(sel.is_some());
        let summary = match &sel {
            Some((_, name)) => tr("Connects to {len} sites over HTTPS through '{name}' ({PROBE_WORKERS} at a time, up to {PROBE_TIMEOUT}s per site).")
                .replace("{name}", name)
                .replace("{len}", &s.rows.len().to_string())
                .replace("{PROBE_WORKERS}", &s.workers.to_string())
                .replace("{PROBE_TIMEOUT}", &s.timeout.to_string()),
            None => tr("No proxy is running. Start one on the Proxies page first."),
        };
        c.set_summary(summary.into());
    });
}

fn conn_event(app: &App, v: &Value) {
    let c = app.global::<Conn>();
    match v["ev"].as_str().unwrap_or("") {
        "conn_start" => {
            let total = v["total"].as_u64().unwrap_or(0);
            CONN.with(|s| {
                let mut s = s.borrow_mut();
                s.running = true;
                s.preparing = false;
                s.done = 0;
                for r in s.rows.iter_mut() {
                    r.result = tr("Waiting…").into();
                    r.conn = SharedString::new();
                    r.total = SharedString::new();
                    r.tone = 0;
                }
            });
            c.set_summary(tr("Testing '{name}'… 0 / {len}").replace("{name}", v["name"].as_str().unwrap_or("")).replace("{len}", &total.to_string()).into());
            conn_rows(app);
            conn_sync(app);
        }
        "conn_result" => {
            let row = v["row"].as_u64().unwrap_or(0) as usize;
            let r = &v["result"];
            let (done, total) = CONN.with(|s| {
                let mut s = s.borrow_mut();
                s.done += 1;
                s.has_results = true;
                if let Some(x) = s.rows.get_mut(row) {
                    x.result = r[0].as_str().unwrap_or("").into();
                    x.conn = r[1].as_str().unwrap_or("").into();
                    x.total = r[2].as_str().unwrap_or("").into();
                    x.tone = if r[3] == json!(true) { 1 } else { 2 };
                }
                (s.done, s.rows.len())
            });
            c.set_summary(tr("Testing… {len} / {len2}").replace("{len}", &done.to_string()).replace("{len2}", &total.to_string()).into());
            conn_rows(app);
        }
        "conn_done" => {
            CONN.with(|s| {
                let mut s = s.borrow_mut();
                s.running = false;
                s.cooldown = s.cooldown_secs;
            });
            c.set_summary(v["summary"].as_str().unwrap_or("").into());
            conn_sync(app);
            // the cooldown: the button counts down, then the page goes back to its idle text
            let weak = app.as_weak();
            CONN_TIMER.with(|t| {
                t.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(1), move || {
                    let Some(a) = weak.upgrade() else { return };
                    let left = CONN.with(|s| {
                        let mut s = s.borrow_mut();
                        s.cooldown = s.cooldown.saturating_sub(1);
                        s.cooldown
                    });
                    if left == 0 {
                        CONN_TIMER.with(|t| t.stop());
                        conn_sync(&a);
                    } else {
                        let c = a.global::<Conn>();
                        c.set_button(tr("Test again ({cooldown}s)").replace("{cooldown}", &left.to_string()).into());
                    }
                });
            });
        }
        _ => {}
    }
}

// --- how to use, about, licenses (Python ui/help.py, about.py, license.py) ---

const REPO_URL: &str = "https://github.com/NenkaLab/proxy-dock";

/// the how-to text of a language (English when it has none)
fn help_markdown(code: &str) -> &'static str {
    match code {
        "ko" => include_str!("../../../../proxydock/locales/help/ko.md"),
        "ja" => include_str!("../../../../proxydock/locales/help/ja.md"),
        "zh_CN" => include_str!("../../../../proxydock/locales/help/zh_CN.md"),
        "zh_TW" => include_str!("../../../../proxydock/locales/help/zh_TW.md"),
        "de" => include_str!("../../../../proxydock/locales/help/de.md"),
        "fr" => include_str!("../../../../proxydock/locales/help/fr.md"),
        "es" => include_str!("../../../../proxydock/locales/help/es.md"),
        _ => include_str!("../../../../proxydock/locales/help/en.md"),
    }
}

fn styled(md: &str) -> slint::StyledText {
    slint::StyledText::from_markdown(md).unwrap_or_else(|_| slint::StyledText::from_plain_text(md))
}

/// The how-to as blocks the window draws: headings and table rows itself, the rest as styled text (Slint's
/// styled text has no headings or tables).
fn help_blocks(md: &str) -> Vec<HelpBlock> {
    let mut out = vec![];
    let mut para: Vec<&str> = vec![];
    let mut table_row = 0;
    let flush = |para: &mut Vec<&str>, out: &mut Vec<HelpBlock>| {
        if !para.is_empty() {
            out.push(HelpBlock { kind: 1, text: styled(&para.join("\n")), ..Default::default() });
            para.clear();
        }
    };
    for line in md.lines() {
        let t = line.trim();
        if let Some(h) = t.strip_prefix('#') {
            flush(&mut para, &mut out);
            out.push(HelpBlock { kind: 0, title: h.trim_start_matches('#').trim().into(), ..Default::default() });
            table_row = 0;
        } else if t.starts_with('|') {
            flush(&mut para, &mut out);
            let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
            if cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| "-: ".contains(ch))) {
                continue; // |---|---|
            }
            let cell = |i: usize| styled(cells.get(i).copied().unwrap_or(""));
            out.push(HelpBlock { kind: 2, a: cell(0), b: cell(1), header: table_row == 0, ..Default::default() });
            table_row += 1;
        } else if t.is_empty() {
            flush(&mut para, &mut out);
            table_row = 0;
        } else {
            para.push(line);
        }
    }
    flush(&mut para, &mut out);
    out
}

/// the licences the window shows: (name, text) (Python `LICENSES`, for the Rust app's own parts)
fn licenses() -> Vec<(String, &'static str)> {
    vec![
        ("ProxyDock · GPL v3".into(), include_str!("../../../../LICENSE")),
        (tr("Rust crates (list)"), include_str!("../../../../licenses/rust-crates.txt")),
        ("sing-box · GPL v3".into(), include_str!("../../../../licenses/sing-box-GPL-3.0.txt")),
        ("Hysteria2 · MIT".into(), include_str!("../../../../licenses/Hysteria-MIT.txt")),
        ("Pretendard · OFL 1.1".into(), include_str!("../../../../licenses/Pretendard-OFL-1.1.txt")),
        ("JetBrains Mono · OFL 1.1".into(), include_str!("../../../../licenses/JetBrainsMono-OFL-1.1.txt")),
        ("Material Symbols · Apache 2.0".into(), include_str!("../../../../licenses/MaterialSymbols-Apache-2.0.txt")),
    ]
}

/// the licences folder: next to the program, else the repository's (a build from the source)
fn license_dir() -> std::path::PathBuf {
    let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("licenses")));
    match beside {
        Some(d) if d.is_dir() => d,
        _ => std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("licenses"),
    }
}

/// Open a folder or a web address with the system (Explorer, the default browser)
fn open_with_system(target: &str) -> std::io::Result<()> {
    std::process::Command::new("explorer").arg(target).spawn().map(|_| ())
}

fn info_labels(app: &App, code: &str) {
    let i = app.global::<Info>();
    let t = |s: &str| SharedString::from(tr(s));
    i.set_help_title(t("How to use"));
    i.set_help(ModelRc::new(VecModel::from(help_blocks(help_markdown(code)))));
    i.set_about_title(t("About"));
    i.set_desc(t("Creates and runs proxies from inbound and outbound blocks."));
    i.set_s_version(t("Version"));
    i.set_s_engines(t("Engines"));
    i.set_s_folder(t("Settings folder"));
    i.set_folder_label(t("Settings and data"));
    i.set_open_folder(t("Open folder"));
    i.set_s_links(t("Links"));
    i.set_repo_label(t("Source code (GitHub)"));
    i.set_repo_url(REPO_URL.into());
    i.set_releases_label(t("Releases"));
    i.set_releases_url(format!("{REPO_URL}/releases").into());
    i.set_open(t("Open"));
    i.set_licenses(t("Licenses"));
    i.set_licenses_desc(t("Licenses of the fonts and open source components included in this program"));
    i.set_lic_desc(t("Fonts and open source components included in ProxyDock (also in the {LICENSE_DIR} folder).").replace("{LICENSE_DIR}", "licenses").into());
    i.set_back(t("Back"));
    let names: Vec<SharedString> = licenses().into_iter().map(|(n, _)| SharedString::from(n)).collect();
    i.set_lic_names(ModelRc::new(VecModel::from(names)));
    app.set_t_help(t("How to use"));
    app.set_t_about(t("About"));
}

/// the About page from the core's "about"
fn show_about(app: &App, v: &Value) {
    let i = app.global::<Info>();
    let version = v["version"].as_str().unwrap_or("");
    i.set_app_version(version.into());
    i.set_version_text(tr("Version {version}").replace("{version}", version).into());
    i.set_app_dir(v["app_dir"].as_str().unwrap_or("").into());
    let rows: Vec<EngineRow> = match v["engines"].as_array() {
        None => vec![EngineRow { name: "sing-box".into(), text: tr("Checking…").into() }, EngineRow { name: "Hysteria2".into(), text: tr("Checking…").into() }],
        Some(list) => list
            .iter()
            .map(|e| {
                let text = match (e[1].as_str(), e[2].as_str()) {
                    (None, _) => tr("Not found"),
                    (Some(path), Some(ver)) => format!("{}\n{path}", tr("Version {version}").replace("{version}", ver)),
                    (Some(path), None) => format!("{}\n{path}", tr("Version unknown")),
                };
                EngineRow { name: e[0].as_str().unwrap_or("").into(), text: text.into() }
            })
            .collect(),
    };
    i.set_engines(ModelRc::new(VecModel::from(rows)));
}

fn show_license(app: &App) {
    let i = app.global::<Info>();
    let list = licenses();
    let k = (i.get_lic_index().max(0) as usize).min(list.len() - 1);
    i.set_lic_text(list[k].1.into());
}

// --- updates (Python ui/updater.py) ---

thread_local! {
    /// the update file downloaded and verified, waiting for "Install"
    static UPDATE_FILE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    /// the release page (when this copy cannot install the update itself)
    static UPDATE_PAGE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// the About page's update line and the release dialog from the core's "update"
fn show_update(app: &App, v: &Value) {
    let u = app.global::<Upd>();
    u.set_status(v["status"].as_str().unwrap_or("").into());
    u.set_busy(v["busy"] == json!(true));
    let rel = &v["release"];
    u.set_has_release(rel.is_object());
    if !rel.is_object() {
        return;
    }
    let version = rel["version"].as_str().unwrap_or("");
    u.set_title(tr("{APP_NAME} {version} is available").replace("{APP_NAME}", "ProxyDock").replace("{version}", version).into());
    u.set_installed(tr("Installed version: {version}").replace("{version}", v["current"].as_str().unwrap_or("")).into());
    let notes = rel["notes"].as_str().filter(|n| !n.is_empty()).map(String::from).unwrap_or_else(|| tr("No release notes."));
    // (Slint's styled text has no headings: they become bold lines)
    let notes: String = notes
        .lines()
        .map(|l| match l.trim_start().strip_prefix('#') {
            Some(h) => format!("**{}**", h.trim_start_matches('#').trim()),
            None => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    u.set_notes(styled(&notes));
    let installs = rel["asset"].is_string();
    let size = rel["size"].as_f64().unwrap_or(0.0);
    let action = if installs {
        tr("Download and install") + &if size > 0.0 { format!(" ({:.1} MB)", size / (1024.0 * 1024.0)) } else { String::new() }
    } else {
        tr("Open the release page")
    };
    u.set_action(action.into());
    u.set_file(rel["asset"].as_str().unwrap_or("").into());
    let page = rel["url"].as_str().filter(|x| !x.is_empty()).unwrap_or(proxydock_core::update::RELEASES_URL);
    UPDATE_PAGE.with(|p| *p.borrow_mut() = (!installs).then(|| page.to_string()));
}
