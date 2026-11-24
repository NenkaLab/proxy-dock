// ProxyDock UI prototype in Slint: canvas, live log, Korean input, tray.
// Closing the window drops the whole UI (only the tray and the data models stay), so the RAM in the
// tray can be compared with keeping a Qt window alive.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use slint::{
    CloseRequestResponse, Color, ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString,
    Timer, TimerMode, VecModel,
};
use slint_node_editor::{
    wire_node_editor, wire_selection, GraphLogic, LinkData, LinkPath, MovableNode, NodeEditorSetup,
};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
    time::Duration,
};

slint::include_modules!();

const RATES: [u32; 5] = [0, 100, 500, 2000, 10000];
const LOG_CAP: usize = 5000;

impl MovableNode for NodeData {
    fn id(&self) -> i32 { self.id }
    fn x(&self) -> f32 { self.x }
    fn y(&self) -> f32 { self.y }
    fn selected(&self) -> bool { self.selected }
    fn set_x(&mut self, x: f32) { self.x = x; }
    fn set_y(&mut self, y: f32) { self.y = y; }
}

/// Everything that outlives the window.
struct Shared {
    nodes: Rc<VecModel<NodeData>>,
    links: Rc<VecModel<LinkData>>,
    logs: Rc<VecModel<SharedString>>,
    log_total: Cell<u64>,
    log_rate: Cell<u32>,
    log_debt: Cell<f64>,
    log_last_second: Cell<u32>,
    log_lines_per_s: Cell<u32>,
    rng: Cell<u64>,
    next_node: Cell<i32>,
    next_link: Cell<i32>,
    window: RefCell<Option<App>>,
    frames: Rc<Cell<u32>>,
    startup_ms: Rc<Cell<f64>>,
    builds: Cell<u32>,
    ui_only: bool,
}

impl Shared {
    fn rand(&self, n: u64) -> u64 {
        let x = self.rng.get().wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.rng.set(x);
        (x >> 33) % n
    }

    fn node(&self, kind: i32, x: f32, y: f32) -> NodeData {
        let id = self.next_node.get();
        self.next_node.set(id + 1);
        let (title, detail) = match kind {
            0 => (format!("인바운드 {id}"), format!("mixed · 127.0.0.1:{}", 7890 + id)),
            1 => (format!("라우터 {id}"), "규칙별 출구 · 위에서부터 먼저".to_string()),
            3 => (format!("릴레이 {id}"), "레인별로 통과".to_string()),
            _ => (format!("아웃바운드 {id}"), format!("hysteria2 · hk-{id}.example.net:443")),
        };
        let (ins, outs) = match kind {
            0 => (0, 1),
            2 => (1, 0),
            _ => (1, 1),
        };
        NodeData {
            id, title: title.into(), detail: detail.into(), kind, x, y, selected: false, ins, outs,
            in_labels: ModelRc::default(), out_labels: ModelRc::default(), wires: ModelRc::default(),
            in_wired: ModelRc::default(),
        }
    }

    /// a router with one output lane per rule, plus the default
    fn router(&self, x: f32, y: f32, rules: &[&str]) -> NodeData {
        let mut n = self.node(1, x, y);
        n.outs = rules.len() as i32;
        n.out_labels = strings(rules);
        n
    }

    /// a pass-through with lanes on both sides and the wiring between them
    fn relay(&self, x: f32, y: f32, ins: &[&str], outs: &[&str], wires: &[(i32, i32)]) -> NodeData {
        let mut n = self.node(3, x, y);
        n.ins = ins.len() as i32;
        n.outs = outs.len() as i32;
        n.in_labels = strings(ins);
        n.out_labels = strings(outs);
        set_wires(&mut n, wires.iter().map(|&(i, o)| Wire { i, o }).collect());
        n
    }

    /// output lane `out_lane` of `out_node` -> input lane `in_lane` of `in_node`
    fn link(&self, out_node: i32, out_lane: i32, in_node: i32, in_lane: i32) -> LinkData {
        let id = self.next_link.get();
        self.next_link.set(id + 1);
        LinkData::new(id, pin_out(out_node, out_lane), pin_in(in_node, in_lane), Color::from_rgb_u8(90, 160, 240))
    }

