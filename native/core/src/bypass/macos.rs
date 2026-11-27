//! The elevated side of the Direct line on macOS (Apple silicon). NOT TRIED ON A REAL MAC YET: written against the
//! Virtualization.framework and vmnet documentation and type-checked only (see README, Limitations).
//!
//! * The guest (`tools/build_bypass_image.py --arch arm64`: kernel Image, initramfs with pd-agent, squashfs with
//!   sing-box) runs in a Virtualization.framework VM: Linux boot loader, 1 CPU, the chosen memory, the squashfs as a
//!   read-only virtio disk, a virtio-vsock device (pd-agent's control and data ports) and an entropy device.
//! * Its network card is a file-handle attachment (one end of a UNIX datagram socket pair). The helper, as root,
//!   pumps the frames between the other end and a vmnet interface in bridged mode on the chosen card: the frames go
//!   straight onto that card, never through the host's IP stack, so a VPN on the Mac does not see them. (Apple
//!   restricts Virtualization.framework's own bridged attachment to signed apps it approves; vmnet's bridged mode
//!   only needs root.)
//! * The app's binary must be signed with the com.apple.security.virtualization entitlement (tools/build_rust.py).
//!
//! Same requests as the Windows helper: prepare, start, stop, status, remove.

use super::{CONTROL_PORT, DATA_PORT, clamp_memory, valid_name};
use crate::i18n::tr;
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{NSArray, NSError, NSFileHandle, NSString, NSURL};
use objc2_virtualization::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::ffi::{CString, c_char, c_int, c_void};
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// --- vmnet and xpc (C APIs; vmnet.framework, libSystem) ---

type XpcObject = *mut c_void;
type InterfaceRef = *mut c_void;

#[repr(C)]
struct VmPktDesc {
    vm_pkt_size: usize,
    vm_pkt_iov: *mut libc::iovec,
    vm_pkt_iovcnt: u32,
    vm_flags: u32,
}

const VMNET_SUCCESS: u32 = 1000;
const VMNET_BRIDGED_MODE: u64 = 1002;
const VMNET_INTERFACE_PACKETS_AVAILABLE: u32 = 1 << 0;

#[link(name = "vmnet", kind = "framework")]
unsafe extern "C" {
    static vmnet_operation_mode_key: *const c_char;
    static vmnet_shared_interface_name_key: *const c_char;
    static vmnet_max_packet_size_key: *const c_char;
    fn vmnet_start_interface(
        desc: XpcObject,
        queue: *mut c_void,
        handler: &block2::DynBlock<dyn Fn(u32, XpcObject)>,
    ) -> InterfaceRef;
    fn vmnet_stop_interface(iface: InterfaceRef, queue: *mut c_void, handler: &block2::DynBlock<dyn Fn(u32)>) -> u32;
    fn vmnet_interface_set_event_callback(
        iface: InterfaceRef,
        mask: u32,
        queue: *mut c_void,
        callback: &block2::DynBlock<dyn Fn(u32, XpcObject)>,
    ) -> u32;
    fn vmnet_read(iface: InterfaceRef, packets: *mut VmPktDesc, count: *mut c_int) -> u32;
    fn vmnet_write(iface: InterfaceRef, packets: *mut VmPktDesc, count: *mut c_int) -> u32;
}

unsafe extern "C" {
    fn xpc_dictionary_create(keys: *const *const c_char, values: *const XpcObject, count: usize) -> XpcObject;
    fn xpc_dictionary_set_uint64(d: XpcObject, key: *const c_char, value: u64);
    fn xpc_dictionary_set_string(d: XpcObject, key: *const c_char, value: *const c_char);
    fn xpc_dictionary_get_uint64(d: XpcObject, key: *const c_char) -> u64;
    fn xpc_release(o: XpcObject);
}

/// the raw dispatch_queue_t of a queue (the C APIs take it)
fn raw_queue(q: &DispatchQueue) -> *mut c_void {
    q as *const DispatchQueue as *mut c_void
}

/// A vmnet interface in bridged mode on `card`, and the threads that pump frames between it and `peer` (the
/// helper's end of the VM's socket pair).
struct Bridge {
    iface: InterfaceRef,
    queue: DispatchRetained<DispatchQueue>,
    stop: Arc<AtomicBool>,
}

// (the vmnet interface is used from its queue and the pump thread only)
unsafe impl Send for Bridge {}

