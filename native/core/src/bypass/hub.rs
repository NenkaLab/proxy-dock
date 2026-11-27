//! ProxyDock's side of the VPN bypass: one elevated helper per run of ProxyDock (started through UAC the first
//! time a VM is needed), and which VMs are in use by which running cards.
//!
//! [`Hub::acquire`] is called before a card's sing-box starts, for each instance the card leaves through; it starts
//! the VM when it is the first user and returns the relay port and the account. [`Hub::release`] stops the VM when
//! its last card stops (unless "keep running" is on). Preparing (image, switch, VM) is a settings action.

use super::{IMAGE_NAME, IMAGE_ZIP, Instance, Store, data_dir, helper};
use crate::i18n::tr;
use crate::singbox::helper::Launch;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// where a card's sing-box reaches one instance
#[derive(Clone, Debug, PartialEq)]
pub struct Endpoint {
    pub port: u16,
    pub user: String,
    pub pass: String,
}

struct Conn {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

#[derive(Default)]
pub struct Hub {
    conn: Option<Conn>,
    /// instance -> (users: cards and the user's switch, its endpoint)
    used: HashMap<String, (usize, Endpoint)>,
    /// instances the user switched on (held running without a card)
    manual: std::collections::HashSet<String>,
    /// tests: how to start the helper (default: this program through UAC, or directly when elevated)
    pub launch: Option<Launch>,
    /// tests: the data folder (default `super::data_dir()`)
    pub dir: Option<PathBuf>,
}

/// the process-wide hub (the card manager and the settings share it)
pub fn hub() -> &'static Mutex<Hub> {
    static HUB: OnceLock<Mutex<Hub>> = OnceLock::new();
    HUB.get_or_init(|| Mutex::new(Hub::default()))
}

impl Hub {
    fn dir(&self) -> PathBuf {
        self.dir.clone().unwrap_or_else(data_dir)
    }