    fn seed(&self) {
        let ins: Vec<NodeData> = (0..3).map(|i| self.node(0, 20.0, 40.0 + i as f32 * 120.0)).collect();
        let router = self.router(290.0, 90.0, &["geosite:kr", "*.netflix.com", "geoip:jp", "기본"]);
        let relay = self.relay(590.0, 200.0, &["레인 1", "레인 2", "레인 3"], &["홍콩", "일본"], &[(0, 0), (1, 0), (2, 1)]);
        let outs: Vec<NodeData> = (0..3).map(|i| self.node(2, 900.0, 40.0 + i as f32 * 120.0)).collect();
        for n in &ins {
            self.links.push(self.link(n.id, 0, router.id, 0));
        }
        self.links.push(self.link(router.id, 0, outs[0].id, 0));
        self.links.push(self.link(router.id, 1, relay.id, 0));
        self.links.push(self.link(router.id, 2, relay.id, 2));
        self.links.push(self.link(router.id, 3, relay.id, 1));
        self.links.push(self.link(relay.id, 0, outs[1].id, 0));
        self.links.push(self.link(relay.id, 1, outs[2].id, 0));
        for n in ins.into_iter().chain([router, relay]).chain(outs) {
            self.nodes.push(n);
        }
    }

    fn log_line(&self) -> SharedString {
        let n = self.log_total.get();
        let level = ["INFO", "INFO", "INFO", "DEBUG", "WARN"][self.rand(5) as usize];
        let hosts = ["www.google.com", "api.github.com", "cdn.jsdelivr.net", "naver.com", "youtube.com", "한국.example"];
        format!(
            "11-24 10:{:02}:{:02}.{:03} {level:<5} [인바운드 {}] tcp 192.168.0.{}:{} -> {}:443 via 아웃바운드 {} ({} ms)",
            (n / 60000) % 60, (n / 1000) % 60, n % 1000, 2 + self.rand(5), 2 + self.rand(250),
            40000 + self.rand(20000), hosts[self.rand(hosts.len() as u64) as usize], 7 + self.rand(8),
            5 + self.rand(300)
        )
        .into()
    }

    /// one 16 ms tick of the fake log stream; runs with or without a window
    fn pump_logs(&self) {
        let want = self.log_debt.get() + self.log_rate.get() as f64 * 0.016;
        let count = want.floor() as usize;
        self.log_debt.set(want - count as f64);
        if count == 0 {
            return;
        }
        for _ in 0..count {
            let line = self.log_line();
            self.logs.push(line);
            self.log_total.set(self.log_total.get() + 1);
        }
        self.log_last_second.set(self.log_last_second.get() + count as u32);
        let extra = self.logs.row_count().saturating_sub(LOG_CAP);
        for _ in 0..extra {
            self.logs.remove(0);
        }
        if let Some(w) = self.window.borrow().as_ref() {
            w.set_log_total(self.log_total.get() as i32);
            if w.get_page() == 1 {
                w.invoke_scroll_to_end();
            }
        }
    }
}

fn pin_in(node: i32, lane: i32) -> i32 {
    node * 64 + lane * 2
}

fn pin_out(node: i32, lane: i32) -> i32 {
    node * 64 + lane * 2 + 1
}

fn strings(items: &[&str]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.iter().map(|s| SharedString::from(*s)).collect::<Vec<_>>()))
}

/// replace a relay's inner wiring (and the per-lane "wired" flags drawn on its dots)
fn set_wires(n: &mut NodeData, mut wires: Vec<Wire>) {
    wires.sort_by_key(|w| (w.i, w.o));
    let wired: Vec<bool> = (0..n.ins).map(|i| wires.iter().any(|w| w.i == i)).collect();
    n.wires = ModelRc::new(VecModel::from(wires));
    n.in_wired = ModelRc::new(VecModel::from(wired));
}

fn strings_owned(items: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.into_iter().map(SharedString::from).collect::<Vec<_>>()))
}

fn row_of(nodes: &VecModel<NodeData>, id: i32) -> Option<usize> {
    (0..nodes.row_count()).find(|&r| nodes.row_data(r).is_some_and(|n| n.id == id))
}

fn remove_where<T: Clone + 'static>(model: &VecModel<T>, remove: impl Fn(&T) -> bool) {
    for i in (0..model.row_count()).rev() {
        if model.row_data(i).is_some_and(|r| remove(&r)) {
            model.remove(i);
        }
    }
}