impl Bridge {
    fn start(card: &str, peer: OwnedFd) -> Result<Bridge, String> {
        let queue = DispatchQueue::new("proxydock.vmnet", None);
        let card_c = CString::new(card).map_err(|e| e.to_string())?;
        let (tx, rx) = channel::<(u32, u64)>();
        let iface = unsafe {
            let desc = xpc_dictionary_create(std::ptr::null(), std::ptr::null(), 0);
            xpc_dictionary_set_uint64(desc, vmnet_operation_mode_key, VMNET_BRIDGED_MODE);
            xpc_dictionary_set_string(desc, vmnet_shared_interface_name_key, card_c.as_ptr());
            let tx = Mutex::new(tx);
            let handler = RcBlock::new(move |status: u32, params: XpcObject| {
                let max =
                    if params.is_null() { 1518 } else { xpc_dictionary_get_uint64(params, vmnet_max_packet_size_key) };
                let _ = tx.lock().unwrap().send((status, max));
            });
            let iface = vmnet_start_interface(desc, raw_queue(&queue), &handler);
            xpc_release(desc);
            iface
        };
        let (status, max) = rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "vmnet did not answer".to_string())?;
        if iface.is_null() || status != VMNET_SUCCESS {
            return Err(tr("vmnet could not bridge {card} (status {status}); ProxyDock must run the helper as root")
                .replace("{card}", card)
                .replace("{status}", &status.to_string()));
        }
        let max = max.clamp(1518, 65536) as usize;
        let stop = Arc::new(AtomicBool::new(false));
        let peer = Arc::new(peer);
        // vmnet -> VM: when packets are there, read them all and send each as one datagram
        {
            let peer = peer.clone();
            let iface_addr = iface as usize;
            let cb = RcBlock::new(move |_event: u32, _info: XpcObject| {
                let mut buf = vec![0u8; max];
                loop {
                    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: max };
                    let mut pkt = VmPktDesc { vm_pkt_size: max, vm_pkt_iov: &mut iov, vm_pkt_iovcnt: 1, vm_flags: 0 };
                    let mut count: c_int = 1;
                    let r = unsafe { vmnet_read(iface_addr as InterfaceRef, &mut pkt, &mut count) };
                    if r != VMNET_SUCCESS || count < 1 {
                        break;
                    }
                    unsafe {
                        libc::send(std::os::fd::AsRawFd::as_raw_fd(&*peer), buf.as_ptr().cast(), pkt.vm_pkt_size, 0)
                    };
                }
            });
            unsafe {
                vmnet_interface_set_event_callback(iface, VMNET_INTERFACE_PACKETS_AVAILABLE, raw_queue(&queue), &cb)
            };
        }
        // VM -> vmnet: each datagram from the VM is one frame
        {
            let peer = peer.clone();
            let stop = stop.clone();
            let iface_addr = iface as usize;
            std::thread::spawn(move || {
                let fd = std::os::fd::AsRawFd::as_raw_fd(&*peer);
                let tv = libc::timeval { tv_sec: 0, tv_usec: 200_000 };
                unsafe {
                    libc::setsockopt(
                        fd,
                        libc::SOL_SOCKET,
                        libc::SO_RCVTIMEO,
                        (&tv as *const libc::timeval).cast(),
                        std::mem::size_of::<libc::timeval>() as u32,
                    )
                };
                let mut buf = vec![0u8; max];
                while !stop.load(Ordering::SeqCst) {
                    let n = unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), max, 0) };
                    if n <= 0 {
                        continue;
                    }
                    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: n as usize };
                    let mut pkt =
                        VmPktDesc { vm_pkt_size: n as usize, vm_pkt_iov: &mut iov, vm_pkt_iovcnt: 1, vm_flags: 0 };
                    let mut count: c_int = 1;
                    unsafe { vmnet_write(iface_addr as InterfaceRef, &mut pkt, &mut count) };
                }
            });
        }
        Ok(Bridge { iface, queue, stop })
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let done = RcBlock::new(|_status: u32| {});
        unsafe { vmnet_stop_interface(self.iface, raw_queue(&self.queue), &done) };
    }
}

// --- the VM ---

/// A running guest: its VM (used on its own queue only), the bridge and the host relay.
struct Guest {
    vm: Retained<VZVirtualMachine>,
    queue: DispatchRetained<DispatchQueue>,
    _bridge: Bridge,
    relay_stop: Option<Arc<AtomicBool>>,
    relay_port: u16,
}

// (VZVirtualMachine is used on its queue only: every call below goes through `queue`)
unsafe impl Send for Guest {}

/// what the queue's work hands back across threads
struct SendPtr<T>(T);
unsafe impl<T> Send for SendPtr<T> {}

