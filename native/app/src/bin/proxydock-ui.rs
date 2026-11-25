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
    static NOTICE_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
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

fn labels(app: &App) {
    app.set_t_close(tr("Close").into());
    app.set_t_save(tr("Save").into());
    app.set_t_cancel(tr("Cancel").into());
    app.set_t_add_inbound(tr("Inbound").into());
    app.set_t_add_router("Router".into());
    app.set_t_add_relay(tr("Pass-through").into());
    app.set_t_add_outbound(tr("Outbound").into());
    app.set_t_delete(tr("Delete").into());
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
            // selected links are cut first, then selected blocks deleted
            for l in models.links.iter().filter(|l| l.selected) {
                let (a, b) = (l.start_pin_id, l.end_pin_id);
                send(json!({"cmd": "disconnect", "from": key_of(a / 64), "out": (a % 64) / 2, "to": key_of(b / 64), "in": (b % 64) / 2}));
            }
            let keys: Vec<String> = models.nodes.iter().filter(|n| n.selected).map(|n| key_of(n.id)).collect();
            if !keys.is_empty() {
                send(json!({"cmd": "delete", "keys": keys}));
            }
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
                            m.set_canvas(&v["canvas"]);
                            // debug builds: PROXYDOCK_EDIT=<block id> opens its editor (screenshots without clicking)
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
                        "error" => {
                            app.set_toast(v["message"].as_str().unwrap_or("").into());
                            let weak = app.as_weak();
                            slint::Timer::single_shot(std::time::Duration::from_secs(5), move || {
                                if let Some(a) = weak.upgrade() {
                                    a.set_toast(SharedString::new());
                                }
                            });
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

    if cfg!(debug_assertions) {
        let send = send.clone();
        let t = slint::Timer::default();
        t.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(200), move || {
            if let Some(id) = EDIT_REQUEST.with(|r| r.borrow_mut().take()) {
                send(json!({"cmd": "edit_open", "id": id}));
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
