// proxydock-ui.exe: the ProxyDock window. Started by proxydock.exe (the tray + core process) with
// PROXYDOCK_IPC="<port> <token>"; it draws what the core sends and sends the user's commands back. Closing the
// window ends this process; the cards keep running in the core.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use proxydock::ipc;
use proxydock_core::i18n::{set_language, tr};
use proxydock_core::log::fmt_size;
use serde_json::{Value, json};
use slint::{Color, ComponentHandle, Model, ModelRc, SharedString, VecModel};
use slint_node_editor::{
    Dragged, GroupData, GroupLogic, GroupMember, LinkData, LinkPath, MovableNode, NodeEditorSetup, PinGrab,
    wire_node_editor, wire_selection,
};
use std::collections::HashMap;
use std::net::TcpStream;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

slint::include_modules!();

const LOG_CAP: usize = 5000;

impl MovableNode for NodeData {
    fn id(&self) -> i32 {
        self.id
    }
    fn x(&self) -> f32 {
        self.x
    }
    fn y(&self) -> f32 {
        self.y
    }
    fn selected(&self) -> bool {
        self.selected
    }
    fn set_x(&mut self, x: f32) {
        self.x = x;
    }
    fn set_y(&mut self, y: f32) {
        self.y = y;
    }
}

impl GroupMember for NodeData {
    fn group_id(&self) -> i32 {
        self.group_id
    }
    fn set_group_id(&mut self, group_id: i32) {
        self.group_id = group_id;
    }
}

/// a group sheet's room around its blocks, and above them for its title band
const GROUP_PAD: f32 = 20.0;
const GROUP_HEAD: f32 = 36.0;

thread_local! {
    /// canvas groups: the core's id of each group row (row n has id n + 1), its member node ids, collapsed
    static GROUPS: std::cell::RefCell<Vec<(String, Vec<i32>, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
    /// a link picked up at its input end (see grab-input-link): its source pin, input pin, row and colour
    static GRAB: std::cell::Cell<Option<(i32, i32, usize, Color)>> = const { std::cell::Cell::new(None) };
    /// a collapsed group's stand-in block: its pins -> the member pins they stand for
    static STANDIN_PINS: std::cell::RefCell<HashMap<i32, i32>> = std::cell::RefCell::new(HashMap::new());
    /// the last size seen of each block (a collapsed group's members are not measured while hidden)
    static SIZES: std::cell::RefCell<HashMap<i32, (f32, f32)>> = std::cell::RefCell::new(HashMap::new());
}

/// a pin of a stand-in block as the member pin it stands for (other pins as they are)
fn real_pin(p: i32) -> i32 {
    STANDIN_PINS.with(|s| s.borrow().get(&p).copied().unwrap_or(p))
}

fn pin_in(node: i32, lane: i32) -> i32 {
    node * 64 + lane * 2
}

fn pin_out(node: i32, lane: i32) -> i32 {
    node * 64 + lane * 2 + 1
}

fn strings(v: &Value) -> ModelRc<SharedString> {
    let items: Vec<SharedString> =
        v.as_array().into_iter().flatten().map(|x| SharedString::from(x.as_str().unwrap_or(""))).collect();
    ModelRc::new(VecModel::from(items))
}

/// "#rrggbb" (or a wider "oklch(...)", drawn as its nearest sRGB colour) -> colour; links without one take the
/// app's default wire colour
fn hex_color(text: &str) -> Option<Color> {
    if text.starts_with("oklch(") {
        return proxydock_core::colors::display_hex(text).and_then(|h| hex_color(&h));
    }
    let h = text.strip_prefix('#')?;
    let n = u32::from_str_radix(h, 16).ok().filter(|_| h.len() == 6)?;
    Some(Color::from_rgb_u8((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

const WIRE: Color = Color::from_rgb_u8(90, 160, 240);

/// links with a mix of colours: (link index, colour), and the block positions they were drawn for
/// what back (false) / forward (true) runs
type NavCallback = Rc<dyn Fn(bool)>;
type MixedLinks = (Vec<(usize, Color)>, Vec<(i32, i32, i32)>);
/// the log as shown: the lines the filters let through
type LogView = Rc<slint::FilterModel<ModelRc<LogRow>, Box<dyn Fn(&LogRow) -> bool>>>;

/// the open colour picker and what its OK does with the chosen colour
type OpenPicker = (proxydock::colorpick::Picker, Rc<dyn Fn(String)>);

/// the block colours the palette offers (Python `PALETTE`)
const PALETTE: [(&str, &str); 10] = [
    ("#e5534b", "Red"),
    ("#e8912d", "Orange"),
    ("#d9b92f", "Yellow"),
    ("#57ab5a", "Green"),
    ("#39c5bb", "Teal"),
    ("#4f8ef7", "Blue"),
    ("#7c6cf2", "Indigo"),
    ("#b76de0", "Purple"),
    ("#e26ba5", "Pink"),
    ("#9aa0a6", "Gray"),
];

thread_local! {
    /// what the open palette colours: the block key, the router output ("" = the block), its colour now
    static PALETTE_FOR: std::cell::RefCell<(String, String, String)> = const { std::cell::RefCell::new((String::new(), String::new(), String::new())) };
    /// the theme colour in use ("" / "system" / a colour), and what redoes it when Windows' accent changes
    static THEME_COLOR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static ON_ACCENT: std::cell::RefCell<Option<Rc<dyn Fn()>>> = const { std::cell::RefCell::new(None) };
    /// each block's stored own colour by (node id, router output key or ""): what the picker starts from
    static RAW_COLORS: std::cell::RefCell<HashMap<(i32, String), String>> = std::cell::RefCell::new(HashMap::new());
    /// the open colour picker and what its OK does
    static COLOR_PICKER: std::cell::RefCell<Option<OpenPicker>> = const { std::cell::RefCell::new(None) };
    /// links carrying a mix of colours: their index and colour (drawn dotted), and the geometry they were drawn for
    static MIXED: std::cell::RefCell<MixedLinks> = const { std::cell::RefCell::new((Vec::new(), Vec::new())) };
}

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

/// a card's state line on its block (Python: "● Running", "○ error - the message", at most 60 characters)
fn node_state_text(state: &str, error: &str) -> SharedString {
    let text = if error.is_empty() { state_text(state) } else { format!("{} - {error}", state_text(state)) };
    let text = if text.chars().count() > 60 { text.chars().take(60).collect::<String>() + "…" } else { text };
    text.into()
}

/// a block's info lines from the core: [text, tone] pairs
fn info_lines(v: &Value) -> ModelRc<InfoLine> {
    let lines: Vec<InfoLine> = v
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| InfoLine { text: l[0].as_str().unwrap_or("").into(), tone: l[1].as_i64().unwrap_or(0) as i32 })
        .collect();
    ModelRc::new(VecModel::from(lines))
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
        menu: ModelRc::default(),
        menu_sub_title: SharedString::new(),
        menu_sub: ModelRc::default(),
        menu_cut: false,
        menu_tail: false,
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
    // a card's connection line can show its way on the canvas
    let routable = !s("id").is_empty() && proxydock::controller::conn_of_line(s("message")).is_some();
    LogRow {
        text: format!("[{}] {:<5} {name}{}", s("time"), level, s("message")).into(),
        tone,
        card: s("id").into(),
        message: s("message").into(),
        routable,
        level: level.into(),
    }
}

/// menu entries from the core ([{action, title}])
fn leaves(v: &Value) -> ModelRc<MenuLeaf> {
    let items: Vec<MenuLeaf> = v
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| MenuLeaf {
            action: e["action"].as_str().unwrap_or("").into(),
            title: e["title"].as_str().unwrap_or("").into(),
        })
        .collect();
    ModelRc::new(VecModel::from(items))
}

/// [[type, name]] -> menu entries (the add menus)
fn type_leaves(v: &Value) -> ModelRc<MenuLeaf> {
    let items: Vec<MenuLeaf> = v
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| MenuLeaf { action: e[0].as_str().unwrap_or("").into(), title: e[1].as_str().unwrap_or("").into() })
        .collect();
    ModelRc::new(VecModel::from(items))
}

/// The view that shows every block (Python `fit_view`), from the blocks' rectangles the editor knows
fn fit_canvas(app: &App, rects: &[(f32, f32, f32, f32)]) {
    if rects.is_empty() {
        return;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y, w, h) in rects {
        x0 = x0.min(*x);
        y0 = y0.min(*y);
        x1 = x1.max(x + w);
        y1 = y1.max(y + h);
    }
    let (vw, vh) = (app.get_width_(), app.get_height_());
    // room around, and above the floating toolbar
    let (pad, bottom) = (48.0, 110.0);
    let zoom = ((vw - pad * 2.0) / (x1 - x0).max(1.0)).min((vh - pad - bottom) / (y1 - y0).max(1.0)).clamp(0.25, 1.25);
    app.set_canvas_zoom(zoom);
    app.set_canvas_pan_x(vw / 2.0 - (x0 + x1) / 2.0 * zoom);
    app.set_canvas_pan_y((vh - bottom + pad) / 2.0 - (y0 + y1) / 2.0 * zoom);
}

fn traffic_text(st: &Value) -> String {
    let n = |k: &str| st[k].as_u64().unwrap_or(0);
    if n("total") == 0 && n("up") == 0 && n("down") == 0 {
        return String::new();
    }
    format!("↑ {}  ↓ {}  ·  TCP {}  UDP {}", fmt_size(n("up") as f64), fmt_size(n("down") as f64), n("tcp"), n("udp"))
}

thread_local! {
    /// the timer that waits for the native window (then colours and takes over its title bar)
    static FIRST_PAINT: std::cell::RefCell<Option<slint::Timer>> = const { std::cell::RefCell::new(None) };
}

trait StopTimer {
    fn stop(&self);
}

impl StopTimer for std::cell::RefCell<Option<slint::Timer>> {
    fn stop(&self) {
        if let Some(t) = self.borrow().as_ref() {
            t.stop();
        }
    }
}

thread_local! {
    /// arrow-key navigation: the choices of the last step and where each block was reached from
    static NAV: std::cell::RefCell<(Vec<i32>, HashMap<i32, i32>)> = std::cell::RefCell::new((Vec::new(), HashMap::new()));
}

/// Only block `id` selected, brought into view when it is not (Python `_select`)
fn select_block(app: &App, models: &Models, id: i32, rect: Option<(f32, f32, f32, f32)>) {
    for i in 0..models.nodes.row_count() {
        let Some(mut n) = models.nodes.row_data(i) else { continue };
        if n.selected != (n.id == id) {
            n.selected = n.id == id;
            models.nodes.set_row_data(i, n);
        }
    }
    let Some((x, y, w, h)) = rect else { return };
    let (z, px, py) = (app.get_canvas_zoom(), app.get_canvas_pan_x(), app.get_canvas_pan_y());
    let (vw, vh) = (app.get_width_(), app.get_height_());
    let (sx, sy) = (px + x * z, py + y * z);
    if sx < 0.0 || sy < 0.0 || sx + w * z > vw || sy + h * z > vh - 100.0 {
        app.set_canvas_pan_x(vw / 2.0 - (x + w / 2.0) * z);
        app.set_canvas_pan_y((vh - 100.0) / 2.0 - (y + h / 2.0) * z);
    }
}

/// Python `nav_key`: → the block the selected one links to, ← one linking into it (back where the last step
/// came from first); ↑ / ↓ the other choices of that step, else the nearest block above / below.
fn canvas_nav(app: &App, models: &Models, rects: &HashMap<i32, (f32, f32, f32, f32)>, dir: &str) {
    let sel: Vec<i32> = models.nodes.iter().filter(|n| n.selected).map(|n| n.id).collect();
    let [cur] = sel[..] else { return };
    let keys = models.keys.borrow().clone();
    let key = |id: i32| keys.get((id - 1).max(0) as usize).cloned().unwrap_or_default();
    let id_of = |k: &str| keys.iter().position(|x| x == k).map(|i| i as i32 + 1);
    let y_of = |id: i32| rects.get(&id).map_or(0.0, |r| r.1);
    if dir == "left" || dir == "right" {
        let forward = dir == "right";
        let mut options: Vec<i32> = vec![];
        // the other half of a wireless pair counts as linked
        let k = key(cur);
        let partner = if forward && !k.ends_with("/out") {
            id_of(&format!("{k}/out"))
        } else if !forward {
            k.strip_suffix("/out").and_then(id_of)
        } else {
            None
        };
        options.extend(partner);
        // links out of (into) it, by port, then top to bottom
        let mut linked: Vec<(i32, f32, i32)> = vec![];
        for (i, (_, a, b)) in models.link_meta.borrow().iter().enumerate() {
            let Some(l) = models.links.row_data(i) else { continue };
            if forward && *a == cur {
                linked.push((l.start_pin_id, y_of(*b), *b));
            } else if !forward && *b == cur {
                linked.push((l.end_pin_id, y_of(*a), *a));
            }
        }
        linked.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)));
        for (_, _, n) in linked {
            if n != cur && !options.contains(&n) {
                options.push(n);
            }
        }
        let back = NAV.with(|nav| nav.borrow().1.get(&cur).copied());
        options.sort_by_key(|n| Some(*n) != back);
        if let Some(first) = options.first().copied() {
            NAV.with(|nav| {
                let mut nav = nav.borrow_mut();
                nav.0 = options.clone();
                nav.1.insert(first, cur);
            });
            select_block(app, models, first, rects.get(&first).copied());
        }
        return;
    }
    let up = dir == "up";
    let choices = NAV.with(|nav| nav.borrow().0.clone());
    if choices.len() > 1 && choices.contains(&cur) {
        let i = choices.iter().position(|n| *n == cur).unwrap() as i32;
        let next = choices[((i + if up { -1 } else { 1 }).rem_euclid(choices.len() as i32)) as usize];
        NAV.with(|nav| {
            let mut nav = nav.borrow_mut();
            if let Some(prev) = nav.1.get(&cur).copied() {
                nav.1.insert(next, prev);
            }
        });
        select_block(app, models, next, rects.get(&next).copied());
        return;
    }
    // the nearest block above / below; one in the same column wins over one beside
    let Some(here) = rects.get(&cur).copied() else { return };
    let mut best: Option<(f32, i32)> = None;
    for (id, r) in rects {
        if *id == cur {
            continue;
        }
        let dy = if up { here.1 - (r.1 + r.3) } else { r.1 - (here.1 + here.3) };
        if dy < -here.3 / 2.0 {
            continue;
        }
        let overlap = (here.0 + here.2).min(r.0 + r.2) - here.0.max(r.0);
        let dx = if overlap > 0.0 { 0.0 } else { ((r.0 + r.2 / 2.0) - (here.0 + here.2 / 2.0)).abs() };
        let score = dy.max(0.0) + 3.0 * dx;
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, *id));
        }
    }
    if let Some((_, id)) = best {
        NAV.with(|nav| nav.borrow_mut().0.clear());
        select_block(app, models, id, rects.get(&id).copied());
    }
}

thread_local! {
    /// the selection the wireless-pair focus was last worked out for
    static PAIR_SEL: std::cell::RefCell<Vec<i32>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A selected half of a wireless pair (Python): the pair stays bright, the other blocks dim, and a dotted arc
/// joins the halves. Checked a few times a second while the canvas shows (the selection has many writers).
fn pair_focus(app: &App, models: &Models, rects: &HashMap<i32, (f32, f32, f32, f32)>) {
    let sel: Vec<i32> = models.nodes.iter().filter(|n| n.selected).map(|n| n.id).collect();
    let same = PAIR_SEL.with(|p| *p.borrow() == sel);
    if same {
        return;
    }
    PAIR_SEL.with(|p| *p.borrow_mut() = sel.clone());
    let keys = models.keys.borrow().clone();
    let id_of = |k: &str| keys.iter().position(|x| x == k).map(|i| i as i32 + 1);
    let pair = match sel[..] {
        [one] => {
            let k = keys.get((one - 1).max(0) as usize).cloned().unwrap_or_default();
            match k.strip_suffix("/out") {
                Some(base) => id_of(base).map(|a| (a, one)),
                None => id_of(&format!("{k}/out")).map(|b| (one, b)),
            }
        }
        _ => None,
    };
    let Some((a, b)) = pair else {
        app.set_focus_a(0);
        app.set_focus_b(0);
        app.set_pair_dots(ModelRc::default());
        return;
    };
    app.set_focus_a(a);
    app.set_focus_b(b);
    // from the input half's right edge to the output half's left edge, sagging a little
    let (Some(ra), Some(rb)) = (rects.get(&a), rects.get(&b)) else { return };
    let (x0, y0) = (ra.0 + ra.2, ra.1 + ra.3 / 2.0);
    let (x1, y1) = (rb.0, rb.1 + rb.3 / 2.0);
    let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0 + ((x1 - x0).abs() + (y1 - y0).abs()) * 0.15 + 30.0);
    let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt().max(1.0);
    let steps = (len / 16.0).ceil().clamp(4.0, 400.0) as usize;
    let dark = app.get_dark();
    let color = if dark { Color::from_rgb_u8(0xa8, 0xc7, 0xfa) } else { Color::from_rgb_u8(0x0b, 0x57, 0xd0) };
    let dots: Vec<FlowDot> = (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let u = 1.0 - t;
            FlowDot {
                x: u * u * x0 + 2.0 * u * t * mx + t * t * x1,
                y: u * u * y0 + 2.0 * u * t * my + t * t * y1,
                size: 5.0,
                color,
            }
        })
        .collect();
    app.set_pair_dots(ModelRc::new(VecModel::from(dots)));
}

/// a group's sheet around its members (their positions and last measured sizes)
fn group_bounds(nodes: &[NodeData], members: &[i32]) -> (f32, f32, f32, f32) {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    SIZES.with(|s| {
        let s = s.borrow();
        for n in nodes.iter().filter(|n| members.contains(&n.id)) {
            let (w, h) = s.get(&n.id).copied().unwrap_or((260.0, 120.0));
            x0 = x0.min(n.x);
            y0 = y0.min(n.y);
            x1 = x1.max(n.x + w);
            y1 = y1.max(n.y + h);
        }
    });
    if x0 > x1 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    (x0 - GROUP_PAD, y0 - GROUP_PAD - GROUP_HEAD, x1 - x0 + GROUP_PAD * 2.0, y1 - y0 + GROUP_PAD * 2.0 + GROUP_HEAD)
}