impl Guest {
    fn boot(files: &Path, card: &str, memory_mb: u32) -> Result<Guest, String> {
        let kernel = files.join("vmlinux");
        let initrd = files.join("initrd");
        let opt = files.join("opt.sqfs");
        for f in [&kernel, &initrd, &opt] {
            if !f.is_file() {
                return Err(tr("The VM image is missing"));
            }
        }
        if !unsafe { VZVirtualMachine::isSupported() } {
            return Err(tr("This Mac cannot run virtual machines"));
        }
        let mut fds = [0 as c_int; 2];
        if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_DGRAM, 0, fds.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        for fd in fds {
            let size: c_int = 4 * 1024 * 1024;
            for opt in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
                unsafe {
                    libc::setsockopt(fd, libc::SOL_SOCKET, opt, (&size as *const c_int).cast(), 4);
                }
            }
        }
        let (vm_end, helper_end) = unsafe { (fds[0], OwnedFd::from_raw_fd(fds[1])) };
        let bridge = Bridge::start(card, helper_end)?;
        let url = |p: &Path| NSURL::fileURLWithPath(&NSString::from_str(&p.display().to_string()));
        let config = unsafe {
            let boot = VZLinuxBootLoader::initWithKernelURL(VZLinuxBootLoader::alloc(), &url(&kernel));
            boot.setInitialRamdiskURL(Some(&url(&initrd)));
            boot.setCommandLine(&NSString::from_str("console=hvc0 quiet rdinit=/init"));
            let config = VZVirtualMachineConfiguration::new();
            config.setBootLoader(Some(&boot));
            config.setCPUCount(1);
            let min = VZVirtualMachineConfiguration::minimumAllowedMemorySize();
            config.setMemorySize((memory_mb as u64 * 1024 * 1024).max(min));
            let disk = VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
                VZDiskImageStorageDeviceAttachment::alloc(),
                &url(&opt),
                true,
            )
            .map_err(|e| e.localizedDescription().to_string())?;
            let blk =
                VZVirtioBlockDeviceConfiguration::initWithAttachment(VZVirtioBlockDeviceConfiguration::alloc(), &disk);
            config.setStorageDevices(&NSArray::from_retained_slice(&[Retained::into_super(blk)]));
            let handle = NSFileHandle::initWithFileDescriptor_closeOnDealloc(NSFileHandle::alloc(), vm_end, true);
            let attachment = VZFileHandleNetworkDeviceAttachment::initWithFileHandle(
                VZFileHandleNetworkDeviceAttachment::alloc(),
                &handle,
            );
            let nic = VZVirtioNetworkDeviceConfiguration::new();
            nic.setAttachment(Some(&attachment));
            nic.setMACAddress(&VZMACAddress::randomLocallyAdministeredAddress());
            config.setNetworkDevices(&NSArray::from_retained_slice(&[Retained::into_super(nic)]));
            let vsock = VZVirtioSocketDeviceConfiguration::new();
            config.setSocketDevices(&NSArray::from_retained_slice(&[Retained::into_super(vsock)]));
            let entropy = VZVirtioEntropyDeviceConfiguration::new();
            config.setEntropyDevices(&NSArray::from_retained_slice(&[Retained::into_super(entropy)]));
            config.validateWithError().map_err(|e| e.localizedDescription().to_string())?;
            config
        };
        let queue = DispatchQueue::new("proxydock.vz", None);
        let (tx, rx) = channel::<Result<SendPtr<Retained<VZVirtualMachine>>, String>>();
        let cfg = SendPtr(config);
        let q2 = queue.clone();
        queue.exec_async(move || {
            let cfg = cfg;
            let vm = unsafe { VZVirtualMachine::initWithConfiguration_queue(VZVirtualMachine::alloc(), &cfg.0, &q2) };
            let tx2 = Mutex::new(tx.clone());
            let vm_for_block = SendPtr(vm.clone());
            let done = RcBlock::new(move |err: *mut NSError| {
                let r = if err.is_null() {
                    Ok(SendPtr(vm_for_block.0.clone()))
                } else {
                    Err(unsafe { (*err).localizedDescription().to_string() })
                };
                let _ = tx2.lock().unwrap().send(r);
            });
            unsafe { vm.startWithCompletionHandler(&done) };
        });
        let vm = rx.recv_timeout(Duration::from_secs(30)).map_err(|_| tr("The direct line VM did not start"))??.0;
        Ok(Guest { vm, queue, _bridge: bridge, relay_stop: None, relay_port: 0 })
    }

    fn connect(&self, port: u32) -> std::io::Result<UnixStream> {
        vsock_connect(&self.vm, &self.queue, port)
    }

    fn agent(&self, req: &Value) -> Result<Value, String> {
        let s = self.connect(CONTROL_PORT).map_err(|e| e.to_string())?;
        let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
        (&s).write_all(format!("{req}\n").as_bytes()).map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(&s).read_line(&mut line).map_err(|e| e.to_string())?;
        serde_json::from_str(&line).map_err(|e| e.to_string())
    }

    fn stop(&mut self) {
        if let Some(s) = self.relay_stop.take() {
            s.store(true, Ordering::SeqCst);
        }
        let _ = self.agent(&json!({"op": "poweroff"}));
        let (tx, rx) = channel::<()>();
        let vm = SendPtr(self.vm.clone());
        self.queue.exec_async(move || {
            let vm = vm;
            let tx = Mutex::new(tx);
            let done = RcBlock::new(move |_err: *mut NSError| {
                let _ = tx.lock().unwrap().send(());
            });
            unsafe {
                if vm.0.canStop() {
                    vm.0.stopWithCompletionHandler(&done);
                } else {
                    tx_done_now(&done);
                }
            }
        });
        let _ = rx.recv_timeout(Duration::from_secs(10));
    }
}