fn build_window(sh: &Rc<Shared>) -> App {
    let window = App::new().unwrap();
    window.set_nodes(ModelRc::from(sh.nodes.clone()));
    window.set_links(ModelRc::from(sh.links.clone()));
    window.set_log_lines(ModelRc::from(sh.logs.clone()));
    window.set_log_total(sh.log_total.get() as i32);
    window.set_rate_index(RATES.iter().position(|&r| r == sh.log_rate.get()).unwrap_or(0) as i32);

    let nodes = sh.nodes.clone();
    let links = sh.links.clone();
    let setup = NodeEditorSetup::new({
        let nodes = nodes.clone();
        move |dragged, dx, dy| GraphLogic::commit_drag(&nodes, dragged, dx, dy)
    });
    let controller = setup.controller().clone();
    wire_node_editor!(window, setup);
    wire_selection!(window, setup, nodes, links);

    window.on_compute_link_at({
        let controller = controller.clone();
        let links = links.clone();
        move |x, y| {
            let rows = (0..links.row_count()).filter_map(|i| {
                let l = links.row_data(i)?;
                Some((l.id, l.start_pin_id, l.end_pin_id))
            });
            controller.cache().borrow().find_bezier_link_at_world(
                x, y, rows, controller.screen_distance_to_world(8.0), 50.0, 20,
            )
        }
    });

    window.on_link_requested({
        let weak = window.as_weak();
        let controller = controller.clone();
        let sh = sh.clone();
        move |pin_a, pin_b| {
            let Some(w) = weak.upgrade() else { return };
            let cache = controller.cache();
            let cache = cache.borrow();
            let (Some(a), Some(b)) = (cache.pin_positions.get(&pin_a), cache.pin_positions.get(&pin_b)) else {
                return;
            };
            if a.node_id == b.node_id {
                return;
            }
            let t = PinTypes::get(&w);
            let (out, inp) = if a.pin_type == t.get_output() && b.pin_type == t.get_input() {
                (pin_a, pin_b)
            } else if a.pin_type == t.get_input() && b.pin_type == t.get_output() {
                (pin_b, pin_a)
            } else {
                return;
            };
            if sh.links.iter().any(|l| l.start_pin_id == out && l.end_pin_id == inp) {
                return;
            }
            let id = sh.next_link.get();
            sh.next_link.set(id + 1);
            sh.links.push(LinkData::new(id, out, inp, Color::from_rgb_u8(90, 160, 240)));
        }
    });

    window.on_add_node_requested({
        let sh = sh.clone();
        move |kind| {
            let off = sh.rand(200) as f32;
            let (x, y) = (if kind == 0 { 60.0 } else { 700.0 } + off / 4.0, 80.0 + off * 2.0);
            let n = if kind == 3 {
                sh.relay(x, y, &["레인 1", "레인 2"], &["레인 1", "레인 2"], &[(0, 0), (1, 1)])
            } else {
                sh.node(kind, x, y)
            };
            sh.nodes.push(n);
        }
    });

    window.on_add_many_requested({
        let sh = sh.clone();
        move |count| {
            let base = sh.nodes.row_count() as f32;
            for i in 0..count {
                let kind = if i % 2 == 0 { 0 } else { 2 };
                let col = ((base as i32 + i) / 20) as f32;
                let row = ((base as i32 + i) % 20) as f32;
                let n = sh.node(kind, 1000.0 + col * 240.0, row * 80.0);
                if i % 2 == 1 {
                    // connect each new outbound to the inbound added just before it
                    let prev = n.id - 1;
                    let l = sh.link(prev, 0, n.id, 0);
                    sh.nodes.push(n);
                    sh.links.push(l);
                } else {
                    sh.nodes.push(n);
                }
            }
        }
    });

    // relay inner wiring: an input lane goes to at most one output lane (an output may take several inputs)
    window.on_relay_wire_requested({
        let nodes = nodes.clone();
        move |node_id, in_lane, out_lane| {
            let Some(row) = (0..nodes.row_count()).find(|&r| nodes.row_data(r).is_some_and(|n| n.id == node_id)) else {
                return;
            };
            let mut n = nodes.row_data(row).unwrap();
            let mut wires: Vec<Wire> = n.wires.iter().filter(|w| w.i != in_lane).collect();
            if out_lane >= 0 {
                wires.push(Wire { i: in_lane, o: out_lane });
            }
            set_wires(&mut n, wires);
            nodes.set_row_data(row, n);
        }
    });

    window.on_relay_lane_add({
        let nodes = nodes.clone();
        move |node_id, is_in| {
            let Some(row) = row_of(&nodes, node_id) else { return };
            let mut n = nodes.row_data(row).unwrap();
            if is_in {
                n.ins += 1;
                n.in_labels = strings_owned((1..=n.ins).map(|k| format!("레인 {k}")).collect());
            } else {
                n.outs += 1;
                let mut labels: Vec<String> = n.out_labels.iter().map(|s| s.to_string()).collect();
                labels.push(format!("출구 {}", n.outs));
                n.out_labels = strings_owned(labels);
            }
            let wires = n.wires.iter().collect();
            set_wires(&mut n, wires);
            nodes.set_row_data(row, n);
        }
    });

    // removing lane k: its links and inner wires go; lanes below move up one (their pin ids and links follow)
    window.on_relay_lane_remove({
        let weak = window.as_weak();
        let nodes = nodes.clone();
        let links = links.clone();
        move |node_id, is_in, lane| {
            let Some(row) = row_of(&nodes, node_id) else { return };
            let mut n = nodes.row_data(row).unwrap();
            let count = if is_in { n.ins } else { n.outs };
            if count <= 1 || lane < 0 || lane >= count {
                return;
            }
            let pin = |l: i32| if is_in { pin_in(node_id, l) } else { pin_out(node_id, l) };
            let lane_of = |p: i32| -> Option<i32> {
                (p / 64 == node_id && (p % 2 == 1) != is_in).then_some((p % 64) / 2)
            };
            remove_where(&links, |l| (if is_in { l.end_pin_id } else { l.start_pin_id }) == pin(lane));
            for r in 0..links.row_count() {
                let mut l = links.row_data(r).unwrap();
                let end = if is_in { &mut l.end_pin_id } else { &mut l.start_pin_id };
                if lane_of(*end).is_some_and(|k| k > lane) {
                    *end -= 2;
                    links.set_row_data(r, l);
                }
            }
            let wires: Vec<Wire> = n
                .wires
                .iter()
                .filter(|w| (if is_in { w.i } else { w.o }) != lane)
                .map(|w| Wire {
                    i: if is_in && w.i > lane { w.i - 1 } else { w.i },
                    o: if !is_in && w.o > lane { w.o - 1 } else { w.o },
                })
                .collect();
            if is_in {
                n.ins -= 1;
                n.in_labels = strings_owned((1..=n.ins).map(|k| format!("레인 {k}")).collect());
            } else {
                n.outs -= 1;
                let mut labels: Vec<String> = n.out_labels.iter().map(|s| s.to_string()).collect();
                if (lane as usize) < labels.len() {
                    labels.remove(lane as usize);
                }
                n.out_labels = strings_owned(labels);
            }
            set_wires(&mut n, wires);
            nodes.set_row_data(row, n);
            // the highest pin id of that side no longer exists: drop its cached geometry
            if let Some(w) = weak.upgrade() {
                w.global::<NodeEditorInternalCallbacks>().invoke_remove_pin(pin(count - 1));
            }
        }
    });

    window.on_delete_selected_requested({
        let weak = window.as_weak();
        let controller = controller.clone();
        let nodes = nodes.clone();
        let links = links.clone();
        move || {
            let removed: HashSet<i32> = nodes.iter().filter(|n| n.selected).map(|n| n.id).collect();
            let cache = controller.cache();
            let cache = cache.borrow();
            remove_where(&links, |l| {
                l.selected
                    || [l.start_pin_id, l.end_pin_id]
                        .iter()
                        .any(|p| cache.pin_positions.get(p).is_some_and(|pos| removed.contains(&pos.node_id)))
            });
            drop(cache);
            remove_where(&nodes, |n| removed.contains(&n.id));
            if let Some(w) = weak.upgrade() {
                let lc = w.global::<NodeEditorInternalCallbacks>();
                for id in &removed {
                    lc.invoke_remove_node(*id);
                }
            }
        }
    });

    window.on_rate_changed({
        let sh = sh.clone();
        move |i| sh.log_rate.set(RATES[i.clamp(0, RATES.len() as i32 - 1) as usize])
    });
    window.on_clear_logs({
        let sh = sh.clone();
        move || sh.logs.clear()
    });

    // frame counter + startup time (first frame after process creation)
    {
        let frames = sh.frames.clone();
        let startup = sh.startup_ms.clone();
        let first = Rc::new(Cell::new(true));
        let _ = window.window().set_rendering_notifier(move |state, _| {
            if let slint::RenderingState::AfterRendering = state {
                frames.set(frames.get() + 1);
                if first.replace(false) && startup.get() == 0.0 {
                    let ms = ms_since_process_start();
                    startup.set(ms);
                    if let Ok(path) = std::env::var("PROTO_STARTUP_FILE") {
                        let _ = std::fs::write(path, format!("{ms:.1}"));
                        // let the frame reach the screen, then quit
                        Timer::single_shot(Duration::from_millis(50), || {
                            let _ = slint::quit_event_loop();
                        });
                    }
                }
            }
        });
    }

    // closing the window drops the UI; the tray brings it back
    window.window().on_close_requested({
        let sh = sh.clone();
        move || {
            if sh.ui_only {
                // --ui: the window is the whole process; the tray process starts it again when needed
                let _ = slint::quit_event_loop();
                return CloseRequestResponse::HideWindow;
            }
            let sh = sh.clone();
            Timer::single_shot(Duration::ZERO, move || {
                sh.window.borrow_mut().take();
            });
            CloseRequestResponse::HideWindow
        }
    });

    sh.builds.set(sh.builds.get() + 1);
    window
}