/// The sheets follow their blocks (sizes measured, blocks moved); the toolbar learns the selection.
fn groups_follow(app: &App, models: &Models, rects: &HashMap<i32, (f32, f32, f32, f32)>) {
    SIZES.with(|s| {
        let mut s = s.borrow_mut();
        for (id, r) in rects {
            s.insert(*id, (r.2, r.3));
        }
    });
    let nodes: Vec<NodeData> = models.nodes.iter().collect();
    let groups = GROUPS.with(|g| g.borrow().clone());
    for (i, (_, members, _)) in groups.iter().enumerate() {
        let Some(mut row) = models.groups.row_data(i) else { continue };
        let (x, y, w, h) = group_bounds(&nodes, members);
        if (row.x - x).abs() > 0.5
            || (row.y - y).abs() > 0.5
            || (row.width - w).abs() > 0.5
            || (row.height - h).abs() > 0.5
        {
            row.x = x;
            row.y = y;
            row.width = w;
            row.height = h;
            models.groups.set_row_data(i, row);
        }
    }
    let sel_count = nodes.iter().filter(|n| n.selected && n.kind != 5).count() as i32;
    if app.get_sel_count() != sel_count {
        app.set_sel_count(sel_count);
    }
    let sel = models.groups.iter().find(|g| g.selected);
    let (gid, collapsed) = sel.map_or((0, false), |g| (g.id, g.collapsed));
    if app.get_group_sel() != gid || app.get_group_sel_collapsed() != collapsed {
        app.set_group_sel(gid);
        app.set_group_sel_collapsed(collapsed);
    }
}

thread_local! {
    /// the group a dragged block is over (row id), for the preview, and that box as it was
    static DROP_TARGET: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    static DROP_BASE: std::cell::Cell<(f32, f32, f32, f32)> = const { std::cell::Cell::new((0.0, 0.0, 0.0, 0.0)) };
}

/// the expanded group row a block's rectangle (world) would drop into: its centre inside the box, not its own
/// group, not collapsed
fn drop_group(groups: &VecModel<GroupData>, own: i32, rect: (f32, f32, f32, f32)) -> Option<GroupData> {
    let (cx, cy) = (rect.0 + rect.2 / 2.0, rect.1 + rect.3 / 2.0);
    groups
        .iter()
        .find(|g| g.id != own && !g.collapsed && cx >= g.x && cx <= g.x + g.width && cy >= g.y && cy <= g.y + g.height)
}

/// While a block is dragged: over another group's box, an outline grows from the box to take the block in; it
/// goes back to the box (and fades) when the block leaves or the drag ends.
fn drop_preview(app: &App, models: &Models) {
    let ds = app.global::<DragState>();
    let node = ds.get_dragged_node_id();
    let dragging = ds.get_is_dragging() && node != 0;
    let hit = dragging.then(|| models.nodes.iter().find(|n| n.id == node && n.kind != 5)).flatten().and_then(|n| {
        let (w, h) = SIZES.with(|s| s.borrow().get(&n.id).copied().unwrap_or((260.0, 120.0)));
        let rect = (n.x + ds.get_drag_offset_x(), n.y + ds.get_drag_offset_y(), w, h);
        drop_group(&models.groups, n.group_id, rect).map(|g| (g, rect))
    });
    match hit {
        Some((g, r)) => {
            if DROP_TARGET.with(|t| t.get()) != g.id {
                // start from the box as it is (no animation for this jump), then grow
                app.set_drop_show(false);
                app.set_drop_x(g.x);
                app.set_drop_y(g.y);
                app.set_drop_w(g.width);
                app.set_drop_h(g.height);
                DROP_TARGET.with(|t| t.set(g.id));
                DROP_BASE.with(|b| b.set((g.x, g.y, g.width, g.height)));
            }
            let x0 = g.x.min(r.0 - GROUP_PAD);
            let y0 = g.y.min(r.1 - GROUP_PAD);
            let x1 = (g.x + g.width).max(r.0 + r.2 + GROUP_PAD);
            let y1 = (g.y + g.height).max(r.1 + r.3 + GROUP_PAD);
            app.set_drop_show(true);
            app.set_drop_x(x0);
            app.set_drop_y(y0);
            app.set_drop_w(x1 - x0);
            app.set_drop_h(y1 - y0);
        }
        None => {
            if DROP_TARGET.with(|t| t.replace(0)) != 0 {
                // back to the box's own size, fading out
                let (x, y, w, h) = DROP_BASE.with(|b| b.get());
                app.set_drop_x(x);
                app.set_drop_y(y);
                app.set_drop_w(w);
                app.set_drop_h(h);
                app.set_drop_show(false);
            }
        }
    }
}

/// the core's id of group row `gid`
fn group_key(gid: i32) -> String {
    GROUPS.with(|g| g.borrow().get((gid - 1).max(0) as usize).map(|x| x.0.clone()).unwrap_or_default())
}

/// The links that carry a mix of colours as dotted lines (Python: dashed), drawn again when blocks move.
fn mix_dots(app: &App, models: &Models, rects: &HashMap<i32, (f32, f32, f32, f32)>) {
    let mut geo: Vec<(i32, i32, i32)> = rects.iter().map(|(id, r)| (*id, r.0 as i32, r.1 as i32)).collect();
    geo.sort_unstable();
    let todo = MIXED.with(|m| {
        let m = m.borrow();
        (m.1 != geo).then(|| m.0.clone())
    });
    let Some(mixed) = todo else { return };
    MIXED.with(|m| m.borrow_mut().1 = geo);
    let mut dots = vec![];
    CURVE.with(|c| {
        let c = c.borrow();
        let Some(curve_of) = c.as_ref() else { return };
        for (i, color) in &mixed {
            let Some(l) = models.links.row_data(*i) else { continue };
            let Some(curve) = curve_of(l.start_pin_id, l.end_pin_id) else { continue };
            // a dot every ~10 world px along the curve
            let pts: Vec<(f32, f32)> = (0..=48).map(|k| curve.eval(k as f32 / 48.0)).collect();
            let mut carry = 0.0f32;
            for w in pts.windows(2) {
                let ((x0, y0), (x1, y1)) = (w[0], w[1]);
                let seg = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
                let mut d = carry;
                while d < seg {
                    let t = d / seg.max(0.001);
                    dots.push(FlowDot { x: x0 + (x1 - x0) * t, y: y0 + (y1 - y0) * t, size: 4.0, color: *color });
                    d += 10.0;
                }
                carry = d - seg;
            }
        }
    });
    app.set_mix_dots(ModelRc::new(VecModel::from(dots)));
}

// --- the back / forward history (Python `go_back / go_forward`, Alt+← / →, the mouse's back / forward buttons) ---

#[derive(Default)]
struct History {
    back: Vec<i32>,
    forward: Vec<i32>,
    current: i32,
    jumping: bool,
}

thread_local! {
    static HISTORY: std::cell::RefCell<History> = std::cell::RefCell::new(History::default());
    /// what the window's message hook calls for back (false) / forward (true)
    static NAV_CB: std::cell::RefCell<Option<NavCallback>> = const { std::cell::RefCell::new(None) };
}

/// the editor's page: leaving it returns to where it was opened from
const EDITOR_PAGE: i32 = 3;

fn page_changed(new: i32) {
    HISTORY.with(|h| {
        let mut h = h.borrow_mut();
        let old = std::mem::replace(&mut h.current, new);
        if h.jumping || old == new {
            return;
        }
        if old == EDITOR_PAGE {
            if h.back.last() == Some(&new) {
                h.back.pop();
            }
        } else {
            h.back.push(old);
            let n = h.back.len();
            if n > 30 {
                h.back.drain(..n - 30);
            }
        }
        h.forward.clear();
    });
}

fn jump(app: &App, page: i32) {
    HISTORY.with(|h| h.borrow_mut().jumping = true);
    app.set_page(page);
    HISTORY.with(|h| {
        let mut h = h.borrow_mut();
        h.jumping = false;
        h.current = page;
    });
}

fn go_back(app: &App) {
    // back in the editor = cancel
    if app.get_page() == EDITOR_PAGE {
        app.invoke_form_cancel();
        return;
    }
    let Some(page) = HISTORY.with(|h| h.borrow_mut().back.pop()) else { return };
    HISTORY.with(|h| h.borrow_mut().forward.push(app.get_page()));
    jump(app, page);
}

fn go_forward(app: &App) {
    let Some(page) = HISTORY.with(|h| h.borrow_mut().forward.pop()) else { return };
    let cur = app.get_page();
    if cur != EDITOR_PAGE {
        HISTORY.with(|h| h.borrow_mut().back.push(cur));
    }
    jump(app, page);
}

// --- the window's size and place (Python `restore_state / save_state`, window.json) ---

fn window_state_path() -> std::path::PathBuf {
    proxydock_core::paths::app_dir().join("window.json")
}

#[cfg(windows)]
mod placement {
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromRect};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowPlacement, SW_SHOWMAXIMIZED, SW_SHOWNORMAL, SetWindowPlacement, WINDOWPLACEMENT,
    };

    /// (left, top, right, bottom of the normal place, maximized)
    pub fn get(hwnd: usize) -> Option<(i32, i32, i32, i32, bool)> {
        let mut p: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
        p.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        if unsafe { GetWindowPlacement(hwnd as HWND, &mut p) } == 0 {
            return None;
        }
        let r = p.rcNormalPosition;
        Some((r.left, r.top, r.right, r.bottom, p.showCmd == SW_SHOWMAXIMIZED as u32))
    }

    /// back where it was (the place is kept only when it is still on a screen)
    pub fn set(hwnd: usize, l: i32, t: i32, r: i32, b: i32, maximized: bool) {
        if r - l < 200 || b - t < 150 {
            return;
        }
        let mut p: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
        p.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        if unsafe { GetWindowPlacement(hwnd as HWND, &mut p) } == 0 {
            return;
        }
        let rc = RECT { left: l, top: t, right: r, bottom: b };
        if unsafe { MonitorFromRect(&rc, MONITOR_DEFAULTTONULL) }.is_null() {
            // off every screen now: the size only, where Windows put it
            let cur = p.rcNormalPosition;
            p.rcNormalPosition =
                RECT { left: cur.left, top: cur.top, right: cur.left + (r - l), bottom: cur.top + (b - t) };
        } else {
            p.rcNormalPosition = rc;
        }
        p.showCmd = if maximized { SW_SHOWMAXIMIZED as u32 } else { SW_SHOWNORMAL as u32 };
        unsafe { SetWindowPlacement(hwnd as HWND, &p) };
    }
}

fn save_window_state(app: &App) {
    #[cfg(windows)]
    {
        let Some((l, t, r, b, max)) = placement::get(owner_hwnd(app)) else { return };
        write_window_state(json!({"left": l, "top": t, "right": r, "bottom": b, "maximized": max}));
    }
    // macOS / Linux: the window's own place and size (physical pixels; a maximised window keeps its last normal
    // size, as Windows' placement does)
    #[cfg(not(windows))]
    {
        let w = app.window();
        let (p, s) = (w.position(), w.size());
        let max = w.is_maximized();
        let old = read_window_state();
        let normal = if max && old["right"].is_i64() {
            (old["left"].clone(), old["top"].clone(), old["right"].clone(), old["bottom"].clone())
        } else {
            (json!(p.x), json!(p.y), json!(p.x + s.width as i32), json!(p.y + s.height as i32))
        };
        write_window_state(
            json!({"left": normal.0, "top": normal.1, "right": normal.2, "bottom": normal.3, "maximized": max}),
        );
    }
}

/// window.json's "rust" entry (the Python app's "geometry" stays as it was)
fn read_window_state() -> Value {
    std::fs::read_to_string(window_state_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .map(|v| v["rust"].clone())
        .unwrap_or(Value::Null)
}

fn write_window_state(rust: Value) {
    let path = window_state_path();
    let mut data: serde_json::Map<String, Value> =
        std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    data.insert("rust".into(), rust);
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, Value::Object(data).to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn restore_window_state(app: &App) {
    #[cfg(windows)]
    {
        let w = &read_window_state();
        let n = |k: &str| w[k].as_i64().unwrap_or(0) as i32;
        if w.is_object() {
            placement::set(
                owner_hwnd(app),
                n("left"),
                n("top"),
                n("right"),
                n("bottom"),
                w["maximized"] == json!(true),
            );
        }
    }
    #[cfg(not(windows))]
    {
        let w = read_window_state();
        let n = |k: &str| w[k].as_i64().unwrap_or(0) as i32;
        let (width, height) = (n("right") - n("left"), n("bottom") - n("top"));
        if w.is_object() && width >= 400 && height >= 300 {
            let win = app.window();
            win.set_size(slint::PhysicalSize::new(width as u32, height as u32));
            win.set_position(slint::PhysicalPosition::new(n("left"), n("top")));
            if w["maximized"] == json!(true) {
                win.set_maximized(true);
            }
        }
    }
}

/// the mouse's back / forward buttons and Alt+← / → anywhere in the window (Python `_MouseNavFilter` and the
/// shortcuts): a hook on the window's messages
#[cfg(windows)]
mod nav_hook {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};

    unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
        const WM_XBUTTONUP: u32 = 0x020C;
        const WM_SYSKEYDOWN: u32 = 0x0104;
        let go = |forward: bool| {
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(cb) = super::NAV_CB.with(|c| c.borrow().clone()) {
                    cb(forward);
                }
            });
        };
        match msg {
            WM_XBUTTONUP => {
                let button = (wp >> 16) & 0xffff;
                if button == 1 || button == 2 {
                    go(button == 2);
                    return 1;
                }
            }
            // Windows' accent colour changed
            0x0320 => {
                let _ = slint::invoke_from_event_loop(|| {
                    if let Some(cb) = super::ON_ACCENT.with(|c| c.borrow().clone()) {
                        cb();
                    }
                });
            }
            // Alt+← / →: (with Alt, keys come as system keys)
            WM_SYSKEYDOWN if wp == 0x25 || wp == 0x27 => {
                go(wp == 0x27);
                return 0;
            }
            _ => {}
        }
        unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
    }

    pub fn install(hwnd: usize) {
        if hwnd != 0 {
            unsafe { SetWindowSubclass(hwnd as HWND, Some(proc_), 7, 0) };
        }
    }
}

/// the window's HWND as a number (0 = none yet), the owner of the file dialogs
fn owner_hwnd(w: &App) -> usize {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let slint_handle = w.window().window_handle();
        if let Ok(handle) = slint_handle.window_handle()
            && let RawWindowHandle::Win32(h) = handle.as_raw()
        {
            return h.hwnd.get() as usize;
        }
    }
    let _ = w;
    0
}

/// an editor field's text now: as typed, else as the core sent it
fn field_text(app: &App, key: &str) -> String {
    if let Some(v) = EDITOR.with(|e| e.borrow().values.get(key).cloned()) {
        return v.as_str().map(String::from).unwrap_or_else(|| v.to_string());
    }
    app.get_form_fields().iter().find(|r| r.key == key).map(|r| r.text.to_string()).unwrap_or_default()
}

/// put a value into an editor field (a chosen file, programs) and ask the core for the form again
fn preview_with(send: &dyn Fn(Value), key: &str, value: &str) {
    let msg = EDITOR.with(|e| {
        let mut e = e.borrow_mut();
        e.values.insert(key.to_string(), json!(value));
        json!({"cmd": "edit_preview", "id": e.id, "values": e.values})
    });
    send(msg);
}

/// the file types a "Browse" button offers, by its field
fn browse_filter(key: &str) -> String {
    if key.ends_with("tls_key") {
        tr("Private keys (*.key *.pem);;All files (*)")
    } else {
        tr("Certificates (*.crt *.pem *.cer);;All files (*)")
    }
}

#[derive(Default)]
struct Picker {
    /// the field the programs go into, the running programs, the chosen ones
    key: String,
    all: Vec<String>,
    chosen: Vec<String>,
}

thread_local! {
    static PICKER: std::cell::RefCell<Picker> = std::cell::RefCell::new(Picker::default());
}

fn split_list(text: &str) -> Vec<String> {
    text.replace(';', ",").split(',').map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect()
}

/// the program picker for field `key` (Python `ProgramPicker`): the programs running now, the ones already in
/// the field checked
fn open_picker(app: &App, key: &str) {
    let have: Vec<String> = split_list(&field_text(app, key)).iter().map(|x| x.to_lowercase()).collect();
    let all = proxydock_core::netinfo::running_programs();
    let chosen = all.iter().filter(|n| have.contains(&n.to_lowercase())).cloned().collect();
    PICKER.with(|p| *p.borrow_mut() = Picker { key: key.to_string(), all, chosen });
    app.set_picker_search(SharedString::new());
    picker_rows(app);
    app.set_picker_open(true);
}

fn picker_rows(app: &App) {
    let words = app.get_picker_search().trim().to_lowercase();
    let rows: Vec<PickItem> = PICKER.with(|p| {
        let p = p.borrow();
        p.all
            .iter()
            .filter(|n| words.is_empty() || n.to_lowercase().contains(&words))
            .map(|n| PickItem { name: n.into(), checked: p.chosen.contains(n) })
            .collect()
    });
    app.set_picker_items(ModelRc::new(VecModel::from(rows)));
}

/// the field with the chosen programs added (Python `add_programs`: the ones there stay, no doubles)
fn picker_apply(app: &App, send: &dyn Fn(Value), extra: &[String]) {
    let (key, chosen) = PICKER.with(|p| {
        let p = p.borrow();
        (p.key.clone(), p.chosen.clone())
    });
    let mut cur = split_list(&field_text(app, &key));
    let mut low: Vec<String> = cur.iter().map(|x| x.to_lowercase()).collect();
    for x in chosen.iter().chain(extra) {
        if !low.contains(&x.to_lowercase()) {
            low.push(x.to_lowercase());
            cur.push(x.clone());
        }
    }
    preview_with(send, &key, &cur.join(", "));
}

/// the native window exists (winit makes it after the event loop starts)
fn has_native_window(w: &App) -> bool {
    use raw_window_handle::HasWindowHandle;
    w.window().window_handle().window_handle().is_ok()
}

/// Windows 11: caption, border and caption text in the app's colours (ignored before Windows 11)
fn apply_title_bar(w: &App) {
    #[cfg(not(windows))]
    let _ = w;
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
    /// bytes per second on each link (same order as `links`), for the moving dots
    rates: std::cell::RefCell<Vec<f64>>,
    /// each block's right-click menu by its key (the list page's ⋯ buttons)
    menus: std::cell::RefCell<HashMap<String, Value>>,
    /// the list page's outbound blocks and routers
    out_rows: Rc<VecModel<BlockRow>>,
    router_rows: Rc<VecModel<BlockRow>>,
    /// the canvas groups' sheets
    groups: Rc<VecModel<GroupData>>,
}