/// (a VM that cannot be stopped is already stopped)
fn tx_done_now(done: &block2::DynBlock<dyn Fn(*mut NSError)>) {
    done.call((std::ptr::null_mut(),));
}

pub struct Helper {
    dir: PathBuf,
    guests: HashMap<String, Guest>,
}

fn name_of(req: &Value) -> Result<String, String> {
    let n = req["name"].as_str().unwrap_or("").to_string();
    if n.is_empty() || valid_name(&n) { Ok(n) } else { Err(tr("Invalid name")) }
}

impl Helper {
    pub fn new(dir: PathBuf) -> Helper {
        Helper { dir, guests: HashMap::new() }
    }

    pub fn handle(&mut self, req: &Value) -> Result<Value, String> {
        let name = name_of(req)?;
        match req["op"].as_str().unwrap_or("") {
            "prepare" => {
                let adapter = req["adapter"].as_str().unwrap_or("");
                if !physical_adapters().iter().any(|a| a == adapter) {
                    return Err(tr("Choose a network card"));
                }
                if !self.dir.join("mac").join("vmlinux").is_file() {
                    return Err(tr("The VM image is missing"));
                }
                Ok(json!({"memory_mb": clamp_memory(req["memory_mb"].as_u64().unwrap_or(0) as u32)}))
            }
            "start" => self.start(
                &name,
                req["adapter"].as_str().unwrap_or(""),
                req["memory_mb"].as_u64().unwrap_or(0) as u32,
                &req["net"],
            ),
            "stop" | "remove" => {
                if let Some(mut g) = self.guests.remove(&name) {
                    g.stop();
                }
                Ok(json!({}))
            }
            "status" => Ok(match self.guests.get(&name) {
                Some(g) => json!({"vm": "Running", "relay": g.relay_port,
                    "agent": g.agent(&json!({"op": "status"})).map(|r| r["status"].clone()).ok()}),
                None => json!({"vm": Value::Null}),
            }),
            op => Err(format!("unknown request {op:?}")),
        }
    }

    pub fn stop_all(&mut self) {
        for (_, mut g) in self.guests.drain() {
            g.stop();
        }
    }