fn show_window(sh: &Rc<Shared>) {
    if let Some(w) = sh.window.borrow().as_ref() {
        let _ = w.show();
        return;
    }
    let w = build_window(sh);
    if let Some(page) = std::env::var("PROTO_PAGE").ok().and_then(|p| p.parse().ok()) {
        w.set_page(page);
    }
    w.show().unwrap();
    apply_title_bar(&w);
    w.on_theme_changed({
        let weak = w.as_weak();
        move || {
            if let Some(w) = weak.upgrade() {
                apply_title_bar(&w);
            }
        }
    });
    w.global::<NodeEditorComputations>().invoke_viewport_resized(w.get_width_(), w.get_height_());
    *sh.window.borrow_mut() = Some(w);
}

/// "Seamless" title bar where the OS allows it, keeping the native frame (snap layouts, resizing, no Win10
/// redraw jitter of a frameless window): Windows 11 paints the caption and border in the app's surface colour,
/// Windows 10 (1809+) at least gets the dark caption in dark mode. Other systems keep their own title bar.
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
        const USE_IMMERSIVE_DARK_MODE: u32 = 20;
        const BORDER_COLOR: u32 = 34;
        const CAPTION_COLOR: u32 = 35;
        const TEXT_COLOR: u32 = 36;
        set(USE_IMMERSIVE_DARK_MODE, w.get_dark() as u32);
        // ignored (error) before Windows 11
        set(CAPTION_COLOR, colorref(w.get_caption_color()));
        set(BORDER_COLOR, colorref(w.get_caption_color()));
        set(TEXT_COLOR, colorref(w.get_caption_text()));
    }
}