    fn launch_helper(&mut self) -> Result<(), String> {
        let dir = self.dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let how = match &self.launch {
            Some(l) => l.clone(),
            None => {
                let program = std::env::current_exe().map_err(|e| e.to_string())?;
                Launch { program, args: vec![helper::ARG.into()], elevate: !crate::manager::is_admin() }
            }
        };
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let token = crate::singbox::process::random_hex(16);
        let mut args = how.args.clone();
        // (the language last: the helper's messages come back in it)
        args.extend([port.to_string(), token.clone(), dir.display().to_string(), crate::i18n::current()]);
        if how.elevate {
            crate::singbox::helper::runas(&how.program, &args)?;
        } else {
            use super::Hidden;
            let mut child = std::process::Command::new(&how.program)
                .args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .creation_flags_hidden()
                .spawn()
                .map_err(|e| e.to_string())?;
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let until = Instant::now() + crate::singbox::helper::CONNECT_TIMEOUT;
        while Instant::now() < until {
            match listener.accept() {
                Ok((s, _)) => {
                    let _ = s.set_nonblocking(false);
                    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
                    let mut line = vec![];
                    let mut byte = [0u8; 1];
                    while line.len() < 256 && (&s).read_exact(&mut byte).is_ok() && byte[0] != b'\n' {
                        line.push(byte[0]);
                    }
                    if line.trim_ascii() == token.as_bytes() {
                        // (a VM start waits for DHCP and the agent: give requests time)
                        let _ = s.set_read_timeout(Some(Duration::from_secs(300)));
                        let writer = s.try_clone().map_err(|e| e.to_string())?;
                        self.conn = Some(Conn { reader: BufReader::new(s), writer });
                        return Ok(());
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(e.to_string()),
            }
        }
        Err(tr("The administrator helper did not connect"))
    }

    /// one request to the helper (started first when needed); Err = the helper's message
    fn request(&mut self, req: Value) -> Result<Value, String> {
        for attempt in 0..2 {
            if self.conn.is_none() {
                self.launch_helper()?;
            }
            let c = self.conn.as_mut().unwrap();
            let sent = c.writer.write_all(format!("{req}\n").as_bytes());
            let mut line = String::new();
            let read = sent.and_then(|_| c.reader.read_line(&mut line));
            match read {
                Ok(n) if n > 0 => {
                    let v: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
                    return if v["ok"] == json!(true) {
                        Ok(v)
                    } else {
                        Err(v["error"].as_str().unwrap_or("?").to_string())
                    };
                }
                _ => {
                    // the helper is gone (it was killed, or the PC slept): every VM it ran is stopped
                    self.conn = None;
                    self.used.clear();
                    self.manual.clear();
                    if attempt == 1 {
                        return Err(tr("The administrator helper stopped"));
                    }
                }
            }
        }
        unreachable!()
    }

    /// A card that leaves through `name` starts: the VM is started when it is the first such card.
    pub fn acquire(&mut self, name: &str) -> Result<Endpoint, String> {
        if let Some((n, ep)) = self.used.get_mut(name) {
            *n += 1;
            return Ok(ep.clone());
        }
        super::availability()?;
        let store = Store::load();
        let label = super::label(name);
        let Some(inst) = store.get(name) else {
            return Err(
                tr("The direct line \"{name}\" is not set up (Settings > Direct line)").replace("{name}", &label)
            );
        };
        if !inst.consented() {
            return Err(
                tr("The direct line \"{name}\" is not prepared (Settings > Direct line)").replace("{name}", &label)
            );
        }
        if !inst.enabled {
            return Err(
                tr("The direct line VM \"{name}\" is switched off (Settings > Direct line)").replace("{name}", &label)
            );
        }
        let r = self.request(json!({"op": "start", "name": name, "adapter": inst.adapter,
            "memory_mb": inst.memory_mb, "net": inst.net_request()}))?;
        let ep = Endpoint {
            port: r["port"].as_u64().unwrap_or(0) as u16,
            user: r["user"].as_str().unwrap_or("").into(),
            pass: r["pass"].as_str().unwrap_or("").into(),
        };
        if ep.port == 0 {
            return Err(tr("The administrator helper stopped"));
        }
        self.used.insert(name.to_string(), (1, ep.clone()));
        Ok(ep)
    }

    /// A card that left through `name` stopped.
    pub fn release(&mut self, name: &str) {
        let Some((n, _)) = self.used.get_mut(name) else { return };
        *n -= 1;
        if *n > 0 {
            return;
        }
        self.used.remove(name);
        if !Store::load().keep_running && self.conn.is_some() {
            let _ = self.request(json!({"op": "stop", "name": name}));
        }
    }

    /// cards using it (the user's switch not counted)
    pub fn in_use(&self, name: &str) -> usize {
        self.used.get(name).map_or(0, |(n, _)| *n).saturating_sub(usize::from(self.manual.contains(name)))
    }

    /// the VM runs for ProxyDock now (a card or the switch holds it)
    pub fn running(&self, name: &str) -> bool {
        self.used.contains_key(name)
    }

    /// The user switched it on: started now and kept running without a card.
    pub fn hold(&mut self, name: &str) -> Result<(), String> {
        if self.manual.contains(name) {
            return Ok(());
        }
        self.acquire(name)?;
        self.manual.insert(name.to_string());
        Ok(())
    }

    /// The user switched it off (its cards were stopped first): the VM stops, "keep running" or not.
    pub fn force_stop(&mut self, name: &str) {
        self.manual.remove(name);
        self.used.remove(name);
        if self.conn.is_some() {
            let _ = self.request(json!({"op": "stop", "name": name}));
        }
    }

    /// Set up (or update) an instance: the image in place, then the switch and the VM. The caller saved the consent.
    pub fn prepare(&mut self, inst: &Instance, progress: &dyn Fn(&str)) -> Result<Value, String> {
        super::availability()?;
        if self.in_use(&inst.name) > 0 {
            return Err(tr("Stop the cards that use this direct line first"));
        }
        // (switched on by hand: preparing stops it)
        if self.running(&inst.name) {
            self.force_stop(&inst.name);
        }
        // (Linux needs no VM, so no image)
        if !cfg!(target_os = "linux") {
            progress(&tr("Getting the VM image..."));
            self.ensure_image(progress)?;
        }
        progress(&tr("Creating the VM (the network may drop for a moment)..."));
        self.request(json!({"op": "prepare", "name": inst.name, "adapter": inst.adapter, "memory_mb": inst.memory_mb}))
    }

    pub fn status(&mut self, name: &str) -> Result<Value, String> {
        self.request(json!({"op": "status", "name": name}))
    }

    pub fn remove(&mut self, name: &str) -> Result<(), String> {
        if self.in_use(name) > 0 {
            return Err(tr("Stop the cards that use this direct line first"));
        }
        self.request(json!({"op": "remove", "name": name})).map(|_| ())
    }

    /// Put this system's guest in place: Windows `<dir>/base.vhdx`, macOS `<dir>/mac/{vmlinux,initrd,opt.sqfs}` -
    /// from PROXYDOCK_BYPASS_IMAGE or a build next to the program (`dist/bypass`), else downloaded from this
    /// version's GitHub release (checked against its SHA256SUMS.txt). Every VM must be off: a running one uses it.
    fn ensure_image(&mut self, progress: &dyn Fn(&str)) -> Result<(), String> {
        let dir = self.dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let stamp = dir.join("base.version");
        let want = crate::update::current_version().to_string();
        let have = std::fs::read_to_string(&stamp).unwrap_or_default();
        if image_ready(&dir) && have.trim() == want {
            return Ok(());
        }
        if !self.used.is_empty() {
            return Err(tr("Stop the cards that use the direct line first"));
        }
        let zip = match local_image() {
            Some(p) => p,
            None => download_image(&dir, progress)?,
        };
        unpack_image(&zip, &dir)?;
        if zip.starts_with(&dir) {
            let _ = std::fs::remove_file(&zip);
        }
        std::fs::write(&stamp, want).map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// this system's guest is unpacked in `dir`
fn image_ready(dir: &Path) -> bool {
    if cfg!(target_os = "macos") {
        ["vmlinux", "initrd", "opt.sqfs"].iter().all(|f| dir.join("mac").join(f).is_file())
    } else {
        helper::base_path(dir).is_file()
    }
}

/// an image built on this PC: PROXYDOCK_BYPASS_IMAGE (a zip, or Windows' .vhdx itself), or dist/bypass/ above the
/// program (development)
fn local_image() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PROXYDOCK_BYPASS_IMAGE").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let exe = std::env::current_exe().ok()?;
    let names: &[&str] = if cfg!(target_os = "macos") { &[IMAGE_ZIP] } else { &[IMAGE_NAME, IMAGE_ZIP] };
    exe.ancestors()
        .skip(1)
        .take(6)
        .flat_map(|d| names.iter().map(move |n| d.join("dist").join("bypass").join(n)))
        .find(|p| p.is_file())
}

/// the zip (or Windows' bare .vhdx) -> `dir`: Windows base.vhdx, macOS mac/{vmlinux,initrd,opt.sqfs}
fn unpack_image(src: &Path, dir: &Path) -> Result<(), String> {
    let part = |p: PathBuf| p.with_extension("part");
    if src.extension().is_some_and(|e| e.eq_ignore_ascii_case("vhdx")) {
        let base = helper::base_path(dir);
        std::fs::copy(src, part(base.clone())).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&base);
        return std::fs::rename(part(base.clone()), &base).map_err(|e| e.to_string());
    }
    let file = std::fs::File::open(src).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let wanted: Vec<(&str, PathBuf)> = if cfg!(target_os = "macos") {
        ["vmlinux", "initrd", "opt.sqfs"].iter().map(|f| (*f, dir.join("mac").join(f))).collect()
    } else {
        vec![(IMAGE_NAME, helper::base_path(dir))]
    };
    for (name, to) in wanted {
        if let Some(d) = to.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let mut entry = archive.by_name(name).map_err(|e| format!("{name}: {e}"))?;
        let tmp = part(to.clone());
        let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        drop(out);
        // (a VM still off with a disk on the old image keeps nothing: each start makes a new disk)
        let _ = std::fs::remove_file(&to);
        std::fs::rename(&tmp, &to).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// this version's release zip for this system, downloaded into `dir` and checked against SHA256SUMS.txt
fn download_image(dir: &Path, progress: &dyn Fn(&str)) -> Result<PathBuf, String> {
    let tag = format!("v{}", crate::update::current_version());
    let rel = crate::update::release_for_tag(&tag)?;
    let last = Mutex::new(Instant::now());
    crate::update::fetch_verified(
        &rel,
        IMAGE_ZIP,
        dir,
        &|done, total| {
            let mut l = last.lock().unwrap();
            if l.elapsed() > Duration::from_millis(500) && total > 0 {
                *l = Instant::now();
                progress(&tr("Downloading the VM image... {pct}%").replace("{pct}", &(done * 100 / total).to_string()));
            }
        },
        &|| false,
    )
    .map_err(|e| match e {
        crate::update::FetchError::Failed(m) => m,
        crate::update::FetchError::Cancelled => tr("Cancelled"),
    })
}