impl Models {
    /// the canvas from the core's description (blocks keyed by block id, links by lanes)
    fn set_canvas(&self, c: &Value) {
        let mut ids: HashMap<String, i32> = HashMap::new();
        let mut nodes: Vec<NodeData> = vec![];
        RAW_COLORS.with(|r| r.borrow_mut().clear());
        for (i, n) in c["nodes"].as_array().into_iter().flatten().enumerate() {
            let id = i as i32 + 1;
            RAW_COLORS.with(|r| {
                let mut r = r.borrow_mut();
                r.insert((id, String::new()), n["color"].as_str().unwrap_or("").to_string());
                for c in n["out_colors"].as_array().into_iter().flatten() {
                    r.insert((id, c[0].as_str().unwrap_or("").to_string()), c[1].as_str().unwrap_or("").to_string());
                }
            });
            ids.insert(n["key"].as_str().unwrap_or("").to_string(), id);
            let card = n["card"].as_str().unwrap_or("");
            let state = n["state"].as_str().unwrap_or("");
            let error = self.cards.iter().find(|r| r.id == card).map(|r| r.error.to_string()).unwrap_or_default();
            // the PC block: its status as its line
            let lines = if n["lines"].as_array().is_some_and(|l| !l.is_empty())
                || n["detail"].as_str().unwrap_or("").is_empty()
            {
                info_lines(&n["lines"])
            } else {
                info_lines(&json!([[n["detail"], 0]]))
            };
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
                routed: n["routed"] == json!(true),
                card: card.into(),
                state: state.into(),
                state_text: node_state_text(state, &error),
                lines,
                autostart: n["autostart"] == json!(true),
                auto_restart: n["auto_restart"] == json!(true),
                group_id: 0,
                swatch: hex_color(n["swatch"].as_str().unwrap_or("")).unwrap_or(WIRE),
                own_color: n["color"].as_str().is_some_and(|c| !c.is_empty()),
                out_colors: ModelRc::new(VecModel::from(
                    n["out_colors"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|c| OutColor {
                            key: c[0].as_str().unwrap_or("").into(),
                            own: c[1].as_str().is_some_and(|x| !x.is_empty()),
                            color: hex_color(c[2].as_str().unwrap_or("")).unwrap_or(WIRE),
                        })
                        .collect::<Vec<_>>(),
                )),
                has_switch: n["switch"].is_boolean(),
                switch_on: n["switch"] == json!(true),
                rate: SharedString::new(),
                badge: SharedString::new(),
                badge_tone: 0,
                menu: leaves(&n["menu"]["head"]),
                menu_sub_title: n["menu"]["sub_title"].as_str().unwrap_or("").into(),
                menu_sub: leaves(&n["menu"]["sub"]),
                menu_cut: n["menu"]["cut"] == json!(true),
                menu_tail: n["menu"]["tail"] == json!(true),
            });
        }
        // canvas groups: each block's group, the rows (bounds are fitted once the blocks are measured)
        let groups_raw: Vec<&Value> = c["groups"].as_array().into_iter().flatten().collect();
        let mut group_rows: Vec<(String, Vec<i32>, bool)> = vec![];
        for (gi, gr) in groups_raw.iter().enumerate() {
            let members: Vec<i32> = gr["members"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(|k| ids.get(k).copied())
                .collect();
            for n in nodes.iter_mut().filter(|n| members.contains(&n.id)) {
                n.group_id = gi as i32 + 1;
            }
            group_rows.push((gr["id"].as_str().unwrap_or("").to_string(), members, gr["collapsed"] == json!(true)));
        }
        let selected_groups: Vec<String> = GROUPS.with(|g| {
            let g = g.borrow();
            self.groups
                .iter()
                .filter(|r| r.selected)
                .filter_map(|r| g.get(r.id as usize - 1).map(|x| x.0.clone()))
                .collect()
        });
        let rows: Vec<GroupData> = groups_raw
            .iter()
            .enumerate()
            .map(|(gi, gr)| {
                let (x, y, w, h) = group_bounds(&nodes, &group_rows[gi].1);
                GroupData {
                    id: gi as i32 + 1,
                    title: gr["name"].as_str().unwrap_or("").into(),
                    x,
                    y,
                    width: w,
                    height: h,
                    color: hex_color(gr["color"].as_str().unwrap_or("")).unwrap_or(Color::from_argb_u8(0, 0, 0, 0)),
                    collapsed: group_rows[gi].2,
                    selected: selected_groups.contains(&group_rows[gi].0),
                }
            })
            .collect();
        // a collapsed group: a block stands in for its members, with a port per link crossing its edge
        let mut standin_pins: HashMap<i32, i32> = HashMap::new();
        let mut reroute: HashMap<usize, (i32, i32, i32, i32)> = HashMap::new(); // link index -> new (a, out pin, b, in pin)
        let raw_links: Vec<&Value> = c["links"].as_array().into_iter().flatten().collect();
        for (gi, (_, members, collapsed)) in group_rows.iter().enumerate() {
            if !*collapsed {
                continue;
            }
            let sid = nodes.len() as i32 + 1;
            let mut ins: Vec<(i32, String)> = vec![]; // member pin, label
            let mut outs: Vec<(i32, String)> = vec![];
            let title_of = |id: i32| nodes.iter().find(|n| n.id == id).map(|n| n.title.to_string()).unwrap_or_default();
            for (k, l) in raw_links.iter().enumerate() {
                let (Some(a), Some(b)) = (
                    ids.get(l["from"].as_str().unwrap_or("")).copied(),
                    ids.get(l["to"].as_str().unwrap_or("")).copied(),
                ) else {
                    continue;
                };
                let (out, inp) = (l["out"].as_i64().unwrap_or(0) as i32, l["in"].as_i64().unwrap_or(0) as i32);
                let (ain, bin) = (members.contains(&a), members.contains(&b));
                if ain == bin {
                    continue;
                }
                let entry = reroute.entry(k).or_insert((a, pin_out(a, out), b, pin_in(b, inp)));
                if ain {
                    let real = pin_out(a, out);
                    let port = outs.iter().position(|(p, _)| *p == real).unwrap_or_else(|| {
                        outs.push((real, title_of(a)));
                        outs.len() - 1
                    }) as i32;
                    let pin = pin_out(sid, port.min(31));
                    standin_pins.insert(pin, real);
                    entry.0 = sid;
                    entry.1 = pin;
                } else {
                    let real = pin_in(b, inp);
                    let port = ins.iter().position(|(p, _)| *p == real).unwrap_or_else(|| {
                        ins.push((real, title_of(b)));
                        ins.len() - 1
                    }) as i32;
                    let pin = pin_in(sid, port.min(31));
                    standin_pins.insert(pin, real);
                    entry.2 = sid;
                    entry.3 = pin;
                }
            }
            let r = &rows[gi];
            let lines = vec![InfoLine {
                text: tr("{count} blocks").replace("{count}", &members.len().to_string()).into(),
                tone: 2,
            }];
            nodes.push(NodeData {
                id: sid,
                title: r.title.clone(),
                kind: 5,
                x: r.x,
                y: r.y + GROUP_HEAD + 4.0,
                ins: ins.len().min(31) as i32,
                outs: outs.len().min(31) as i32,
                in_labels: ModelRc::new(VecModel::from(
                    ins.iter().map(|(_, t)| SharedString::from(t.as_str())).collect::<Vec<_>>(),
                )),
                out_labels: ModelRc::new(VecModel::from(
                    outs.iter().map(|(_, t)| SharedString::from(t.as_str())).collect::<Vec<_>>(),
                )),
                lines: ModelRc::new(VecModel::from(lines)),
                swatch: r.color,
                ..Default::default()
            });
            ids.insert(format!("group:{}", group_rows[gi].0), sid);
        }
        GROUPS.with(|g| *g.borrow_mut() = group_rows);
        STANDIN_PINS.with(|s| *s.borrow_mut() = standin_pins);
        self.groups.set_vec(rows);
        let mut links = vec![];
        let mut meta = vec![];
        let mut mixed = vec![];
        for (k, l) in c["links"].as_array().into_iter().flatten().enumerate() {
            let (Some(a), Some(b)) =
                (ids.get(l["from"].as_str().unwrap_or("")), ids.get(l["to"].as_str().unwrap_or("")))
            else {
                continue;
            };
            let full = hex_color(l["color"].as_str().unwrap_or("")).unwrap_or(WIRE);
            // a mix: the line faint, dots in the mixed colour over it
            let color = if l["mixed"] == json!(true) {
                mixed.push((links.len(), full));
                full.with_alpha(0.3)
            } else {
                full
            };
            let (out, inp) = (l["out"].as_i64().unwrap_or(0) as i32, l["in"].as_i64().unwrap_or(0) as i32);
            // (a link crossing a collapsed group's edge: to / from its stand-in block)
            let (a, pa, b, pb) = reroute.get(&k).copied().unwrap_or((*a, pin_out(*a, out), *b, pin_in(*b, inp)));
            links.push(LinkData::new(k as i32 + 1, pa, pb, color));
            meta.push((color, a, b));
        }
        let mut keys = vec![String::new(); nodes.len()];
        for (k, id) in &ids {
            if let Some(slot) = keys.get_mut(*id as usize - 1) {
                *slot = k.clone();
            }
        }
        // the list page: each block's menu, the outbound blocks and the routers
        let mut menus = HashMap::new();
        let (mut outs, mut routers) = (vec![], vec![]);
        for n in c["nodes"].as_array().into_iter().flatten() {
            let key = n["key"].as_str().unwrap_or("").to_string();
            menus.insert(key.clone(), n["menu"].clone());
            let kind = n["kind"].as_i64().unwrap_or(0) as i32;
            if kind == 1 || kind == 2 {
                let row = BlockRow {
                    key: key.into(),
                    title: n["title"].as_str().unwrap_or("").into(),
                    detail: n["detail"].as_str().unwrap_or("").into(),
                    kind,
                    has_switch: n["switch"].is_boolean(),
                    switch_on: n["switch"] == json!(true),
                    menu: leaves(&n["menu"]["head"]),
                    menu_sub_title: n["menu"]["sub_title"].as_str().unwrap_or("").into(),
                    menu_sub: leaves(&n["menu"]["sub"]),
                    menu_cut: n["menu"]["cut"] == json!(true),
                    menu_tail: n["menu"]["tail"] == json!(true),
                };
                if kind == 2 { outs.push(row) } else { routers.push(row) }
            }
        }
        *self.menus.borrow_mut() = menus;
        self.out_rows.set_vec(outs);
        self.router_rows.set_vec(routers);
        for i in 0..self.cards.row_count() {
            if let Some(row) = self.cards.row_data(i) {
                self.cards.set_row_data(i, self.with_menu(row));
            }
        }
        self.nodes.set_vec(nodes);
        self.links.set_vec(links);
        *self.link_meta.borrow_mut() = meta;
        *self.keys.borrow_mut() = keys;
        MIXED.with(|m| *m.borrow_mut() = (mixed, Vec::new()));
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
    fn set_node_state(&self, card: &str, state: &str, error: &str) {
        let text = node_state_text(state, error);
        for i in 0..self.nodes.row_count() {
            let Some(mut n) = self.nodes.row_data(i) else { continue };
            if n.card == card && (n.state != state || n.state_text != text) {
                n.state = state.into();
                n.state_text = text.clone();
                self.nodes.set_row_data(i, n);
            }
        }
    }

    /// bytes per second on each link: links with traffic get wider and brighter, blocks show what passes them
    fn set_flow(&self, rates: &[Value]) {
        let meta = self.link_meta.borrow();
        let mut per_node: HashMap<i32, (f64, f64)> = HashMap::new(); // (in, out)
        let mut link_rates = self.rates.borrow_mut();
        link_rates.clear();
        for i in 0..self.links.row_count() {
            let Some(mut l) = self.links.row_data(i) else { continue };
            let Some((color, a, b)) = meta.get(i).copied() else { continue };
            let r = rates.get(l.id as usize - 1).and_then(Value::as_f64).unwrap_or(0.0);
            link_rates.push(r);
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

    /// a card row with its block's ⋯ menu
    fn with_menu(&self, mut row: CardRow) -> CardRow {
        if let Some(m) = self.menus.borrow().get(row.id.as_str()) {
            row.menu = leaves(&m["head"]);
            row.menu_sub_title = m["sub_title"].as_str().unwrap_or("").into();
            row.menu_sub = leaves(&m["sub"]);
            row.menu_cut = m["cut"] == json!(true);
            row.menu_tail = m["tail"] == json!(true);
        }
        row
    }

    fn set_card(&self, c: &Value) {
        let state = c["state"].as_str().unwrap_or("");
        let error = if state == "error" { c["error"].as_str().unwrap_or("") } else { "" };
        self.set_node_state(c["id"].as_str().unwrap_or(""), state, error);
        let row = self.with_menu(card_row(c));
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
    /// a row's buttons' actions (an item's, or the buttons drawn on a field's row)
    actions: HashMap<String, Vec<String>>,
}

thread_local! {
    static EDIT_REQUEST: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    static EDITOR: std::cell::RefCell<Editor> = std::cell::RefCell::new(Editor::default());
}

fn field_rows(fields: &[Value]) -> Vec<FieldRow> {
    let mut rows = EDITOR.with(|e| {
        let mut e = e.borrow_mut();
        e.options.clear();
        e.rerender.clear();
        e.kinds.clear();
        e.actions.clear();
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
                let buttons: Vec<SharedString> = if kind == "item" {
                    opts.iter().map(|(_, l)| SharedString::from(l.as_str())).collect()
                } else {
                    vec![]
                };
                if f["rerender"] == json!(true) {
                    e.rerender.insert(key.clone());
                }
                e.options.insert(key.clone(), opts.iter().map(|(v, _)| v.clone()).collect());
                if kind == "item" {
                    e.actions.insert(key.clone(), opts.iter().map(|(v, _)| v.clone()).collect());
                }
                e.kinds.insert(key.clone(), kind.clone());
                FieldRow {
                    key: key.into(),
                    label: f["label"].as_str().unwrap_or("").into(),
                    desc: f["desc"].as_str().unwrap_or("").into(),
                    kind: kind.into(),
                    text: text.into(),
                    checked: value == &json!(true),
                    options: ModelRc::new(VecModel::from(
                        opts.into_iter().map(|(_, l)| SharedString::from(l)).collect::<Vec<_>>(),
                    )),
                    index,
                    visible: f["visible"] == json!(true),
                    buttons: ModelRc::new(VecModel::from(buttons)),
                    busy: f["busy"] == json!(true),
                    tone: f["tone"].as_i64().unwrap_or(0) as i32,
                    first: false,
                    last: false,
                    joined: false,
                    held: false,
                }
            })
            .collect::<Vec<_>>()
    });
    mark_groups(&mut rows);
    merge_buttons(&mut rows);
    mark_groups(&mut rows);
    rows
}

/// A button that continues a field's card (Generate, Browse, Refresh, Add...) is drawn on that field's row,
/// beside its box, instead of on a line of its own.
fn merge_buttons(rows: &mut [FieldRow]) {
    let shown: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].visible).collect();
    let mut target: Option<usize> = None;
    let mut added: HashMap<usize, Vec<(String, String)>> = HashMap::new();
    for &i in &shown {
        let r = &rows[i];
        if r.kind == "button"
            && r.joined
            && let Some(t) = target
        {
            added.entry(t).or_default().push((r.key.to_string(), r.label.to_string()));
            rows[i].visible = false;
            continue;
        }
        target = matches!(r.kind.as_str(), "text" | "secret" | "int" | "choice" | "bool").then_some(i);
    }
    EDITOR.with(|e| {
        let mut e = e.borrow_mut();
        for (t, buttons) in added {
            let key = rows[t].key.to_string();
            e.actions.insert(key, buttons.iter().map(|(a, _)| a.clone()).collect());
            rows[t].buttons = ModelRc::new(VecModel::from(
                buttons.into_iter().map(|(_, l)| SharedString::from(l)).collect::<Vec<_>>(),
            ));
        }
    });
}

/// The cards' places in their groups: the rows shown between headings make one group (its first and
/// last cards get round outer corners); a heading that is the first row shown has no room above it.
/// A row that continues the one above (a second box, a button) shares its card.
fn mark_groups(rows: &mut [FieldRow]) {
    let shown: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].visible).collect();
    let heads: Vec<bool> = shown.iter().map(|&i| rows[i].kind == "heading").collect();
    let joins: Vec<bool> = shown
        .iter()
        .enumerate()
        .map(|(n, &i)| {
            n > 0
                && !heads[n]
                && !heads[n - 1]
                && (rows[i].label.is_empty() || (rows[i].kind == "button" && rows[i].desc.is_empty()))
        })
        .collect();
    for (n, &i) in shown.iter().enumerate() {
        let prev_head = n.checked_sub(1).map(|m| heads[m]);
        let next_head = heads.get(n + 1).copied();
        rows[i].first = if heads[n] { prev_head.is_none() } else { prev_head.unwrap_or(true) };
        rows[i].last = !heads[n] && next_head.unwrap_or(true);
        rows[i].joined = joins[n];
        rows[i].held = joins.get(n + 1).copied().unwrap_or(false);
    }
}

type CurveOf = Box<dyn Fn(i32, i32) -> Option<slint_node_editor::path::CubicBezier>>;

thread_local! {
    /// a link's curve on the canvas (world units) by its two pins
    static CURVE: std::cell::RefCell<Option<CurveOf>> = const { std::cell::RefCell::new(None) };
    /// the moving dots: their timer, each link's offset along its curve, the last tick and the dots' buffer
    static FLOW_TIMER: slint::Timer = slint::Timer::default();
    static FLOW_STATE: std::cell::RefCell<(Vec<f32>, Option<std::time::Instant>, Vec<FlowDot>)> = const { std::cell::RefCell::new((Vec::new(), None, Vec::new())) };
}

/// Dots along the links that carry traffic, moving toward the exit (faster with more traffic), 20 frames a second. They run only
/// while the canvas is shown, the window is not minimised and some link has traffic: the timer stops itself
/// otherwise and the next "flow" (every second) starts it again.
fn start_flow_dots(app: &App) {
    if FLOW_TIMER.with(|t| t.running()) {
        return;
    }
    let weak = app.as_weak();
    FLOW_TIMER.with(|t| {
        t.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(50), move || {
            let Some(app) = weak.upgrade() else { return };
            if !flow_dots_tick(&app) {
                FLOW_TIMER.with(|t| t.stop());
                FLOW_STATE.with(|s| s.borrow_mut().1 = None);
                if app.get_flow_dots().row_count() > 0 {
                    app.set_flow_dots(ModelRc::default());
                }
            }
        });
    });
}