fn ms_since_process_start() -> f64 {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::{SystemInformation::GetSystemTimePreciseAsFileTime, Threading::*},
    };
    let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    unsafe {
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u, mut now) = (zero, zero, zero, zero, zero);
        GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
        GetSystemTimePreciseAsFileTime(&mut now);
        (ft(now) - ft(c)) as f64 / 10_000.0
    }
}

/// (working set, private bytes) in MB
fn memory_mb() -> (f64, f64) {
    use windows_sys::Win32::System::{ProcessStatus::*, Threading::GetCurrentProcess};
    unsafe {
        let mut pmc: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc as *mut _ as *mut PROCESS_MEMORY_COUNTERS, pmc.cb);
        (pmc.WorkingSetSize as f64 / 1048576.0, pmc.PrivateUsage as f64 / 1048576.0)
    }
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

fn slint_icon() -> Image {
    let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&icon_rgba(), 32, 32);
    Image::from_rgba8(buf)
}

/// The Slint UI. `ui_only`: started by the tray process (`--ui`) - no tray of its own, closing the window ends the
/// process. Otherwise (`--single`) the old one-process layout: Slint tray + window in the same process.
fn run_ui(ui_only: bool, hidden: bool) {
    // software renderer by default: no OpenGL / GPU driver in the process (about half the memory, same speed
    // at normal window sizes); SLINT_BACKEND=winit-femtovg still picks the GPU renderer for comparison
    // context menus drawn by Slint in the app's own (material) style instead of the native Win32 menu;
    // set before Slint starts, while this is still the only thread
    unsafe { std::env::set_var("SLINT_NO_MUDA", "1") };
    if std::env::var_os("SLINT_BACKEND").is_none() {
        let _ = slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name("software".into())
            .select();
    }
    let sh = Rc::new(Shared {
        nodes: Rc::new(VecModel::default()),
        links: Rc::new(VecModel::default()),
        logs: Rc::new(VecModel::default()),
        log_total: Cell::new(0),
        log_rate: Cell::new(500),
        log_debt: Cell::new(0.0),
        log_last_second: Cell::new(0),
        log_lines_per_s: Cell::new(0),
        rng: Cell::new(0x5eed),
        next_node: Cell::new(1),
        next_link: Cell::new(1),
        window: RefCell::new(None),
        frames: Rc::new(Cell::new(0)),
        startup_ms: Rc::new(Cell::new(0.0)),
        builds: Cell::new(0),
        ui_only,
    });
    sh.seed();
    if let Some(rate) = std::env::var("PROTO_RATE").ok().and_then(|r| r.parse().ok()) {
        sh.log_rate.set(rate);
    }

    let tray = if ui_only {
        None
    } else {
        let tray = Tray::new().unwrap();
        tray.set_app_icon(slint_icon());
        tray.on_show_requested({
            let sh = sh.clone();
            move || show_window(&sh)
        });
        tray.on_quit_requested(|| {
            let _ = slint::quit_event_loop();
        });
        Some(tray)
    };

    let log_timer = Timer::default();
    log_timer.start(TimerMode::Repeated, Duration::from_millis(16), {
        let sh = sh.clone();
        move || sh.pump_logs()
    });

    let stats_timer = Timer::default();
    stats_timer.start(TimerMode::Repeated, Duration::from_secs(1), {
        let sh = sh.clone();
        move || {
            sh.log_lines_per_s.set(sh.log_last_second.replace(0));
            let fps = sh.frames.replace(0);
            let (ws, private) = memory_mb();
            let text = format!(
                "UI 프로세스 메모리 {ws:.0} MB (전용 {private:.0} MB) · {fps} fps · 로그 {} 줄/초 · 노드 {} · 시작 {:.0} ms",
                sh.log_lines_per_s.get(),
                sh.nodes.row_count(),
                sh.startup_ms.get(),
            );
            if let Some(w) = sh.window.borrow().as_ref() {
                w.set_stats(text.clone().into());
            }
            if let Ok(path) = std::env::var("PROTO_STATS_FILE") {
                let _ = std::fs::write(path, format!("{ws:.1} {private:.1} {fps} {}", sh.log_lines_per_s.get()));
            }
        }
    });

    if !hidden {
        show_window(&sh);
    }
    slint::run_event_loop_until_quit().unwrap();
    drop(tray);
}