    fn start(&mut self, name: &str, adapter: &str, memory_mb: u32, net: &Value) -> Result<Value, String> {
        if let Some(mut g) = self.guests.remove(name) {
            g.stop();
        }
        if !physical_adapters().iter().any(|a| a == adapter) {
            return Err(tr("Choose a network card"));
        }
        let mut g = Guest::boot(&self.dir.join("mac"), adapter, clamp_memory(memory_mb))?;
        let result = (|| -> Result<Value, String> {
            let until = Instant::now() + Duration::from_secs(60);
            while let Err(e) = g.agent(&json!({"op": "status"})) {
                if Instant::now() > until {
                    return Err(tr("The direct line VM did not answer: {e}").replace("{e}", &e));
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            if net.is_object() {
                let r = g.agent(net)?;
                if r["ok"] != json!(true) {
                    return Err(r["error"].as_str().unwrap_or("network").to_string());
                }
            }
            let until = Instant::now() + Duration::from_secs(40);
            let status = loop {
                let r = g.agent(&json!({"op": "status"}))?;
                if r["status"]["ip"].is_string() && r["status"]["gateway"].is_string() {
                    break r["status"].clone();
                }
                if Instant::now() > until {
                    return Err(
                        tr("The direct line VM got no network: {e}").replace("{e}", &tr("no address from DHCP"))
                    );
                }
                std::thread::sleep(Duration::from_millis(500));
            };
            let user = crate::singbox::process::random_hex(12);
            let pass = crate::singbox::process::random_hex(24);
            let r = g.agent(&json!({"op": "socks_auth", "user": user, "pass": pass}))?;
            if r["ok"] != json!(true) {
                return Err(r["error"].as_str().unwrap_or("socks_auth").to_string());
            }
            std::thread::sleep(Duration::from_millis(800));
            Ok(json!({"user": user, "pass": pass, "status": status}))
        })();
        let mut reply = match result {
            Ok(r) => r,
            Err(e) => {
                g.stop();
                return Err(e);
            }
        };
        let (port, stop) = relay(SendPtr(g.vm.clone()), g.queue.clone())?;
        g.relay_port = port;
        g.relay_stop = Some(stop);
        reply["port"] = json!(port);
        self.guests.insert(name.to_string(), g);
        Ok(reply)
    }
}

/// 127.0.0.1:<port> -> the guest's data port, one vsock connection per connection
fn relay(
    vm: SendPtr<Retained<VZVirtualMachine>>,
    queue: DispatchRetained<DispatchQueue>,
) -> Result<(u16, Arc<AtomicBool>), String> {
    let l = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = l.local_addr().map_err(|e| e.to_string())?.port();
    l.set_nonblocking(true).map_err(|e| e.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    // (a Guest-like handle for connecting: the VM and its queue)
    let guest = Arc::new(Mutex::new(SendPtr((vm.0, queue))));
    std::thread::spawn(move || {
        while !st.load(Ordering::SeqCst) {
            match l.accept() {
                Ok((c, _)) => {
                    let guest = guest.clone();
                    std::thread::spawn(move || {
                        let _ = c.set_nonblocking(false);
                        let v = {
                            let g = guest.lock().unwrap();
                            vsock_connect(&g.0.0, &g.0.1, DATA_PORT)
                        };
                        if let Ok(v) = v {
                            pipe(c, v);
                        }
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        }
    });
    Ok((port, stop))
}

/// a stream to a guest vsock port (the connection is made on the VM's queue; its socket is duplicated)
fn vsock_connect(vm: &Retained<VZVirtualMachine>, queue: &DispatchQueue, port: u32) -> std::io::Result<UnixStream> {
    let (tx, rx) = channel::<Result<c_int, String>>();
    let vm = SendPtr(vm.clone());
    queue.exec_async(move || {
        let vm = vm;
        let devices = unsafe { vm.0.socketDevices() };
        let Some(dev) = devices.firstObject() else {
            let _ = tx.send(Err("no socket device".into()));
            return;
        };
        let Ok(dev) = dev.downcast::<VZVirtioSocketDevice>() else {
            let _ = tx.send(Err("not a virtio socket device".into()));
            return;
        };
        let tx = Mutex::new(tx);
        let done = RcBlock::new(move |conn: *mut VZVirtioSocketConnection, err: *mut NSError| {
            let r = if conn.is_null() {
                Err(if err.is_null() { "vsock".into() } else { unsafe { (*err).localizedDescription().to_string() } })
            } else {
                let fd = unsafe { libc::dup((*conn).fileDescriptor()) };
                if fd < 0 { Err("dup".into()) } else { Ok(fd) }
            };
            let _ = tx.lock().unwrap().send(r);
        });
        unsafe { dev.connectToPort_completionHandler(port, &done) };
    });
    let fd = rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| std::io::Error::other("vsock: no answer"))?
        .map_err(std::io::Error::other)?;
    Ok(unsafe { UnixStream::from_raw_fd(fd) })
}

fn pipe(a: TcpStream, b: UnixStream) {
    let _ = a.set_nodelay(true);
    let (Ok(mut a2), Ok(mut b2)) = (a.try_clone(), b.try_clone()) else { return };
    let t = std::thread::spawn(move || {
        let _ = std::io::copy(&mut a2, &mut b2);
        let _ = b2.shutdown(Shutdown::Write);
    });
    let (mut a, mut b) = (a, b);
    let _ = std::io::copy(&mut b, &mut a);
    let _ = a.shutdown(Shutdown::Write);
    let _ = t.join();
}

/// the Mac's network cards that can be bridged (wired and Wi-Fi): "Device: en0" lines of networksetup
pub fn physical_adapters() -> Vec<String> {
    let Ok(out) = std::process::Command::new("networksetup").arg("-listallhardwareports").output() else {
        return vec![];
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut v: Vec<String> = text
        .lines()
        .filter_map(|l| l.strip_prefix("Device: "))
        .map(|d| d.trim().to_string())
        .filter(|d| d.starts_with("en"))
        .collect();
    v.sort();
    v.dedup();
    v
}