/// One frame of the dots; false when there is nothing to move (or nothing to show it on)
fn flow_dots_tick(app: &App) -> bool {
    const SPACING: f32 = 44.0; // world px between dots
    if app.get_page() != 0 || app.window().is_minimized() {
        return false;
    }
    let Some(m) = MODELS.with(|m| m.borrow().clone()) else { return false };
    let rates = m.rates.borrow();
    if !rates.iter().any(|r| *r > 0.0) {
        return false;
    }
    FLOW_STATE.with(|st| {
        let mut st = st.borrow_mut();
        let (offsets, last, dots) = &mut *st;
        let now = std::time::Instant::now();
        let dt = last.map_or(0.0, |l| (now - l).as_secs_f32().min(0.1));
        *last = Some(now);
        offsets.resize(rates.len(), 0.0);
        dots.clear();
        CURVE.with(|c| {
            let c = c.borrow();
            let Some(curve_of) = c.as_ref() else { return };
            let mut pts = [(0.0f32, 0.0f32); 25];
            let mut lens = [0.0f32; 25];
            for (i, r) in rates.iter().enumerate() {
                if *r <= 0.0 {
                    continue;
                }
                let Some(l) = m.links.row_data(i) else { continue };
                let Some(curve) = curve_of(l.start_pin_id, l.end_pin_id) else { continue };
                // the curve as 24 straight pieces, with the length up to each point
                for k in 0..pts.len() {
                    pts[k] = curve.eval(k as f32 / 24.0);
                    if k > 0 {
                        let (dx, dy) = (pts[k].0 - pts[k - 1].0, pts[k].1 - pts[k - 1].1);
                        lens[k] = lens[k - 1] + (dx * dx + dy * dy).sqrt();
                    }
                }
                let total = lens[24];
                if total < 1.0 {
                    continue;
                }
                // 1 KB/s .. 100 MB/s on a log scale: speed 40..200 world px/s, size 7..12 (wider than the line: 3..8)
                let t = ((r.max(1024.0).log10() - 3.0) / 5.0).clamp(0.0, 1.0) as f32;
                offsets[i] = (offsets[i] + (40.0 + 160.0 * t) * dt) % SPACING;
                let color = l.color.brighter(0.6);
                let mut d = offsets[i];
                let mut k = 1;
                while d < total {
                    while k < 24 && lens[k] < d {
                        k += 1;
                    }
                    let seg = (lens[k] - lens[k - 1]).max(1e-3);
                    let f = ((d - lens[k - 1]) / seg).clamp(0.0, 1.0);
                    let (a, b) = (pts[k - 1], pts[k]);
                    dots.push(FlowDot {
                        x: a.0 + (b.0 - a.0) * f,
                        y: a.1 + (b.1 - a.1) * f,
                        size: 7.0 + 5.0 * t,
                        color,
                    });
                    d += SPACING;
                }
            }
        });
        app.set_flow_dots(ModelRc::new(VecModel::from(dots.clone())));
    });
    true
}

thread_local! {
    /// latency badges by block key: (text, tone)
    static BADGES: std::cell::RefCell<HashMap<String, (String, i32)>> = std::cell::RefCell::new(HashMap::new());
    /// "Quit with the X button"
    static CLOSE_EXITS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TOAST_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// the block whose title the rename box edits
    static RENAME_KEY: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

thread_local! {
    static ROUTE_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A logged connection's way on the canvas: its links stand out (the others fade) for a while
fn show_route(app: &App, m: &Models, v: &Value) {
    let err = v["error"].as_str().unwrap_or("");
    let ways: std::collections::HashSet<usize> =
        v["links"].as_array().into_iter().flatten().filter_map(|l| l.as_u64()).map(|l| l as usize).collect();
    if ways.is_empty() {
        show_toast(app, if err.is_empty() { "?" } else { err }, false, 5.0);
        return;
    }
    set_route(m, Some(&ways));
    app.set_page(0);
    show_toast(app, &tr("The connection's way is lit on the canvas"), true, 4.0);
    ROUTE_SEQ.with(|n| n.set(n.get() + 1));
    let seq = ROUTE_SEQ.with(|n| n.get());
    let weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_secs(12), move || {
        if ROUTE_SEQ.with(|n| n.get()) != seq || weak.upgrade().is_none() {
            return;
        }
        if let Some(m) = MODELS.with(|m| m.borrow().clone()) {
            set_route(&m, None);
        }
    });
}

/// lights the links of `ways` (by their index in the core's canvas links) and fades the rest; None: all as usual
fn set_route(m: &Models, ways: Option<&std::collections::HashSet<usize>>) {
    for i in 0..m.links.row_count() {
        let Some(mut l) = m.links.row_data(i) else { continue };
        // (link ids are the core's link index + 1)
        let on = ways.is_some_and(|w| w.contains(&(l.id as usize - 1)));
        let dim = ways.is_some() && !on;
        if l.selected != on || l.dimmed != dim {
            l.selected = on;
            l.dimmed = dim;
            m.links.set_row_data(i, l);
        }
    }
}

thread_local! {
    /// what "yes" in the question box does (a message to the core)
    static CONFIRM: std::cell::RefCell<Option<Value>> = const { std::cell::RefCell::new(None) };
}

/// Ask before deleting blocks (Python `confirm_delete`): "yes" sends `then`
fn ask_delete(weak: &slint::Weak<App>, names: &[String], then: Value) {
    let Some(a) = weak.upgrade() else { return };
    let list = names.join(", ");
    CONFIRM.with(|c| *c.borrow_mut() = Some(then));
    a.set_t_yes(tr("Delete").into());
    a.set_confirm_text(tr("Delete '{name}'? It will be stopped if it is running.").replace("{name}", &list).into());
}

// --- the log's filters (Python `logs.LogsInterface._matcher`) ---

/// the level groups (Python `LEVEL_FILTERS`): name, level tags ([] = all)
fn level_filters() -> Vec<(String, &'static [&'static str])> {
    vec![
        (tr("All"), &[]),
        (tr("Connections (TCP+UDP)"), &["TCP", "UDP"]),
        ("TCP".into(), &["TCP"]),
        ("UDP".into(), &["UDP"]),
        (tr("All problems"), &["BLOCK", "AUTH", "FAIL", "ERROR", "WARN"]),
        (tr("Blocked"), &["BLOCK"]),
        (tr("Auth failures"), &["AUTH"]),
        (tr("Connection failures"), &["FAIL"]),
        (tr("Errors & warnings"), &["ERROR", "WARN"]),
        (tr("Settings"), &["SET"]),
        (tr("Info"), &["INFO"]),
    ]
}

#[derive(Default)]
struct LogFilter {
    /// the card shown ("" = the app and every card), the level group, the words (lower case)
    card: String,
    levels: Vec<&'static str>,
    words: String,
    /// the cards in the source list (after "All"), by id
    cards: Vec<String>,
}

thread_local! {
    static LOG_FILTER: std::cell::RefCell<LogFilter> = std::cell::RefCell::new(LogFilter::default());
    static LOG_VIEW: std::cell::RefCell<Option<LogView>> = const { std::cell::RefCell::new(None) };
}

fn log_shown(r: &LogRow) -> bool {
    LOG_FILTER.with(|f| {
        let f = f.borrow();
        (f.card.is_empty() || r.card == f.card.as_str())
            && (f.levels.is_empty() || f.levels.contains(&r.level.as_str()))
            && (f.words.is_empty() || r.text.to_lowercase().contains(&f.words) || r.level.to_lowercase() == f.words)
    })
}

/// the filters from the page's controls, then the view again
fn apply_log_filter(app: &App) {
    LOG_FILTER.with(|f| {
        let mut f = f.borrow_mut();
        let src = app.get_log_source();
        f.card = if src <= 0 { String::new() } else { f.cards.get(src as usize - 1).cloned().unwrap_or_default() };
        f.levels = level_filters().get(app.get_log_level().max(0) as usize).map(|l| l.1.to_vec()).unwrap_or_default();
        f.words = app.get_log_search().trim().to_lowercase();
    });
    LOG_VIEW.with(|v| {
        if let Some(v) = v.borrow().as_ref() {
            v.reset();
        }
    });
}

/// the card list of the source filter (kept on the same card when it is still there)
fn log_sources(app: &App, cards: &[(String, String)]) {
    let keep = LOG_FILTER.with(|f| f.borrow().card.clone());
    let mut names = vec![SharedString::from(tr("All (app + all proxies)"))];
    names.extend(cards.iter().map(|(_, n)| SharedString::from(n.as_str())));
    app.set_log_sources(ModelRc::new(VecModel::from(names)));
    LOG_FILTER.with(|f| f.borrow_mut().cards = cards.iter().map(|(id, _)| id.clone()).collect());
    let pos = cards.iter().position(|(id, _)| *id == keep).map_or(0, |i| i as i32 + 1);
    app.set_log_source(pos);
}

thread_local! {
    // what the toast's button does ("" = no button)
    static TOAST_ACTION: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// a toast with a button (Python's bar with "Restart now")
fn show_toast_action(app: &App, text: &str, button: &str, action: &str, secs: f32) {
    TOAST_ACTION.with(|t| *t.borrow_mut() = action.to_string());
    show_toast(app, text, false, secs);
    app.set_toast_action(button.into());
}

/// an RGBA picture from the picker as a Slint image
fn pick_image(img: proxydock::colorpick::Rgba) -> slint::Image {
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(img.w, img.h);
    buf.make_mut_bytes().copy_from_slice(&img.px);
    slint::Image::from_rgba8(buf)
}

/// Open the colour picker at `initial` ("#rrggbb" / "oklch(...)"); OK hands the stored colour to `apply`
fn open_color_picker(app: &App, initial: &str, title: &str, apply: Rc<dyn Fn(String)>) {
    let monitor = proxydock::gamut::monitor_of(owner_hwnd(app) as isize);
    let picker = proxydock::colorpick::Picker::new(initial, monitor);
    let p = app.global::<Pick>();
    p.set_title(if title.is_empty() { tr("Colour") } else { format!("{} · {title}", tr("Colour")) }.into());
    p.set_t_monitor(
        match picker.monitor_name() {
            n if n.is_empty() => tr("Monitor colour profile unknown: sRGB assumed"),
            n => tr("Monitor colour profile: {name}").replace("{name}", &n),
        }
        .into(),
    );
    COLOR_PICKER.with(|k| *k.borrow_mut() = Some((picker, apply)));
    picker_show(app, true);
    p.set_open(true);
}

/// Put the picker's colour on the dialog (`ring`: the hue ring too, which only changes with the space)
fn picker_show(app: &App, ring: bool) {
    COLOR_PICKER.with(|k| {
        let k = k.borrow();
        let Some((picker, _)) = k.as_ref() else { return };
        let p = app.global::<Pick>();
        // pictures at the screen's own pixels
        let scale = app.window().scale_factor();
        if ring {
            p.set_ring(pick_image(picker.ring_image((264.0 * scale) as u32, (22.0 * scale) as f64)));
        }
        let sq = ((264.0 / 2.0 - 22.0 - 6.0) * 1.414 * scale) as u32;
        p.set_square(pick_image(picker.square_image(sq.max(16))));
        let v = picker.view();
        p.set_space(picker.space as i32);
        p.set_hue(v.hue as f32);
        p.set_square_x(v.square_x as f32);
        p.set_square_y(v.square_y as f32);
        p.set_sliders(ModelRc::new(VecModel::from(v.sliders.map(|x| x as f32).to_vec())));
        p.set_slider_labels(ModelRc::new(VecModel::from(v.slider_labels.map(SharedString::from).to_vec())));
        p.set_slider_values(ModelRc::new(VecModel::from(v.slider_values.map(SharedString::from).to_vec())));
        p.set_slider_images(ModelRc::new(VecModel::from(v.slider_images.map(pick_image).to_vec())));
        p.set_fields(ModelRc::new(VecModel::from(v.fields.map(SharedString::from).to_vec())));
        p.set_new_color(hex(&v.new_hex));
        p.set_old_color(hex(&v.old_hex));
        p.set_wide(v.wide);
        p.set_unseen(v.unseen);
    });
}

/// Windows' accent colour (the DWM's "AccentColor", 0xAABBGGRR) as "#rrggbb"
fn system_accent() -> Option<String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
        let key: Vec<u16> = "Software\\Microsoft\\Windows\\DWM".encode_utf16().chain(Some(0)).collect();
        let name: Vec<u16> = "AccentColor".encode_utf16().chain(Some(0)).collect();
        let (mut v, mut len) = (0u32, 4u32);
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut v as *mut u32).cast(),
                &mut len,
            )
        };
        if r == 0 {
            return Some(format!("#{:02x}{:02x}{:02x}", v & 0xff, (v >> 8) & 0xff, (v >> 16) & 0xff));
        }
    }
    #[cfg(not(windows))]
    {
        let out = |cmd: &[&str]| -> Option<String> {
            let o = std::process::Command::new(cmd[0]).args(&cmd[1..]).output().ok()?;
            o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        // macOS: the accent's number (none = multicolour: blue), in Apple's system colours
        if cfg!(target_os = "macos") {
            let n = out(&["defaults", "read", "-g", "AppleAccentColor"]).and_then(|s| s.parse::<i32>().ok());
            return Some(
                match n {
                    Some(-1) => "#8e8e93",
                    Some(0) => "#ff3b30",
                    Some(1) => "#ff9500",
                    Some(2) => "#ffcc00",
                    Some(3) => "#28cd41",
                    Some(5) => "#af52de",
                    Some(6) => "#ff2d55",
                    _ => "#007aff",
                }
                .into(),
            );
        }
        // KDE: kdeglobals' AccentColor "r,g,b"
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_uppercase();
        if desktop.contains("KDE") {
            for tool in ["kreadconfig6", "kreadconfig5"] {
                if let Some(rgb) = out(&[tool, "--file", "kdeglobals", "--group", "General", "--key", "AccentColor"]) {
                    let c: Vec<u8> = rgb.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    if c.len() >= 3 {
                        return Some(format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]));
                    }
                }
            }
        }
        // GNOME 47+: a named accent, in libadwaita's colours
        if let Some(name) = out(&["gsettings", "get", "org.gnome.desktop.interface", "accent-color"]) {
            let hex = match name.trim_matches('\'') {
                "blue" => "#3584e4",
                "teal" => "#2190a4",
                "green" => "#3a944a",
                "yellow" => "#c88800",
                "orange" => "#ed5b00",
                "red" => "#e62d42",
                "pink" => "#d56199",
                "purple" => "#9141ac",
                "slate" => "#6f8396",
                _ => "",
            };
            if !hex.is_empty() {
                return Some(hex.into());
            }
        }
    }
    None
}

/// The window's colours from the theme colour ("" = the app's, "system" = Windows' accent, else a colour)
fn apply_theme_color(app: &App, value: &str) {
    use proxydock_core::theme;
    let seed = match value {
        "" => theme::APP_SEED.to_string(),
        "system" => system_accent().unwrap_or_else(|| theme::APP_SEED.to_string()),
        c => c.to_string(),
    };
    THEME_COLOR.with(|t| *t.borrow_mut() = value.to_string());
    let m = app.global::<M3>();
    for (role, l, d) in theme::scheme(&seed) {
        let (l, d) = (hex(&l), hex(&d));
        match role {
            "primary" => {
                m.set_l_primary(l);
                m.set_d_primary(d);
            }
            "on-primary" => {
                m.set_l_on_primary(l);
                m.set_d_on_primary(d);
            }
            "primary-container" => {
                m.set_l_primary_container(l);
                m.set_d_primary_container(d);
            }
            "on-primary-container" => {
                m.set_l_on_primary_container(l);
                m.set_d_on_primary_container(d);
            }
            "secondary-container" => {
                m.set_l_secondary_container(l);
                m.set_d_secondary_container(d);
            }
            "on-secondary-container" => {
                m.set_l_on_secondary_container(l);
                m.set_d_on_secondary_container(d);
            }
            "tertiary-container" => {
                m.set_l_tertiary_container(l);
                m.set_d_tertiary_container(d);
            }
            "on-tertiary-container" => {
                m.set_l_on_tertiary_container(l);
                m.set_d_on_tertiary_container(d);
            }
            "surface" => {
                m.set_l_surface(l);
                m.set_d_surface(d);
            }
            "surface-container-low" => {
                m.set_l_surface_container_low(l);
                m.set_d_surface_container_low(d);
            }
            "surface-container" => {
                m.set_l_surface_container(l);
                m.set_d_surface_container(d);
            }
            "surface-container-high" => {
                m.set_l_surface_container_high(l);
                m.set_d_surface_container_high(d);
            }
            "surface-container-highest" => {
                m.set_l_surface_container_highest(l);
                m.set_d_surface_container_highest(d);
            }
            "on-surface" => {
                m.set_l_on_surface(l);
                m.set_d_on_surface(d);
            }
            "on-surface-variant" => {
                m.set_l_on_surface_variant(l);
                m.set_d_on_surface_variant(d);
            }
            "outline-variant" => {
                m.set_l_outline_variant(l);
                m.set_d_outline_variant(d);
            }
            _ => {}
        }
    }
    // the settings page's own dots
    let st = app.global::<Settings>();
    st.set_system_accent(hex(&system_accent().unwrap_or_else(|| theme::APP_SEED.to_string())));
    let custom = value != "system" && !value.is_empty() && !theme::PRESETS.contains(&value);
    st.set_theme_color_custom(custom);
    if custom {
        st.set_custom_color(hex_color(value).unwrap_or(hex(theme::APP_SEED)));
    }
}

/// a message at the bottom of the canvas for `secs` (a newer one replaces it and its timer)
fn show_toast(app: &App, text: &str, info: bool, secs: f32) {
    app.set_toast_action(SharedString::new());
    app.set_toast(text.into());
    app.set_toast_info(info);
    TOAST_SEQ.with(|n| n.set(n.get() + 1));
    let seq = TOAST_SEQ.with(|n| n.get());
    let weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_secs_f32(secs), move || {
        if let Some(a) = weak.upgrade()
            && TOAST_SEQ.with(|n| n.get()) == seq
        {
            a.set_toast(SharedString::new());
        }
    });
}