// ---------------------------------------------------------------------------------------------------------------
// Tray process: no Slint (so no GPU driver), a plain Win32 message loop. It starts `--ui` while the window is
// wanted; the UI process exits when its window closes and all of its memory goes back to the system.
// ---------------------------------------------------------------------------------------------------------------

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

fn open_ui() {
    let mut child = UI_CHILD.lock().unwrap();
    if let Some(c) = child.as_mut() {
        if matches!(c.try_wait(), Ok(None)) {
            focus_process_window(c.id());
            return;
        }
    }
    let exe = std::env::current_exe().unwrap();
    *child = std::process::Command::new(exe).arg("--ui").spawn().ok();
}

fn run_tray(show_at_start: bool) {
    use tray_icon::{
        menu::{Menu, MenuEvent, MenuItem},
        Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    let menu = Menu::new();
    let open = MenuItem::new("창 열기", true, None);
    let quit = MenuItem::new("종료", true, None);
    menu.append_items(&[&open, &quit]).unwrap();
    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("ProxyDock prototype")
        .with_icon(Icon::from_rgba(icon_rgba(), 32, 32).unwrap())
        .build()
        .unwrap();

    let (open_id, quit_id) = (open.id().clone(), quit.id().clone());
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == open_id {
            open_ui();
        } else if e.id == quit_id {
            if let Some(mut c) = UI_CHILD.lock().unwrap().take() {
                let _ = c.kill();
            }
            unsafe { PostQuitMessage(0) };
        }
    }));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            open_ui();
        }
    }));

    if show_at_start {
        open_ui();
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
    let has = |flag: &str| std::env::args().any(|a| a == flag);
    if has("--ui") {
        run_ui(true, false);
    } else if has("--single") {
        run_ui(false, has("--hidden"));
    } else {
        run_tray(!has("--hidden"));
    }
}