/// puts `text` on the clipboard; false if it could not
fn set_clipboard_text(text: &str) -> bool {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::GlobalFree;
        use windows_sys::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
        };
        use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
        const CF_UNICODETEXT: u32 = 13;
        let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();
        let h = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
        let mut ok = false;
        if !h.is_null() {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                GlobalUnlock(h);
                // the clipboard owns the memory once it takes it
                ok = !SetClipboardData(CF_UNICODETEXT, h).is_null();
            }
            if !ok {
                GlobalFree(h);
            }
        }
        CloseClipboard();
        ok
    }
    #[cfg(not(windows))]
    {
        let _ = text;
        false
    }
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
    let tc = vals["theme_color"].as_str().unwrap_or("").to_string();
    st.set_theme_color(tc.as_str().into());
    st.set_saved_theme_color(tc.as_str().into());
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
    let (ui_font, mono_font) =
        (vals["ui_font"].as_str().unwrap_or("").to_string(), vals["mono_font"].as_str().unwrap_or("").to_string());
    FONTS.with(|f| {
        let mut f = f.borrow_mut();
        f.ui_choice = ui_font.clone();
        let code = proxydock_core::i18n::current();
        let builtin = tr("Built-in · {builtin_ui}")
            .replace("{builtin_ui}", if lang_font(&code).is_empty() { system_ui_font() } else { lang_font(&code) });
        let ui_names: Vec<SharedString> = std::iter::once(SharedString::from(builtin))
            .chain(f.ui.iter().map(|x| SharedString::from(x.as_str())))
            .collect();
        let mono_names: Vec<SharedString> = std::iter::once(SharedString::from(
            tr("Built-in · {builtin_mono}").replace("{builtin_mono}", "JetBrains Mono"),
        ))
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
    app.global::<Type>().set_mono(if mono_font.is_empty() {
        "JetBrains Mono".into()
    } else {
        mono_font.as_str().into()
    });
    apply_fonts(app, &proxydock_core::i18n::current());
    st.set_admin(b("run_as_admin"));
    st.set_windows(v["windows"] == json!(true));
    st.set_startup_supported(v["startup_supported"] == json!(true));
    st.set_login(v["startup"]["registered"] == json!(true));
    st.set_hidden(v["startup"]["hidden"] == json!(true));
    let fw = match v["firewall"].as_bool() {
        Some(true) => 1,
        Some(false) => 0,
        None => -1,
    };
    st.set_fw_state(fw);
    st.set_fw_text(
        tr(match fw {
            1 => "On",
            0 => "Off",
            _ => "Checking…",
        })
        .into(),
    );
    st.set_engine(v["engine"].as_str().unwrap_or("").into());
    st.set_log_dir(v["log_dir"].as_str().unwrap_or("").into());
    // what applies to this window right away
    app.set_theme(i("theme"));
    apply_theme_color(app, &tc);
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
        "ja" => {
            if windows {
                "Yu Gothic UI"
            } else if mac {
                "Hiragino Sans"
            } else {
                "Noto Sans CJK JP"
            }
        }
        "zh_CN" => {
            if windows {
                "Microsoft YaHei UI"
            } else if mac {
                "PingFang SC"
            } else {
                "Noto Sans CJK SC"
            }
        }
        "zh_TW" => {
            if windows {
                "Microsoft JhengHei UI"
            } else if mac {
                "PingFang TC"
            } else {
                "Noto Sans CJK TC"
            }
        }
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
        use windows_sys::Win32::Graphics::Gdi::{
            DEFAULT_CHARSET, EnumFontFamiliesExW, GetDC, LOGFONTW, ReleaseDC, TEXTMETRICW,
        };
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
            lf.lfCharSet = DEFAULT_CHARSET;
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
    if cfg!(windows) {
        "Segoe UI"
    } else if cfg!(target_os = "macos") {
        "SF Pro"
    } else {
        "Sans"
    }
}

/// the window's font for the language, and each language's name in its own font
fn apply_fonts(app: &App, code: &str) {
    // the user's UI font, else the language's own
    let chosen = FONTS.with(|f| f.borrow().ui_choice.clone());
    app.set_ui_font(if chosen.is_empty() { lang_font(code).into() } else { chosen.as_str().into() });
    let fonts: Vec<SharedString> =
        proxydock_core::i18n::LANGUAGES.iter().map(|(c, _)| SharedString::from(lang_font(c))).collect();
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
        dns_desc: SharedString::from(format!(
            "{} {}",
            tr(
                "Empty = the PC's DNS. Otherwise 1.1.1.1 (UDP), tcp://, tls:// (DoT), https://…/dns-query (DoH), quic:// or h3://. Used where ProxyDock looks names up itself (Direct, IP rules, TUN). Needs the sing-box engine"
            ),
            tr("Several servers (separated by commas): all are asked at once and the first answer is used.")
        )),
        dnsvia: t("DNS through the outbound"),
        dnsvia_desc: t(
            "The card's own lookups go through its tunnel or upstream instead of leaving this PC directly (needs a DNS server above)",
        ),
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
        admin_desc: t(
            "ProxyDock restarts itself with administrator rights (UAC prompt) when it starts, so TUN cards start without asking. Applies from the next start.",
        ),
        updates: t("Updates"),
        check_updates: t("Check for updates at startup"),
        check_updates_desc: t(
            "Asks GitHub for the latest release at most once a day. Only the app version is sent (as the User-Agent).",
        ),
        firewall: t("Windows Firewall"),
        fw_block: t("Block external access"),
        fw_block_desc: t(
            "Blocks connections from other PCs to ProxyDock (all proxies, needs administrator rights). Turn off to use servers for other devices (Shadowsocks, Hysteria2, etc.)",
        ),
        fw_on: t("Turn block on"),
        fw_off: t("Turn block off"),
        cat_defaults: t("Proxy defaults"),
        cat_startup: t("Startup"),
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
    // (what installing does here: Windows' installer, macOS' disk image, Linux' AppImage)
    u.set_t_install_text(
        tr(if cfg!(target_os = "macos") {
            "The disk image opens. Quit ProxyDock, then drag ProxyDock into Applications to replace the old version."
        } else if cfg!(target_os = "linux") {
            "The AppImage file is replaced with the new version and ProxyDock restarts. Running proxies stop until then."
        } else {
            "ProxyDock closes, the installer updates it and then starts it again. Running proxies stop until then."
        })
        .into(),
    );
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
    {
        let t = app.global::<CanvasText>();
        t.set_run(tr("Run").into());
        t.set_at_start(tr("At start").into());
        t.set_auto_restart(tr("Auto-restart").into());
        t.set_switched_on(tr("Switched on").into());
        t.set_switched_off(tr("Switched off").into());
        t.set_stop(tr("Stop").into());
        t.set_add_inbound(tr("Add inbound").into());
        t.set_add_outbound(tr("Add outbound").into());
        t.set_add_router(tr("Add router").into());
        t.set_from_link(tr("From a share link (clipboard)").into());
        t.set_router(tr("Router (several rules)").into());
        t.set_if_block(tr("IF (one condition: then / else)").into());
        t.set_relay(tr("Pass-through (tidy up the wiring)").into());
        t.set_wireless(tr("Wireless pair (linked without a wire)").into());
        t.set_fit(tr("Fit").into());
        t.set_latency(tr("Test latency").into());
        t.set_undo(tr("Undo").into());
        t.set_redo(tr("Redo").into());
        t.set_cut(tr("Disconnect all links").into());
        t.set_add_lane(tr("Add a lane").into());
        t.set_add_input(tr("Input").into());
        t.set_add_output(tr("Output").into());
        let st = app.global::<Settings>();
        st.set_t_theme_color(tr("Theme colour").into());
        st.set_t_system(tr("System").into());
        st.set_t_custom(tr("Custom").into());
        let pick = app.global::<Pick>();
        pick.set_t_cancel(tr("Cancel").into());
        pick.set_t_ok(tr("Apply").into());
        pick.set_t_wide(tr("Beyond sRGB: saved as an OKLCH colour (Display P3)").into());
        pick.set_t_unseen(tr("This monitor cannot show this colour exactly; it is drawn as the nearest one").into());
        t.set_duplicate(tr("Duplicate").into());
        t.set_delete(tr("Delete").into());
    }
    app.set_t_copy(tr("Copy address/link for clients").into());
    app.set_t_search(tr("Search").into());
    app.set_t_pc(tr("PC (system proxy)").into());
    app.set_t_pc_restore(tr("Restore previous").into());
    app.set_t_pc_off(tr("Off (default)").into());
    app.set_t_group(tr("Group blocks").into());
    app.set_t_ungroup(tr("Ungroup").into());
    app.set_t_collapse(tr("Collapse").into());
    app.set_t_expand(tr("Expand").into());
    app.set_t_colour(tr("Colour").into());
    app.set_palette_colors(ModelRc::new(VecModel::from(
        PALETTE.iter().map(|(h, _)| hex_color(h).unwrap()).collect::<Vec<_>>(),
    )));
    app.set_palette_names(ModelRc::new(VecModel::from(
        PALETTE.iter().map(|(_, n)| SharedString::from(tr(n))).collect::<Vec<_>>(),
    )));
    app.set_t_palette_custom(tr("Custom colour…").into());
    app.set_t_pick_title(tr("Choose programs").into());
    app.set_t_pick_search(tr("Search running programs").into());
    app.set_t_pick_file(tr("Choose a program file...").into());
    app.set_t_pick_note(tr("Rules match the program's file name, so it does not need to be running now.").into());
    app.set_t_add(tr("Add").into());
    // (the level groups' names: in the language set by now)
    let levels: Vec<SharedString> = level_filters().into_iter().map(|(n, _)| SharedString::from(n)).collect();
    app.set_log_levels(ModelRc::new(VecModel::from(levels)));
    app.set_t_copy_log(tr("Copy").into());
    app.set_t_clear_log(tr("Clear").into());
    app.set_t_sec_inbound(tr("Inbound").into());
    app.set_t_sec_outbound(tr("Outbound").into());
    app.set_t_sec_routers(tr("Routers").into());
    app.set_t_no_outbound(tr("No outbound blocks.").into());
    app.set_t_new_proxy(tr("New proxy").into());
    app.set_t_new_outbound(tr("New outbound").into());
    app.set_t_empty(tr("No proxies yet. Create one with 'New proxy'.").into());
}

fn main() -> Result<(), slint::PlatformError> {
    // Slint draws its own context menus
    unsafe { std::env::set_var("SLINT_NO_MUDA", "1") };
    // Skia on the GPU: the CPU renderer redraws every pixel of the canvas on each pan or zoom, which gets slow
    // on large windows (measured: 59 ms a frame at 1900x1100). The CPU renderer stays as the fallback.
    if std::env::var_os("SLINT_BACKEND").is_none() {
        let gpu = slint::BackendSelector::new().backend_name("winit".into()).renderer_name("skia".into()).select();
        if gpu.is_err() {
            let _ =
                slint::BackendSelector::new().backend_name("winit".into()).renderer_name("software".into()).select();
        }
    }
    let Some((port, token)) = std::env::var(ipc::ENV).ok().and_then(|v| {
        let mut it = v.split_whitespace();
        Some((it.next()?.parse::<u16>().ok()?, it.next()?.to_string()))
    }) else {
        eprintln!("proxydock-ui is started by proxydock.exe");
        std::process::exit(2);
    };
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| slint::PlatformError::Other(e.to_string()))?;
    ipc::send(&mut stream, &json!({"cmd": "hello", "token": token}))
        .map_err(|e| slint::PlatformError::Other(e.to_string()))?;
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
        rates: Default::default(),
        menus: Default::default(),
        out_rows: Rc::new(VecModel::default()),
        router_rows: Rc::new(VecModel::default()),
        groups: Rc::new(VecModel::default()),
    });
    app.set_groups(ModelRc::from(models.groups.clone()));
    app.set_cards(ModelRc::from(models.cards.clone()));
    {
        // the log page shows the lines its filters let through
        let view: Box<dyn Fn(&LogRow) -> bool> = Box::new(log_shown);
        let filtered = Rc::new(slint::FilterModel::new(ModelRc::from(models.logs.clone()), view));
        app.set_logs(ModelRc::from(filtered.clone()));
        LOG_VIEW.with(|v| *v.borrow_mut() = Some(filtered));
    }
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
    app.set_out_rows(ModelRc::from(models.out_rows.clone()));
    app.set_router_rows(ModelRc::from(models.router_rows.clone()));
    {
        // a connection's log line clicked: ask the core which way it went
        let send = send.clone();
        app.on_log_route(move |card, message| {
            send(json!({"cmd": "route_of", "id": card.as_str(), "message": message.as_str()}))
        });
    }
    // canvas: moving and selecting blocks (a move is saved by the core when the drag ends)
    {
        let nodes = models.nodes.clone();
        let links = models.links.clone();
        let setup = NodeEditorSetup::with_drag_commit({
            let (nodes, send, key_of, groups) = (nodes.clone(), send.clone(), key_of.clone(), models.groups.clone());
            move |dragged, dx, dy| {
                let before: Vec<(f32, f32)> = nodes.iter().map(|n| (n.x, n.y)).collect();
                GroupLogic::commit_drag(&*nodes, &*groups, dragged, dx, dy);
                // a collapsed group's stand-in carries its (hidden) members
                let carried: Vec<i32> = nodes
                    .iter()
                    .enumerate()
                    .filter(|(i, n)| n.kind == 5 && before.get(*i) != Some(&(n.x, n.y)))
                    .map(|(_, n)| n.id)
                    .collect();
                for sid in carried {
                    let key = key_of(sid);
                    let members = GROUPS.with(|g| {
                        g.borrow()
                            .iter()
                            .find(|x| format!("group:{}", x.0) == key)
                            .map(|x| x.1.clone())
                            .unwrap_or_default()
                    });
                    for i in 0..nodes.row_count() {
                        let Some(mut n) = nodes.row_data(i) else { continue };
                        if members.contains(&n.id) && before.get(i) == Some(&(n.x, n.y)) {
                            n.x += dx;
                            n.y += dy;
                            nodes.set_row_data(i, n);
                        }
                    }
                }
                let moved: Vec<Value> = nodes
                    .iter()
                    .enumerate()
                    .filter(|(i, n)| n.kind != 5 && before.get(*i) != Some(&(n.x, n.y)))
                    .map(|(_, n)| json!({"key": key_of(n.id), "x": n.x, "y": n.y}))
                    .collect();
                if !moved.is_empty() {
                    send(json!({"cmd": "move", "blocks": moved}));
                }
                // a single block dropped onto another group's box joins that group
                if let Dragged::Node(id) = dragged {
                    let selected = nodes.iter().filter(|n| n.selected).count();
                    if let Some(n) = nodes.iter().find(|n| n.id == id && n.kind != 5).filter(|_| selected <= 1) {
                        let (w, h) = SIZES.with(|s| s.borrow().get(&n.id).copied().unwrap_or((260.0, 120.0)));
                        if let Some(g) = drop_group(&groups, n.group_id, (n.x, n.y, w, h)) {
                            send(json!({"cmd": "group_add", "id": group_key(g.id), "key": key_of(n.id)}));
                        }
                    }
                }
            }
        });
        let controller = setup.controller().clone();
        {
            // fit: every block in view, from the rectangles the editor measured
            let (controller, weak) = (controller.clone(), app.as_weak());
            app.on_canvas_fit(move || {
                let Some(a) = weak.upgrade() else { return };
                let rects: Vec<(f32, f32, f32, f32)> = controller
                    .cache()
                    .borrow()
                    .node_rects
                    .values()
                    .map(|r| {
                        use slint_node_editor::NodeGeometry;
                        r.rect()
                    })
                    .collect();
                fit_canvas(&a, &rects);
            });
        }
        {
            // a selected wireless pair: focus on it
            let (controller, weak, models) = (controller.clone(), app.as_weak(), models.clone());
            let t = slint::Timer::default();
            t.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(150), move || {
                let Some(a) = weak.upgrade() else { return };
                if a.get_page() != 0 {
                    return;
                }
                let rects: HashMap<i32, (f32, f32, f32, f32)> = controller
                    .cache()
                    .borrow()
                    .node_rects
                    .iter()
                    .map(|(id, r)| {
                        use slint_node_editor::NodeGeometry;
                        (*id, r.rect())
                    })
                    .collect();
                pair_focus(&a, &models, &rects);
                mix_dots(&a, &models, &rects);
                groups_follow(&a, &models, &rects);
            });
            std::mem::forget(t);
        }
        {
            // a block dragged over a group: the drop preview, 30 times a second while dragging
            let (weak, models) = (app.as_weak(), models.clone());
            let t = slint::Timer::default();
            t.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(33), move || {
                if let Some(a) = weak.upgrade()
                    && a.get_page() == 0
                    && (a.global::<DragState>().get_is_dragging() || a.get_drop_show())
                {
                    drop_preview(&a, &models);
                }
            });
            std::mem::forget(t);
        }
        {
            // the arrow keys, over the rectangles the editor measured
            let (controller, weak, models) = (controller.clone(), app.as_weak(), models.clone());
            app.on_canvas_nav(move |dir| {
                let Some(a) = weak.upgrade() else { return };
                let rects: HashMap<i32, (f32, f32, f32, f32)> = controller
                    .cache()
                    .borrow()
                    .node_rects
                    .iter()
                    .map(|(id, r)| {
                        use slint_node_editor::NodeGeometry;
                        (*id, r.rect())
                    })
                    .collect();
                canvas_nav(&a, &models, &rects, dir.as_str());
            });
        }
        {
            // the links' curves, for the moving dots
            let controller = controller.clone();
            CURVE.with(|c| {
                *c.borrow_mut() = Some(Box::new(move |a, b| {
                    controller.cache().borrow().link_curve_world(a, b, controller.bezier_offset())
                }));
            });
        }
        wire_node_editor!(app, setup);
        let groups = models.groups.clone();
        wire_selection!(app, setup, nodes, links, groups);
        {
            // groups: which move with a drag, which hide their members; the sheet's toggle goes to the core
            let computations = app.global::<NodeEditorComputations>();
            let g1 = groups.clone();
            computations.on_group_in_drag(move |gid, node, group| {
                GroupLogic::group_moves(&*g1, gid, Dragged::from_ids(node, group))
            });
            let g2 = groups.clone();
            computations.on_group_collapsed(move |gid, _| GroupLogic::is_collapsed(&*g2, gid));
            let send_c = send.clone();
            app.global::<NodeEditorEvents>().on_group_collapse_requested(move |gid, collapsed| {
                send_c(json!({"cmd": "group_collapse", "id": group_key(gid), "collapsed": collapsed}));
            });
            let (send_r, weak_r) = (send.clone(), app.as_weak());
            let _ = send_r;
            app.global::<NodeEditorEvents>().on_group_double_clicked(move |gid| {
                if let Some(a) = weak_r.upgrade() {
                    a.set_group_sel(gid);
                    a.invoke_group_rename();
                }
            });
        }
        app.on_compute_link_at({
            let links = links.clone();
            let controller = controller.clone();
            move |x, y| {
                let rows = (0..links.row_count()).filter_map(|i| {
                    let l = links.row_data(i)?;
                    Some((l.id, l.start_pin_id, l.end_pin_id))
                });
                controller.cache().borrow().find_bezier_link_at_world(
                    x,
                    y,
                    rows,
                    controller.screen_distance_to_world(8.0),
                    50.0,
                    20,
                )
            }
        });
        // a link dragged between two pins: the core decides (and answers with the new canvas or a reason)
        {
            let (send, key_of) = (send.clone(), key_of.clone());
            // (a stand-in block's pin: the member pin it stands for)
            let link_cmd = move |cmd: &str, out: i32, inp: i32| {
                let (out, inp) = (real_pin(out), real_pin(inp));
                json!({"cmd": cmd, "from": key_of(out / 64), "out": (out % 64) / 2, "to": key_of(inp / 64), "in": (inp % 64) / 2})
            };
            // a link picked up at its input end: shown again where it was (dropped back, or nowhere useful)
            let put_back = {
                let links = links.clone();
                move || {
                    if let Some((_, _, row, color)) = GRAB.take()
                        && let Some(mut l) = links.row_data(row)
                    {
                        l.color = color;
                        links.set_row_data(row, l);
                    }
                }
            };
            // pressing an input pin that has a link picks that link up there: it hangs from its source pin and
            // follows the mouse until dropped on another input (moved), on nothing (removed) or back (kept)
            let (links_g, ctl_g, weak_g) = (links.clone(), controller.clone(), app.as_weak());
            app.global::<NodeEditorInternalCallbacks>().on_grab_input_link(move |pin| {
                let none = PinGrab { pin: 0, x: 0.0, y: 0.0 };
                let Some(app) = weak_g.upgrade() else { return none };
                let Some(row) =
                    (0..links_g.row_count()).rev().find(|&i| links_g.row_data(i).is_some_and(|l| l.end_pin_id == pin))
                else {
                    return none;
                };
                let mut l = links_g.row_data(row).unwrap();
                let cache = ctl_g.cache();
                let Some(p) = cache.borrow().get_absolute_pins().find(|p| p.id == l.start_pin_id) else { return none };
                let z = app.get_canvas_zoom();
                GRAB.set(Some((l.start_pin_id, pin, row, l.color)));
                let start = l.start_pin_id;
                l.color = Color::from_argb_u8(0, 0, 0, 0);
                links_g.set_row_data(row, l);
                PinGrab { pin: start, x: p.x * z + app.get_canvas_pan_x(), y: p.y * z + app.get_canvas_pan_y() }
            });
            let (send_r, cmd_r, back_r) = (send.clone(), link_cmd.clone(), put_back.clone());
            app.on_link_requested(move |a, b| {
                let grabbed = GRAB.get();
                let (out, inp) = if a % 2 == 1 && b % 2 == 0 {
                    (a, b)
                } else if b % 2 == 1 && a % 2 == 0 {
                    (b, a)
                } else {
                    (0, 0)
                };
                if out == 0 || out / 64 == inp / 64 || grabbed.is_some_and(|(o, i, ..)| (o, i) == (out, inp)) {
                    back_r();
                    return;
                }
                if let Some((o, i, ..)) = grabbed {
                    GRAB.set(None);
                    send_r(cmd_r("disconnect", o, i));
                }
                send_r(cmd_r("connect", out, inp));
            });
            let (send_d, cmd_d) = (send.clone(), link_cmd.clone());
            app.on_link_dropped(move |_, _, _| {
                if let Some((o, i, ..)) = GRAB.take() {
                    send_d(cmd_d("disconnect", o, i));
                }
            });
            app.on_link_cancelled(put_back);
        }
    }
    if let Some(p) = std::env::var("PROXYDOCK_PAGE").ok().and_then(|p| p.parse::<i32>().ok()) {
        app.set_page(p);
    }
    labels(&app);
    {
        // the page title bar moves the window (winit: Windows, macOS, Linux alike)
        // (the press starts the system's move, which takes the release: a second press soon after is the
        // double click that maximises / restores)
        let weak = app.as_weak();
        let last = std::cell::Cell::new(None::<std::time::Instant>);
        app.on_drag_window(move || {
            use slint::winit_030::WinitWindowAccessor;
            let Some(a) = weak.upgrade() else { return };
            if last.get().is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(500)) {
                last.set(None);
                a.invoke_toggle_maximized();
                return;
            }
            last.set(Some(std::time::Instant::now()));
            a.window().with_winit_window(|w| {
                let _ = w.drag_window();
            });
        });
        let weak = app.as_weak();
        app.on_toggle_maximized(move || {
            if let Some(a) = weak.upgrade() {
                let w = a.window();
                w.set_maximized(!w.is_maximized());
            }
        });
    }
    {
        let w = writer.clone();
        app.on_toggle(move |id| {
            // the card's own state decides: the core starts a stopped card and stops a running one
            let _ = ipc::send(&mut w.lock().unwrap(), &json!({"cmd": "toggle", "id": id.as_str()}));
        });
    }
    // block editor
    {
        let (send1, key_of1) = (send.clone(), key_of.clone());
        app.on_edit_block(move |node| {
            let key = key_of1(node);
            match key.strip_prefix("group:") {
                Some(gid) => send1(json!({"cmd": "group_collapse", "id": gid, "collapsed": false})),
                None => send1(json!({"cmd": "edit_open", "id": key})),
            }
        });
        let send2 = send.clone();
        app.on_edit_card(move |id| send2(json!({"cmd": "edit_open", "id": id.as_str()})));
        let send3 = send.clone();
        app.on_copy_card(move |id| send3(json!({"cmd": "copy_client", "id": id.as_str()})));
        // the log page: filters, copy what is shown, clear (one card's lines, or all)
        let weak11 = app.as_weak();
        app.on_log_filter(move || {
            if let Some(a) = weak11.upgrade() {
                apply_log_filter(&a);
            }
        });
        let weak12 = app.as_weak();
        app.on_log_copy(move || {
            let Some(a) = weak12.upgrade() else { return };
            let text = LOG_VIEW.with(|v| {
                v.borrow()
                    .as_ref()
                    .map(|v| v.iter().map(|r| r.text.to_string()).collect::<Vec<_>>().join("\n"))
                    .unwrap_or_default()
            });
            if set_clipboard_text(&text) {
                show_toast(&a, &tr("Copied the visible log to the clipboard."), true, 2.5);
            } else {
                show_toast(&a, &tr("Cannot use the clipboard"), false, 5.0);
            }
        });
        let (send13, models13, weak13) = (send.clone(), models.clone(), app.as_weak());
        app.on_log_clear(move || {
            let card = LOG_FILTER.with(|f| f.borrow().card.clone());
            // (the core forgets them too: a window opened later does not get them back)
            send13(json!({"cmd": "log_clear", "id": card}));
            let kept: Vec<LogRow> = if card.is_empty() {
                vec![]
            } else {
                models13.logs.iter().filter(|r| r.card != card.as_str()).collect()
            };
            models13.logs.set_vec(kept);
            if let Some(a) = weak13.upgrade() {
                a.set_log_count(models13.logs.row_count() as i32);
            }
        });
        // the question box's "yes"
        let send4 = send.clone();
        app.on_confirm_yes(move || {
            if let Some(m) = CONFIRM.with(|c| c.borrow_mut().take()) {
                send4(m);
            }
        });
        // a block double-clicked: its editor (Python)
        let (send5, key_of5) = (send.clone(), key_of.clone());
        app.global::<NodeEditorEvents>().on_node_double_clicked(move |node| {
            let key = key_of5(node);
            if key != "pc" {
                send5(json!({"cmd": "edit_open", "id": key}));
            }
        });
        // the empty canvas's menu: a block where it was opened (a share link from the clipboard)
        let send6 = send.clone();
        app.on_canvas_add_at(move |what, t, x, y| {
            let text = if what == "link" { clipboard_text() } else { String::new() };
            send6(json!({"cmd": "add", "what": what.as_str(), "type": t.as_str(), "text": text, "x": x - 115.0, "y": y - 34.0}));
        });
        // a block's menu: most entries go to the core; delete asks first; "go to the partner" is the window's
        let (key_of8, weak8) = (key_of.clone(), app.as_weak());
        app.on_node_action(move |node, action| {
            if let Some(a) = weak8.upgrade() {
                a.invoke_block_action(key_of8(node).into(), action);
            }
        });
        // the list page's add buttons
        let send9 = send.clone();
        app.on_list_add(move |what, t| {
            let index: u64 = t.parse().unwrap_or(0);
            send9(json!({"cmd": "add", "what": what.as_str(), "type": t.as_str(), "index": index, "x": 0.0, "y": 0.0}));
        });
        // an IF's switch on the list page
        let send10 = send.clone();
        app.on_set_switch(move |key, on| send10(json!({"cmd": "set_switch", "id": key.as_str(), "on": on})));
        // a swatch clicked: the palette for the block, or for one router output
        let (models12, key_of12, weak12) = (models.clone(), key_of.clone(), app.as_weak());
        app.on_open_palette(move |id, out, x, y| {
            let Some(a) = weak12.upgrade() else { return };
            let key = key_of12(id);
            let Some(n) = models12.nodes.iter().find(|n| n.id == id) else { return };
            let (title, cur, own) = if out.is_empty() {
                (String::new(), n.swatch, n.own_color)
            } else {
                let i = n.out_colors.iter().position(|c| c.key == out).unwrap_or(0);
                let c = n.out_colors.row_data(i);
                (
                    n.out_labels.row_data(i).map(|s| s.to_string()).unwrap_or_default(),
                    c.as_ref().map_or(WIRE, |c| c.color),
                    c.is_some_and(|c| c.own),
                )
            };
            let cur_hex = format!("#{:02x}{:02x}{:02x}", cur.red(), cur.green(), cur.blue());
            let index = if own { PALETTE.iter().position(|(h, _)| *h == cur_hex).map_or(-1, |i| i as i32) } else { -1 };
            let raw = RAW_COLORS
                .with(|r| r.borrow().get(&(id, out.to_string())).cloned())
                .filter(|r| !r.is_empty())
                .unwrap_or(cur_hex);
            PALETTE_FOR.with(|p| *p.borrow_mut() = (key, out.to_string(), raw));
            a.set_palette_title(title.into());
            a.set_palette_current(index);
            a.set_palette_auto_current(!own);
            // an output without its own colour uses the block's (an IF: the incoming links)
            let auto = if out.is_empty() || n.detail == "IF" {
                tr("Automatic (follow the incoming links)")
            } else {
                tr("Same as the block")
            };
            a.set_t_palette_auto(auto.into());
            a.set_palette_x(x);
            a.set_palette_y(y);
            a.set_palette_open(true);
        });
        let (send12, weak13) = (send.clone(), app.as_weak());
        app.on_palette_pick(move |i| {
            let (key, out, cur) = PALETTE_FOR.with(|p| p.borrow().clone());
            let apply = {
                let send = send12.clone();
                move |color: String| {
                    if let Some(gid) = key.strip_prefix("group:") {
                        send(json!({"cmd": "group_color", "id": gid, "color": color}));
                        return;
                    }
                    let mut msg = json!({"cmd": "color", "key": key, "color": color});
                    if !out.is_empty() {
                        msg["out"] = json!(out);
                    }
                    send(msg);
                }
            };
            match i {
                -1 => apply(String::new()),
                -2 => {
                    // any colour: the colour picker
                    let Some(a) = weak13.upgrade() else { return };
                    let title = a.get_palette_title().to_string();
                    open_color_picker(&a, &cur, &title, Rc::new(apply));
                }
                i => apply(PALETTE.get(i as usize).map(|p| p.0.to_string()).unwrap_or_default()),
            }
        });
        // the list page's PC bar: "Restore previous" / "Off (default)"
        let send_pc = send.clone();
        app.on_pc_action(move |what| send_pc(json!({"cmd": if what == "off" { "pc_off" } else { "pc_restore" }})));
        // the colour picker: every change goes through its model, then the dialog is filled in again
        {
            let pick = app.global::<Pick>();
            let with = |weak: slint::Weak<App>, f: Box<dyn Fn(&mut proxydock::colorpick::Picker) -> bool>| {
                move || {
                    let Some(a) = weak.upgrade() else { return };
                    let ring = COLOR_PICKER.with(|k| k.borrow_mut().as_mut().map(|(p, _)| f(p)).unwrap_or(false));
                    picker_show(&a, ring);
                }
            };
            let w = app.as_weak();
            pick.on_ring_drag(move |deg| {
                with(
                    w.clone(),
                    Box::new(move |p| {
                        p.ring(deg as f64);
                        false
                    }),
                )()
            });
            let w = app.as_weak();
            pick.on_square_drag(move |fx, fy| {
                with(
                    w.clone(),
                    Box::new(move |p| {
                        p.square(fx as f64, fy as f64);
                        false
                    }),
                )()
            });
            let w = app.as_weak();
            pick.on_slider_drag(move |i, f| {
                with(
                    w.clone(),
                    Box::new(move |p| {
                        p.slider(i.max(0) as usize, f as f64);
                        false
                    }),
                )()
            });
            let w = app.as_weak();
            pick.on_text(move |i, t| {
                let t = t.to_string();
                with(
                    w.clone(),
                    Box::new(move |p| {
                        p.text(i.max(0) as usize, &t);
                        false
                    }),
                )()
            });
            let w = app.as_weak();
            pick.on_space_picked(move |i| {
                with(
                    w.clone(),
                    Box::new(move |p| {
                        p.set_space(i.max(0) as usize);
                        true
                    }),
                )()
            });
            let w = app.as_weak();
            pick.on_ok(move || {
                if let Some(a) = w.upgrade() {
                    a.global::<Pick>().set_open(false);
                }
                if let Some((p, apply)) = COLOR_PICKER.with(|k| k.borrow_mut().take()) {
                    apply(p.stored());
                }
            });
            let w = app.as_weak();
            pick.on_cancel(move || {
                if let Some(a) = w.upgrade() {
                    a.global::<Pick>().set_open(false);
                }
                COLOR_PICKER.with(|k| k.borrow_mut().take());
            });
        }
        let send11 = send.clone();
        app.on_set_flag(move |card, key, on| {
            send11(json!({"cmd": "set_flag", "id": card.as_str(), "key": key.as_str(), "on": on}))
        });
        let key_of11 = key_of.clone();
        app.on_node_key(move |id| key_of11(id).into());
        let (send7, key_of7, models7, weak7) = (send.clone(), key_of.clone(), models.clone(), app.as_weak());
        app.on_block_action(move |key, action| {
            let key = key.to_string();
            match action.as_str() {
                "delete" => {
                    let names: Vec<String> =
                        models7.nodes.iter().filter(|n| key_of7(n.id) == key).map(|n| n.title.to_string()).collect();
                    ask_delete(&weak7, &names, json!({"cmd": "delete", "keys": [key], "links": []}));
                }
                // "Group blocks": the selection, with this block
                "group.make" => {
                    let mut keys: Vec<String> =
                        models7.nodes.iter().filter(|n| n.selected && n.kind != 5).map(|n| key_of7(n.id)).collect();
                    if !keys.contains(&key) {
                        keys.push(key.clone());
                    }
                    send7(json!({"cmd": "group", "keys": keys}));
                }
                a if a.starts_with("wireless.partner.") => {
                    let id = key.strip_suffix("/out").unwrap_or(&key).to_string();
                    let partner = if a.ends_with(".out") { format!("{id}/out") } else { id };
                    for i in 0..models7.nodes.row_count() {
                        let Some(mut n) = models7.nodes.row_data(i) else { continue };
                        let on = key_of7(n.id) == partner;
                        if n.selected != on {
                            n.selected = on;
                            models7.nodes.set_row_data(i, n.clone());
                        }
                        if on && let Some(app) = weak7.upgrade() {
                            // the partner in the middle of the view
                            let z = app.get_canvas_zoom();
                            app.set_canvas_pan_x(app.get_width_() / 2.0 - (n.x + 100.0) * z);
                            app.set_canvas_pan_y(app.get_height_() / 2.0 - (n.y + 34.0) * z);
                        }
                    }
                }
                // a pass-through's inside wire dragged on the block: "lane.wire.<input>.<output or -1>"
                a if a.starts_with("lane.wire.") => {
                    let mut it = a["lane.wire.".len()..].split('.').map(|x| x.parse::<i64>().unwrap_or(-1));
                    let (i, o) = (it.next().unwrap_or(-1), it.next().unwrap_or(-1));
                    if i >= 0 {
                        let out = if o >= 0 { json!(o) } else { Value::Null };
                        send7(json!({"cmd": "lane_map", "id": key, "in": i, "out": out}));
                    }
                }
                _ => send7(json!({"cmd": "node_action", "key": key, "action": action.as_str()})),
            }
        });
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
        let weak_p = app.as_weak();
        app.on_form_pressed(move |key, i| {
            // a plain button's key is its action; a row's button n has its action kept here
            let action = if i < 0 {
                key.to_string()
            } else {
                EDITOR.with(|e| {
                    e.borrow().actions.get(key.as_str()).and_then(|o| o.get(i as usize)).cloned().unwrap_or_default()
                })
            };
            // "ui.…": done here (a file dialog, the program picker), the result goes into the draft
            if let Some(what) = action.strip_prefix("ui.") {
                let Some(a) = weak_p.upgrade() else { return };
                let owner = owner_hwnd(&a);
                if let Some(field) = what.strip_prefix("pick.") {
                    open_picker(&a, field);
                } else if let Some(field) = what.strip_prefix("browse.") {
                    let (field, send, weak) = (field.to_string(), send_p.clone(), weak_p.clone());
                    let (title, filter) = (tr("Choose file"), browse_filter(&field));
                    std::thread::spawn(move || {
                        if let Some(path) = proxydock::filedlg::open(owner, &title, &filter) {
                            let _ = weak
                                .upgrade_in_event_loop(move |_| preview_with(&send, &field, &path.to_string_lossy()));
                        }
                    });
                } else if what == "wg.import" {
                    let (send, weak) = (send_p.clone(), weak_p.clone());
                    let (title, filter) = (tr("Choose file"), tr("WireGuard config (*.conf);;All files (*)"));
                    std::thread::spawn(move || {
                        let Some(path) = proxydock::filedlg::open(owner, &title, &filter) else { return };
                        let read = std::fs::read(&path).map(|b| String::from_utf8_lossy(&b).into_owned());
                        let _ = weak.upgrade_in_event_loop(move |a| match read {
                            Ok(text) => {
                                let msg = EDITOR.with(|e| {
                                    let e = e.borrow();
                                    let mut values = e.values.clone();
                                    values.insert("conf_text".into(), json!(text));
                                    values.insert(
                                        "conf_name".into(),
                                        json!(
                                            path.file_name()
                                                .map(|n| n.to_string_lossy().into_owned())
                                                .unwrap_or_default()
                                        ),
                                    );
                                    json!({"cmd": "edit_action", "id": e.id, "action": "wg.import", "values": values})
                                });
                                send(msg);
                            }
                            Err(err) => show_toast(&a, &format!("{}\n{err}", tr("Cannot import")), false, 6.0),
                        });
                    });
                }
                return;
            }
            let msg = EDITOR.with(|e| {
                let e = e.borrow();
                let mut values = e.values.clone();
                if action == "link.paste" {
                    values.insert("clipboard".into(), json!(clipboard_text()));
                }
                json!({"cmd": "edit_action", "id": e.id, "action": action, "values": values})
            });
            send_p(msg);
        });
        let weak_f = app.as_weak();
        app.on_picker_filter(move || {
            if let Some(a) = weak_f.upgrade() {
                picker_rows(&a);
            }
        });
        let weak_t = app.as_weak();
        app.on_picker_toggle(move |name| {
            PICKER.with(|p| {
                let mut p = p.borrow_mut();
                let name = name.to_string();
                if let Some(i) = p.chosen.iter().position(|n| *n == name) {
                    p.chosen.remove(i);
                } else {
                    p.chosen.push(name);
                }
            });
            if let Some(a) = weak_t.upgrade() {
                picker_rows(&a);
            }
        });
        let (send_d, weak_d) = (send.clone(), app.as_weak());
        app.on_picker_done(move || {
            if let Some(a) = weak_d.upgrade() {
                picker_apply(&a, &send_d, &[]);
            }
        });
        // "Choose a program file...": its file name is added and the picker closes
        let (send_b, weak_b) = (send.clone(), app.as_weak());
        app.on_picker_browse(move || {
            let Some(a) = weak_b.upgrade() else { return };
            let owner = owner_hwnd(&a);
            let (send, weak) = (send_b.clone(), weak_b.clone());
            let (title, filter) = (tr("Choose a program file..."), tr("Programs (*.exe);;All files (*)"));
            std::thread::spawn(move || {
                let Some(path) = proxydock::filedlg::open(owner, &title, &filter) else { return };
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let _ = weak.upgrade_in_event_loop(move |a| {
                    a.set_picker_open(false);
                    picker_apply(&a, &send, &[name]);
                });
            });
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
        let weak = app.as_weak();
        app.on_delete_selected(move || {
            // one message (one undo step): selected links are cut first, then selected blocks deleted (blocks
            // only after the question, as in Python `confirm_delete`)
            let links: Vec<Value> = models
                .links
                .iter()
                .filter(|l| l.selected)
                .map(|l| {
                    let (a, b) = (real_pin(l.start_pin_id), real_pin(l.end_pin_id));
                    json!({"from": key_of(a / 64), "out": (a % 64) / 2, "to": key_of(b / 64), "in": (b % 64) / 2})
                })
                .collect();
            let keys: Vec<String> =
                models.nodes.iter().filter(|n| n.selected && n.kind != 5).map(|n| key_of(n.id)).collect();
            if keys.is_empty() {
                if !links.is_empty() {
                    send(json!({"cmd": "delete", "keys": keys, "links": links}));
                }
                return;
            }
            let names: Vec<String> = models.nodes.iter().filter(|n| n.selected).map(|n| n.title.to_string()).collect();
            ask_delete(&weak, &names, json!({"cmd": "delete", "keys": keys, "links": links}));
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
            let keys: Vec<String> = models2
                .nodes
                .iter()
                .filter(|n| n.selected && n.kind != 5)
                .map(|n| key_of2(n.id))
                .filter(|k| k != "pc")
                .collect();
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
            a.set_rename_title(SharedString::new());
            a.set_rename_open(true);
        });
        // canvas groups: make one of the selection, and the selected group's bar
        let (models4, key_of4, send4) = (models.clone(), key_of.clone(), send.clone());
        app.on_group_make(move || {
            let keys: Vec<String> =
                models4.nodes.iter().filter(|n| n.selected && n.kind != 5).map(|n| key_of4(n.id)).collect();
            send4(json!({"cmd": "group", "keys": keys}));
        });
        let weak = app.as_weak();
        // a wireless pair's number: the rename box with the number
        let (models9, key_of9, weak9) = (models.clone(), key_of.clone(), weak.clone());
        app.on_edit_number(move |id| {
            let Some(a) = weak9.upgrade() else { return };
            let Some(n) = models9.nodes.iter().find(|n| n.id == id) else { return };
            RENAME_KEY.with(|k| *k.borrow_mut() = format!("number:{}", key_of9(id)));
            a.set_rename_text(n.detail.trim_start_matches('#').into());
            a.set_rename_title(tr("Pair number").into());
            a.set_rename_open(true);
        });
        let weak5 = weak.clone();
        app.on_group_rename(move || {
            let Some(a) = weak5.upgrade() else { return };
            let gid = a.get_group_sel();
            let Some(row) = a.get_groups().iter().find(|g| g.id == gid) else { return };
            RENAME_KEY.with(|k| *k.borrow_mut() = format!("group:{}", group_key(gid)));
            a.set_rename_text(row.title.clone());
            a.set_rename_title(SharedString::new());
            a.set_rename_open(true);
        });
        let send6 = send.clone();
        let weak6 = weak.clone();
        app.on_group_collapse(move || {
            let Some(a) = weak6.upgrade() else { return };
            send6(json!({"cmd": "group_collapse", "id": group_key(a.get_group_sel()), "collapsed": !a.get_group_sel_collapsed()}));
        });
        let send7 = send.clone();
        let weak7 = weak.clone();
        app.on_ungroup(move || {
            let Some(a) = weak7.upgrade() else { return };
            send7(json!({"cmd": "ungroup", "id": group_key(a.get_group_sel())}));
        });
        let weak8 = weak.clone();
        app.on_group_color(move |x, y| {
            let Some(a) = weak8.upgrade() else { return };
            let gid = a.get_group_sel();
            let Some(row) = a.get_groups().iter().find(|g| g.id == gid) else { return };
            let own = row.color.alpha() > 0;
            let hex = format!("#{:02x}{:02x}{:02x}", row.color.red(), row.color.green(), row.color.blue());
            PALETTE_FOR.with(|p| *p.borrow_mut() = (format!("group:{}", group_key(gid)), String::new(), hex.clone()));
            a.set_palette_title(row.title.clone());
            a.set_palette_current(if own {
                PALETTE.iter().position(|(h, _)| *h == hex).map_or(-1, |i| i as i32)
            } else {
                -1
            });
            a.set_palette_auto_current(!own);
            a.set_t_palette_auto(tr("Default").into());
            a.set_palette_x(x);
            a.set_palette_y(y);
            a.set_palette_open(true);
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
        // the theme colour: shown at once (the apply bar saves it, Cancel puts the saved one back)
        let presets = proxydock_core::theme::PRESETS;
        st.set_presets(ModelRc::new(VecModel::from(presets.iter().map(|c| hex(c)).collect::<Vec<_>>())));
        st.set_preset_values(ModelRc::new(VecModel::from(
            presets.iter().map(|c| SharedString::from(*c)).collect::<Vec<_>>(),
        )));
        let weak = app.as_weak();
        st.on_preview_theme_color(move |v| {
            if let Some(a) = weak.upgrade() {
                apply_theme_color(&a, &v);
            }
        });
        let weak = app.as_weak();
        st.on_pick_theme_color(move || {
            let Some(a) = weak.upgrade() else { return };
            let st = a.global::<Settings>();
            let cur = match st.get_theme_color().as_str() {
                "" => proxydock_core::theme::APP_SEED.to_string(),
                "system" => system_accent().unwrap_or_else(|| proxydock_core::theme::APP_SEED.to_string()),
                c => c.to_string(),
            };
            let w = a.as_weak();
            let apply = move |c: String| {
                if let Some(a) = w.upgrade() {
                    a.global::<Settings>().set_theme_color(c.as_str().into());
                    apply_theme_color(&a, &c);
                }
            };
            open_color_picker(&a, &cur, &tr("Theme colour"), Rc::new(apply));
        });
        // Windows' accent changed: the colours follow it when the theme colour is "system"
        let weak = app.as_weak();
        ON_ACCENT.with(|c| {
            *c.borrow_mut() = Some(Rc::new(move || {
                if let Some(a) = weak.upgrade() {
                    let v = THEME_COLOR.with(|t| t.borrow().clone());
                    a.global::<Settings>().set_system_accent(hex(
                        &system_accent().unwrap_or_else(|| proxydock_core::theme::APP_SEED.to_string())
                    ));
                    if v == "system" {
                        apply_theme_color(&a, &v);
                    }
                }
            }))
        });
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
                "text_size": st.get_text_size(), "theme_color": st.get_theme_color().as_str(),
                "ui_font": FONTS.with(|f| f.borrow().ui.get((st.get_ui_font() - 1).max(-1) as usize).cloned().filter(|_| st.get_ui_font() > 0).unwrap_or_default()),
                "mono_font": FONTS.with(|f| f.borrow().mono.get((st.get_mono_font() - 1).max(-1) as usize).cloned().filter(|_| st.get_mono_font() > 0).unwrap_or_default()),
            }}));
        });
        let weak = app.as_weak();
        st.on_open_log_dir(move || {
            let Some(a) = weak.upgrade() else { return };
            let dir = a.global::<Settings>().get_log_dir().to_string();
            let _ = std::fs::create_dir_all(&dir);
            if let Err(e) = open_with_system(&dir) {
                show_toast(&a, &format!("{}: {e}", tr("Cannot open the folder")), false, 5.0);
            }
        });
    }
    {
        // "Quit with the X button": the whole app quits, else only this window closes (the tray keeps running)
        let send_q = send.clone();
        let weak = app.as_weak();
        app.window().on_close_requested(move || {
            if let Some(a) = weak.upgrade() {
                save_window_state(&a);
            }
            if CLOSE_EXITS.with(|c| c.get()) {
                send_q(json!({"cmd": "quit"}));
            }
            slint::CloseRequestResponse::HideWindow
        });
        // the history: pages as they change, back / forward from the hook
        app.on_page_changed(page_changed);
        HISTORY.with(|h| h.borrow_mut().current = app.get_page());
        let weak = app.as_weak();
        NAV_CB.with(|c| {
            *c.borrow_mut() = Some(Rc::new(move |forward: bool| {
                if let Some(a) = weak.upgrade() {
                    if forward { go_forward(&a) } else { go_back(&a) }
                }
            }))
        });
        // the toast's button: "Restart now" (the window opens again with the new app scale)
        let send_t = send.clone();
        let weak_t = app.as_weak();
        app.on_toast_clicked(move || {
            let action = TOAST_ACTION.with(|t| t.borrow().clone());
            if let Some(a) = weak_t.upgrade() {
                a.set_toast(SharedString::new());
                a.set_toast_action(SharedString::new());
            }
            if action == "restart" {
                send_t(json!({"cmd": "restart", "app": false}));
            }
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
        // a column's header clicked: by it upwards, again downwards; the # column goes back to the sites' order
        let weak = app.as_weak();
        c.on_sort(move |col| {
            CONN.with(|s| {
                let mut s = s.borrow_mut();
                if col == 0 || s.sort_col != col {
                    s.sort_col = col;
                    s.sort_desc = false;
                } else {
                    s.sort_desc = !s.sort_desc;
                }
            });
            if let Some(a) = weak.upgrade() {
                conn_rows(&a);
            }
        });
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
            if url.starts_with("https://")
                && let (Err(e), Some(a)) = (open_with_system(&url), weak.upgrade())
            {
                show_toast(&a, &e.to_string(), false, 5.0);
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
        u.on_act(move || match UPDATE_PAGE.with(|p| p.borrow().clone()) {
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

    // the core's events, read on a thread and applied on the UI thread in batches: everything that arrived
    // since the last update is applied at once (a burst of log lines is one model change and one redraw)
    let weak = app.as_weak();
    std::thread::spawn(move || {
        let queue: Arc<Mutex<Vec<Value>>> = Arc::default();
        for v in ipc::messages(stream) {
            let first = {
                let mut q = queue.lock().unwrap();
                q.push(v);
                q.len() == 1
            };
            if !first {
                // an update is already on its way and takes this one too
                continue;
            }
            let weak = weak.clone();
            let queue = queue.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = weak.upgrade() else { return };
                let batch = std::mem::take(&mut *queue.lock().unwrap());
                for v in batch {
                    MODELS.with(|m| {
                        let Some(m) = m.borrow().clone() else { return };
                        match v["ev"].as_str().unwrap_or("") {
                            "hello" => {
                                if v["add_menu"].is_object() {
                                    let t = app.global::<CanvasText>();
                                    t.set_inbounds(type_leaves(&v["add_menu"]["inbounds"]));
                                    t.set_outbounds(type_leaves(&v["add_menu"]["outbounds"]));
                                    let templates: Vec<MenuLeaf> = v["add_menu"]["templates"]
                                        .as_array()
                                        .into_iter()
                                        .flatten()
                                        .enumerate()
                                        .map(|(i, x)| MenuLeaf {
                                            action: i.to_string().into(),
                                            title: format!(
                                                "{}  —  {}",
                                                x[0].as_str().unwrap_or(""),
                                                x[1].as_str().unwrap_or("")
                                            )
                                            .into(),
                                        })
                                        .collect();
                                    t.set_templates(ModelRc::new(VecModel::from(templates)));
                                }
                                set_language(v["language"].as_str().unwrap_or("en"));
                                apply_fonts(&app, v["language"].as_str().unwrap_or("en"));
                                labels(&app);
                                m.cards.set_vec(
                                    v["cards"].as_array().into_iter().flatten().map(card_row).collect::<Vec<_>>(),
                                );
                                log_sources(
                                    &app,
                                    &m.cards.iter().map(|c| (c.id.to_string(), c.name.to_string())).collect::<Vec<_>>(),
                                );
                                m.add_logs(v["logs"].as_array().map(Vec::as_slice).unwrap_or(&[]));
                                m.set_stats(&v["stats"]);
                                show_latency(&app, &m, &v["latency"]);
                                m.set_canvas(&v["canvas"]);
                                app.set_pc_state(v["canvas"]["pc_status"][0].as_str().unwrap_or("off").into());
                                app.set_pc_text(v["canvas"]["pc_status"][1].as_str().unwrap_or("").into());
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
                                if cfg!(debug_assertions)
                                    && let Ok(id) = std::env::var("PROXYDOCK_EDIT")
                                {
                                    EDIT_REQUEST.with(|r| *r.borrow_mut() = Some(id));
                                }
                            }
                            "flow" => {
                                m.set_flow(v["links"].as_array().map(Vec::as_slice).unwrap_or(&[]));
                                start_flow_dots(&app);
                            }
                            "graph" => {
                                m.cards.set_vec(
                                    v["cards"].as_array().into_iter().flatten().map(card_row).collect::<Vec<_>>(),
                                );
                                m.set_canvas(&v["canvas"]);
                                app.set_pc_state(v["canvas"]["pc_status"][0].as_str().unwrap_or("off").into());
                                app.set_pc_text(v["canvas"]["pc_status"][1].as_str().unwrap_or("").into());
                                log_sources(
                                    &app,
                                    &m.cards.iter().map(|c| (c.id.to_string(), c.name.to_string())).collect::<Vec<_>>(),
                                );
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
                                });
                                let rows = field_rows(v["fields"].as_array().map(Vec::as_slice).unwrap_or(&[]));
                                app.set_form_fields(ModelRc::new(VecModel::from(rows)));
                                app.set_form_title(v["title"].as_str().unwrap_or("").into());
                                app.set_form_errors(ModelRc::default());
                                app.set_page(3);
                            }
                            "notice" => {
                                let (title, text) =
                                    (v["title"].as_str().unwrap_or(""), v["text"].as_str().unwrap_or(""));
                                // a short result (port free, connection works...): a snackbar at the bottom; a
                                // success says just what it found, a failure also what failed
                                let ok = v["ok"] == json!(true);
                                let msg = match (text.is_empty(), ok) {
                                    (true, _) => title.to_string(),
                                    (false, true) => text.to_string(),
                                    (false, false) => format!("{title}\n{text}"),
                                };
                                show_toast(&app, &msg, ok, if ok { 4.0 } else { 7.0 });
                            }
                            // a file to save (a WireGuard device's .conf): Windows' save dialog, then written
                            "save_file" => {
                                let (name, text) = (
                                    v["name"].as_str().unwrap_or("").to_string(),
                                    v["text"].as_str().unwrap_or("").to_string(),
                                );
                                let (owner, weak) = (owner_hwnd(&app), app.as_weak());
                                std::thread::spawn(move || {
                                    let Some(path) =
                                        proxydock::filedlg::save(owner, &tr("Save .conf"), "WireGuard (*.conf)", &name)
                                    else {
                                        return;
                                    };
                                    let result = std::fs::write(&path, text);
                                    let _ = weak.upgrade_in_event_loop(move |a| match result {
                                        Ok(()) => {
                                            show_toast(&a, &format!("{}\n{}", tr("Saved"), path.display()), true, 3.0)
                                        }
                                        Err(e) => show_toast(&a, &e.to_string(), false, 6.0),
                                    });
                                });
                            }
                            "pick_programs" => open_picker(&app, v["key"].as_str().unwrap_or("")),
                            "warn" => show_toast(&app, v["message"].as_str().unwrap_or(""), false, 6.0),
                            // the tray's "Close window", or "Restart now": keep the place, end this process only
                            "close_window" => {
                                save_window_state(&app);
                                let _ = app.hide();
                                let _ = slint::quit_event_loop();
                            }
                            "restart_needed" => show_toast_action(
                                &app,
                                v["message"].as_str().unwrap_or(""),
                                &tr("Restart now"),
                                "restart",
                                15.0,
                            ),
                            // text to hand to another program (a WireGuard device's config): onto the clipboard
                            "show_text" => {
                                if set_clipboard_text(v["text"].as_str().unwrap_or("")) {
                                    show_toast(&app, &tr("Copied"), true, 2.5);
                                } else {
                                    show_toast(&app, &tr("Cannot use the clipboard"), false, 5.0);
                                }
                            }
                            "form_errors" => {
                                let errs: Vec<SharedString> = v["errors"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(Value::as_str)
                                    .map(SharedString::from)
                                    .collect();
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
                            "route" => show_route(&app, &m, &v),
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
                                let (done, total) =
                                    (v["done"].as_f64().unwrap_or(0.0), v["total"].as_f64().unwrap_or(0.0));
                                let u = app.global::<Upd>();
                                u.set_progress(if total > 0.0 { (done / total) as f32 } else { 0.0 });
                                let mb = |n: f64| format!("{:.1} MB", n / (1024.0 * 1024.0));
                                u.set_progress_text(
                                    if total > 0.0 { format!("{} / {}", mb(done), mb(total)) } else { mb(done) }.into(),
                                );
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
                }
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
        // (winit makes the native window a moment after the loop starts: try until it is there, for 2 s)
        let weak = app.as_weak();
        let tries = std::cell::Cell::new(0);
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(50), move || {
            tries.set(tries.get() + 1);
            let Some(a) = weak.upgrade() else { return };
            if has_native_window(&a) || tries.get() > 40 {
                apply_title_bar(&a);
                #[cfg(windows)]
                caption::install(&a);
                restore_window_state(&a);
                #[cfg(windows)]
                nav_hook::install(owner_hwnd(&a));
                FIRST_PAINT.with(|t| t.stop());
            }
        });
        FIRST_PAINT.with(|t| *t.borrow_mut() = Some(timer));
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
const LINE_COLORS: [&str; 10] =
    ["#4f8ef7", "#e8912d", "#57ab5a", "#b76de0", "#e5534b", "#39c5bb", "#d9b92f", "#e26ba5", "#7c6cf2", "#9aa0a6"];

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
        .map(|x| {
            (
                x[0].as_str().unwrap_or("").to_string(),
                x[1].as_str().unwrap_or("").to_string(),
                x[2].as_str().unwrap_or("").to_string(),
            )
        })
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
        s.hist = v["cards"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, h)| {
                (
                    k.clone(),
                    h.as_array()
                        .into_iter()
                        .flatten()
                        .map(|p| (p[0].as_f64().unwrap_or(0.0), p[1].as_f64().unwrap_or(0.0)))
                        .collect(),
                )
            })
            .collect();
        s.exits = v["exits"]
            .as_object()
            .into_iter()
            .flatten()
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
            push_hist(
                s.hist.entry(id.clone()).or_default(),
                (r[0].as_f64().unwrap_or(0.0), r[1].as_f64().unwrap_or(0.0)),
            );
            seen.insert(id.clone());
        }
        s.hist.retain(|k, _| seen.contains(k));
        let rates: HashMap<String, f64> = point["exits"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, r)| (k.clone(), r.as_f64().unwrap_or(0.0)))
            .collect();
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
        if i > 0 && (s.len() - i).is_multiple_of(3) {
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
        let tile =
            |t: &str, v: String, sub: String| StatTileData { title: tr(t).into(), value: v.into(), sub: sub.into() };
        let tiles = vec![
            tile("Active TCP connections", n("tcp").to_string(), String::new()),
            tile("Active UDP sessions", n("udp").to_string(), String::new()),
            tile("Total connections", thousands(n("total")), String::new()),
            tile("Download", rate(down), total(n("down"))),
            tile("Upload", rate(up), total(n("up"))),
            tile(
                "Blocked / failed",
                format!("{} / {}", thousands(n("blocked")), thousands(n("failed"))),
                String::new(),
            ),
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
                let (d, area) =
                    chart_path(values, top, st.get_chart_w().max(1.0) as f64, st.get_chart_h().max(1.0) as f64);
                ChartLine {
                    name: name.as_str().into(),
                    color: *color,
                    commands: d.into(),
                    area: if *fill { area.into() } else { SharedString::new() },
                }
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
    let modes: Vec<SharedString> =
        ["Combined", "Per proxy", "Per exit"].iter().map(|m| SharedString::from(tr(m))).collect();
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
    /// the column the table is sorted by (0 = the sites' order) and whether downwards
    sort_col: i32,
    sort_desc: bool,
}

/// a waterfall-less row (not tested yet): every time unknown
fn conn_untimed(r: &mut ConnRow) {
    r.conn_ms = -1.0;
    r.total_ms = -1.0;
    r.w_start = -1.0;
    r.w_tcp = -1.0;
    r.w_connect = -1.0;
    r.w_tls = -1.0;
    r.w_first = -1.0;
    r.w_total = -1.0;
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
    let cols: Vec<SharedString> =
        ["Region", "Site", "Address", "Result", "Connect", "All"].iter().map(|x| SharedString::from(tr(x))).collect();
    c.set_columns(ModelRc::new(VecModel::from(cols)));
    c.set_t_waterfall(tr("Waterfall").into());
    let phases: Vec<SharedString> =
        ["Proxy", "Connect", "TLS", "Waiting", "Response"].iter().map(|x| SharedString::from(tr(x))).collect();
    c.set_phases(ModelRc::new(VecModel::from(phases)));
}

/// the table: the rows in the chosen order (an unknown number last either way), and the waterfall's scale
fn conn_rows(app: &App) {
    CONN.with(|s| {
        let s = s.borrow();
        let mut rows = s.rows.clone();
        let (col, desc) = (s.sort_col, s.sort_desc);
        let num = |r: &ConnRow| -> f32 {
            match col {
                5 => r.conn_ms,
                6 => r.total_ms,
                7 => r.w_start,
                _ => 0.0,
            }
        };
        rows.sort_by(|a, b| {
            use std::cmp::Ordering;
            let ord = match col {
                1 => a.region.to_lowercase().cmp(&b.region.to_lowercase()),
                2 => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                3 => a.host.cmp(&b.host),
                // answered first, then failed, then not tested; then by the text
                4 => {
                    let rank = |r: &ConnRow| match r.tone {
                        1 => 0,
                        2 => 1,
                        _ => 2,
                    };
                    rank(a).cmp(&rank(b)).then_with(|| a.result.cmp(&b.result))
                }
                5..=7 => {
                    let (x, y) = (num(a), num(b));
                    // unknown times stay at the bottom whichever way
                    match (x < 0.0, y < 0.0) {
                        (true, true) => Ordering::Equal,
                        (true, false) => return Ordering::Greater,
                        (false, true) => return Ordering::Less,
                        _ => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
                    }
                }
                _ => Ordering::Equal,
            };
            let ord = if desc { ord.reverse() } else { ord };
            ord.then_with(|| a.index.cmp(&b.index))
        });
        let w_max = s.rows.iter().map(|r| r.w_start + r.w_total).fold(0.0f32, f32::max);
        let c = app.global::<Conn>();
        c.set_rows(ModelRc::new(VecModel::from(rows)));
        c.set_sort_col(col);
        c.set_sort_desc(desc);
        c.set_w_max(w_max.max(1.0));
    });
}

/// the sites (results cleared) from the core's "conn"
fn conn_sites(app: &App, v: &Value) {
    CONN.with(|s| {
        let mut s = s.borrow_mut();
        s.rows = v["sites"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, x)| ConnRow {
                index: i as i32 + 1,
                conn_ms: -1.0,
                total_ms: -1.0,
                w_start: -1.0,
                w_tcp: -1.0,
                w_connect: -1.0,
                w_tls: -1.0,
                w_first: -1.0,
                w_total: -1.0,
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
    let running: Vec<(String, String)> =
        m.cards.iter().filter(|r| r.state == "running").map(|r| (r.id.to_string(), r.name.to_string())).collect();
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
                    conn_untimed(r);
                }
            });
            c.set_summary(
                tr("Testing '{name}'… 0 / {len}")
                    .replace("{name}", v["name"].as_str().unwrap_or(""))
                    .replace("{len}", &total.to_string())
                    .into(),
            );
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
                    let t = &v["timing"];
                    let ms = |k: &str| t[k].as_f64().map_or(-1.0, |v| v as f32);
                    // the phases' ends are from the site's own start; the start is from the test's
                    x.w_start = ms("start");
                    x.w_tcp = ms("tcp");
                    x.w_connect = ms("connect");
                    x.w_tls = ms("tls");
                    x.w_first = ms("first_byte");
                    x.w_total = ms("total");
                    x.conn_ms = x.w_connect;
                    x.total_ms = x.w_total;
                }
                (s.done, s.rows.len())
            });
            c.set_summary(
                tr("Testing… {len} / {len2}")
                    .replace("{len}", &done.to_string())
                    .replace("{len2}", &total.to_string())
                    .into(),
            );
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
        "ko" => include_str!("../../../../locales/help/ko.md"),
        "ja" => include_str!("../../../../locales/help/ja.md"),
        "zh_CN" => include_str!("../../../../locales/help/zh_CN.md"),
        "zh_TW" => include_str!("../../../../locales/help/zh_TW.md"),
        "de" => include_str!("../../../../locales/help/de.md"),
        "fr" => include_str!("../../../../locales/help/fr.md"),
        "es" => include_str!("../../../../locales/help/es.md"),
        _ => include_str!("../../../../locales/help/en.md"),
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

/// Open a folder or a web address with the system (Explorer / Finder / the file manager, the default browser)
fn open_with_system(target: &str) -> std::io::Result<()> {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(target).spawn().map(|_| ())
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
    i.set_lic_desc(
        t("Fonts and open source components included in ProxyDock (also in the {LICENSE_DIR} folder).")
            .replace("{LICENSE_DIR}", "licenses")
            .into(),
    );
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
        None => vec![
            EngineRow { name: "sing-box".into(), text: tr("Checking…").into() },
            EngineRow { name: "Hysteria2".into(), text: tr("Checking…").into() },
        ],
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
    u.set_title(
        tr("{APP_NAME} {version} is available").replace("{APP_NAME}", "ProxyDock").replace("{version}", version).into(),
    );
    u.set_installed(
        tr("Installed version: {version}").replace("{version}", v["current"].as_str().unwrap_or("")).into(),
    );
    let notes =
        rel["notes"].as_str().filter(|n| !n.is_empty()).map(String::from).unwrap_or_else(|| tr("No release notes."));
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
        tr("Download and install")
            + &if size > 0.0 { format!(" ({:.1} MB)", size / (1024.0 * 1024.0)) } else { String::new() }
    } else {
        tr("Open the release page")
    };
    u.set_action(action.into());
    u.set_file(rel["asset"].as_str().unwrap_or("").into());
    let page = rel["url"].as_str().filter(|x| !x.is_empty()).unwrap_or(proxydock_core::update::RELEASES_URL);
    UPDATE_PAGE.with(|p| *p.borrow_mut() = (!installs).then(|| page.to_string()));
}

// --- the window's own title bar (Windows 11): taller, in the app's M3 colours, with Windows' snapping kept ---
//
// Windows draws no caption (WM_NCCALCSIZE gives its height to the client area) but keeps the borders, the
// shadow and resizing. WM_NCHITTEST tells Windows where the bar and its buttons are: the bar moves the window
// (drag, double-click, Aero Snap), and HTMAXBUTTON over the maximise button brings Windows 11's snap layouts.
// Clicks on the buttons come as non-client messages, handled here; their hover and pressed looks go to the
// window (caption-hover / caption-pressed). Windows 10 keeps its own bar (resizing a frame drawn this way
// shakes there).
#[cfg(windows)]
mod caption {
    use super::App;
    use slint::ComponentHandle;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::Controls::MARGINS;
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClientRect, HTCAPTION, HTCLIENT, HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HTTOP, HTTOPLEFT, HTTOPRIGHT, IsZoomed,
        NCCALCSIZE_PARAMS, PostMessageW, SM_CXPADDEDBORDER, SM_CYFRAME, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE,
        SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, ShowWindow, WM_ACTIVATE, WM_CLOSE,
        WM_MOUSEMOVE, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP,
        WM_NCMOUSEMOVE, WM_SIZE,
    };

    /// the bar's height and each button's width (logical px, as in app.slint)
    pub const HEIGHT: f32 = 48.0;
    const BUTTON_W: f32 = 46.0;
    const WM_NCMOUSELEAVE: u32 = 0x02A2;
    const SIZE_MAXIMIZED: usize = 2;

    thread_local! {
        static APP: std::cell::RefCell<Option<slint::Weak<App>>> = const { std::cell::RefCell::new(None) };
        static PRESSED: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    }

    /// Windows 11 (build 22000 or later)
    fn windows_11() -> bool {
        use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
        let key: Vec<u16> = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\0".encode_utf16().collect();
        let name: Vec<u16> = "CurrentBuildNumber\0".encode_utf16().collect();
        let mut buf = [0u16; 32];
        let mut size = (buf.len() * 2) as u32;
        let ok = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        } == 0;
        let text = String::from_utf16_lossy(&buf[..buf.iter().position(|c| *c == 0).unwrap_or(0)]);
        ok && text.parse::<u32>().is_ok_and(|b| b >= 22000)
    }

    fn hwnd_of(app: &App) -> Option<HWND> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = app.window().window_handle();
        let raw = handle.window_handle().ok()?.as_raw();
        let RawWindowHandle::Win32(h) = raw else { return None };
        Some(h.hwnd.get() as HWND)
    }

    /// Take over the caption (once, after the window exists). False: Windows keeps its own bar.
    pub fn install(app: &App) -> bool {
        if std::env::var_os("PROXYDOCK_NATIVE_CAPTION").is_some() || !windows_11() {
            return false;
        }
        let Some(hwnd) = hwnd_of(app) else { return false };
        APP.with(|a| *a.borrow_mut() = Some(app.as_weak()));
        unsafe {
            if SetWindowSubclass(hwnd, Some(proc_), 1, 0) == 0 {
                return false;
            }
            // the shadow and the rounded corners stay with a frame extended one pixel into the client area
            let margins = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 };
            DwmExtendFrameIntoClientArea(hwnd, &margins);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
            );
        }
        app.set_caption_h(HEIGHT);
        app.set_win_maximized(unsafe { IsZoomed(hwnd) } != 0);
        true
    }

    fn with_app(f: impl FnOnce(&App)) {
        APP.with(|a| {
            if let Some(app) = a.borrow().as_ref().and_then(|w| w.upgrade()) {
                f(&app);
            }
        });
    }

    /// the resize border's thickness (physical px)
    fn frame(hwnd: HWND) -> i32 {
        unsafe {
            let dpi = GetDpiForWindow(hwnd);
            GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
        }
    }

    /// 1 minimise, 2 maximise, 3 close, 0 another place
    fn button_of(hit: u32) -> i32 {
        match hit {
            HTMINBUTTON => 1,
            HTMAXBUTTON => 2,
            HTCLOSE => 3,
            _ => 0,
        }
    }

    fn set_hover(b: i32) {
        with_app(|a| {
            if a.get_caption_hover() != b {
                a.set_caption_hover(b);
            }
        });
    }

    fn set_pressed(b: i32) {
        PRESSED.with(|p| p.set(b));
        with_app(|a| {
            if a.get_caption_pressed() != b {
                a.set_caption_pressed(b);
            }
        });
    }

    unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
        unsafe {
            match msg {
                // no caption: the client area reaches the window's top (a maximised window's top border is off the
                // screen: keep the client below it)
                WM_NCCALCSIZE if wp != 0 => {
                    let params = lp as *mut NCCALCSIZE_PARAMS;
                    let top = (*params).rgrc[0].top;
                    let r = DefSubclassProc(hwnd, msg, wp, lp);
                    (*params).rgrc[0].top = top + if IsZoomed(hwnd) != 0 { frame(hwnd) } else { 0 };
                    r
                }
                WM_NCHITTEST => {
                    let r = DefSubclassProc(hwnd, msg, wp, lp);
                    if r != HTCLIENT as LRESULT {
                        return r;
                    }
                    let mut pt = POINT { x: (lp & 0xffff) as i16 as i32, y: ((lp >> 16) & 0xffff) as i16 as i32 };
                    ScreenToClient(hwnd, &mut pt);
                    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                    GetClientRect(hwnd, &mut rc);
                    let mut scale = 1.0f32;
                    with_app(|a| scale = a.window().scale_factor());
                    let border = frame(hwnd);
                    if IsZoomed(hwnd) == 0 && pt.y < border {
                        return if pt.x < border * 2 {
                            HTTOPLEFT
                        } else if pt.x >= rc.right - border * 2 {
                            HTTOPRIGHT
                        } else {
                            HTTOP
                        } as LRESULT;
                    }
                    // the page title bar's empty part is a caption too (moving, double click, snapping)
                    let mut band = (0.0f32, 0.0f32, 0.0f32);
                    with_app(|a| band = (a.get_bar_h(), a.get_drag_left(), a.get_drag_right()));
                    let (x, y) = (pt.x as f32 / scale, pt.y as f32 / scale);
                    if y >= HEIGHT && y < HEIGHT + band.0 && x >= band.1 && x < band.2 {
                        return HTCAPTION as LRESULT;
                    }
                    if (pt.y as f32) < HEIGHT * scale {
                        let bw = BUTTON_W * scale;
                        let from_right = (rc.right - pt.x) as f32;
                        let hit = if from_right <= bw {
                            HTCLOSE
                        } else if from_right <= bw * 2.0 {
                            HTMAXBUTTON
                        } else if from_right <= bw * 3.0 {
                            HTMINBUTTON
                        } else {
                            HTCAPTION
                        };
                        return hit as LRESULT;
                    }
                    HTCLIENT as LRESULT
                }
                WM_NCMOUSEMOVE => {
                    let b = button_of(wp as u32);
                    set_hover(b);
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE | TME_NONCLIENT,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    TrackMouseEvent(&mut t);
                    if b != 0 { 0 } else { DefSubclassProc(hwnd, msg, wp, lp) }
                }
                WM_NCMOUSELEAVE => {
                    set_hover(0);
                    set_pressed(0);
                    DefSubclassProc(hwnd, msg, wp, lp)
                }
                WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if button_of(wp as u32) != 0 => {
                    set_pressed(button_of(wp as u32));
                    0
                }
                WM_NCLBUTTONUP if button_of(wp as u32) != 0 => {
                    let b = button_of(wp as u32);
                    let was = PRESSED.with(|p| p.get());
                    set_pressed(0);
                    if was == b {
                        match b {
                            1 => {
                                ShowWindow(hwnd, SW_MINIMIZE);
                            }
                            2 => {
                                ShowWindow(hwnd, if IsZoomed(hwnd) != 0 { SW_RESTORE } else { SW_MAXIMIZE });
                            }
                            _ => {
                                PostMessageW(hwnd, WM_CLOSE, 0, 0);
                            }
                        }
                    }
                    0
                }
                WM_MOUSEMOVE => {
                    set_hover(0);
                    DefSubclassProc(hwnd, msg, wp, lp)
                }
                WM_SIZE => {
                    let max = wp == SIZE_MAXIMIZED;
                    with_app(|a| {
                        if a.get_win_maximized() != max {
                            a.set_win_maximized(max);
                        }
                    });
                    DefSubclassProc(hwnd, msg, wp, lp)
                }
                WM_ACTIVATE => {
                    let active = (wp & 0xffff) != 0;
                    with_app(|a| a.set_window_active(active));
                    DefSubclassProc(hwnd, msg, wp, lp)
                }
                _ => DefSubclassProc(hwnd, msg, wp, lp),
            }
        }
    }
}
