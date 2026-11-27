//! The elevated side of the VPN bypass: `<ProxyDock> --bypass-helper <port> <token> <dir>`, started through UAC
//! by [`super::hub`] (once per run of ProxyDock). It connects back to 127.0.0.1:<port>, proves itself with <token>
//! and then answers one JSON request per line:
//!
//! * `prepare {name, adapter, memory_mb}`: register pd-agent's Hyper-V socket services, find or create the
//!   External switch of that network card, create or update the VM (Generation 2, 1 CPU, static memory, Secure Boot
//!   off, no checkpoints, never started by Windows itself)
//! * `start {name, net}`: start the VM on a fresh differencing disk of `<dir>/base.vhdx`, wait for pd-agent, set
//!   its address, give its SOCKS5 server a new random account and open a 127.0.0.1 relay port to it
//! * `stop {name}`, `status {name}`, `remove {name}` (also the switch when ProxyDock made it and nothing else uses
//!   it), `quit`
//!
//! Everything goes through PowerShell's Hyper-V module with every value quoted as a PowerShell literal. When the
//! connection closes (ProxyDock quit or crashed) the helper stops the VMs it started and exits.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// the first command line argument of the helper mode
pub const ARG: &str = "--bypass-helper";

/// a PowerShell single-quoted literal
pub fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Run a PowerShell script (Windows PowerShell 5.1, the one with the Hyper-V module) and return what it printed.
/// The script travels base64-encoded (UTF-16), so non-ASCII names such as "이더넷" survive.
#[cfg(windows)]
pub fn powershell(script: &str) -> Result<String, String> {
    use super::Hidden;
    use base64::Engine;
    let full = format!(
        "$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue'; \
         [Console]::OutputEncoding = [Text.Encoding]::UTF8\n{script}"
    );
    let utf16: Vec<u8> = full.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let enc = base64::engine::general_purpose::STANDARD.encode(utf16);
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &enc])
        .stdin(std::process::Stdio::null())
        .creation_flags_hidden()
        .output()
        .map_err(|e| format!("powershell: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() {
        Ok(text)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        // the first line of PowerShell's error record is the message
        let msg = err.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("PowerShell failed").to_string();
        Err(msg)
    }
}

#[cfg(not(windows))]
pub fn powershell(_script: &str) -> Result<String, String> {
    Err("not on Windows".into())
}

/// the differencing disk of an instance (the name hex-encoded: any letters stay a valid file name)
pub fn disk_path(dir: &Path, name: &str) -> PathBuf {
    let slug = if name.is_empty() { "shared".to_string() } else { name.bytes().map(|b| format!("{b:02x}")).collect() };
    dir.join(format!("vm-{slug}.vhdx"))
}

pub fn base_path(dir: &Path) -> PathBuf {
    dir.join("base.vhdx")
}

/// The helper side (runs elevated): `args` = port, token, dir.
pub fn main(args: &[String]) -> i32 {
    let [port, token, dir, rest @ ..] = args else { return 2 };
    if let Some(lang) = rest.first() {
        crate::i18n::set_language(lang);
    }
    let Ok(port) = port.parse::<u16>() else { return 2 };
    let Ok(mut sock) = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(10)) else {
        return 1;
    };
    if sock.write_all(format!("{token}\n").as_bytes()).is_err() {
        return 1;
    }
    serve(sock, PathBuf::from(dir));
    0
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn serve(mut sock: TcpStream, _dir: PathBuf) {
    let _ = sock.write_all(b"{\"ok\":false,\"error\":\"not on Windows\"}\n");
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn serve(sock: TcpStream, dir: PathBuf) {
    let mut w = match sock.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    #[cfg(windows)]
    let mut h = imp::Helper::new(dir);
    #[cfg(target_os = "linux")]
    let mut h = super::linux::Helper::new(dir);
    #[cfg(target_os = "macos")]
    let mut h = super::macos::Helper::new(dir);
    for line in BufReader::new(sock).lines() {
        let Ok(line) = line else { break };
        let req: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        if req["op"] == json!("quit") {
            break;
        }
        let reply = match h.handle(&req) {
            Ok(mut v) => {
                v["ok"] = json!(true);
                v
            }
            Err(e) => json!({"ok": false, "error": e}),
        };
        if w.write_all(format!("{reply}\n").as_bytes()).is_err() {
            break;
        }
    }
    h.stop_all();
}

#[cfg(windows)]
mod imp {
    use super::super::hvsock;
    use super::super::{CONTROL_PORT, DATA_PORT, clamp_memory, valid_name, vm_name};
    use super::{base_path, disk_path, powershell, q};
    use crate::i18n::tr;
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    const GUEST_SERVICES: &str =
        r"HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Virtualization\GuestCommunicationServices";

    struct Relay {
        port: u16,
        stop: Arc<AtomicBool>,
    }

    impl Drop for Relay {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    pub struct Helper {
        dir: PathBuf,
        /// instance name -> its relay, for the VMs this helper started
        started: HashMap<String, Relay>,
    }

    fn name_of(req: &Value) -> Result<String, String> {
        let n = req["name"].as_str().unwrap_or("").to_string();
        if n.is_empty() || valid_name(&n) { Ok(n) } else { Err(tr("Invalid name")) }
    }

    impl Helper {
        pub fn new(dir: PathBuf) -> Helper {
            Helper { dir, started: HashMap::new() }
        }

        pub fn handle(&mut self, req: &Value) -> Result<Value, String> {
            let name = name_of(req)?;
            match req["op"].as_str().unwrap_or("") {
                "prepare" => self.prepare(&name, req["adapter"].as_str().unwrap_or(""), req["memory_mb"].as_u64()),
                "start" => self.start(&name, &req["net"]),
                "stop" => {
                    self.stop(&name);
                    Ok(json!({}))
                }
                "status" => self.status(&name),
                "remove" => self.remove(&name),
                op => Err(format!("unknown request {op:?}")),
            }
        }

        fn prepare(&mut self, name: &str, adapter: &str, memory: Option<u64>) -> Result<Value, String> {
            if adapter.is_empty() {
                return Err(tr("Choose a network card"));
            }
            let base = base_path(&self.dir);
            if !base.is_file() {
                return Err(tr("The VM image is missing"));
            }
            self.stop(name);
            let mb = clamp_memory(memory.unwrap_or(0) as u32);
            let services: Vec<String> = [CONTROL_PORT, DATA_PORT].iter().map(|p| q(&hvsock::service_id(*p))).collect();
            let script = format!(
                r#"
$gk = {gk}
foreach ($g in @({services})) {{
    if (-not (Test-Path "$gk\$g")) {{ $k = New-Item "$gk\$g" -Force; $k.SetValue('ElementName', 'ProxyDock VPN bypass') }}
}}
$nic = Get-NetAdapter -Physical -Name {adapter}
$sw = Get-VMSwitch -SwitchType External -ErrorAction SilentlyContinue |
    Where-Object {{ $_.NetAdapterInterfaceDescription -eq $nic.InterfaceDescription }} | Select-Object -First 1
if (-not $sw) {{ $sw = New-VMSwitch -Name ('ProxyDock (' + $nic.Name + ')') -NetAdapterName $nic.Name -AllowManagementOS $true }}
$vm = Get-VM -Name {vm} -ErrorAction SilentlyContinue
if (-not $vm) {{
    $vm = New-VM -Name {vm} -Generation 2 -MemoryStartupBytes ({mb} * 1MB) -NoVHD -SwitchName $sw.Name
    Set-VMFirmware -VM $vm -EnableSecureBoot Off
    Set-VMProcessor -VM $vm -Count 1
    Set-VM -VM $vm -AutomaticCheckpointsEnabled $false -CheckpointType Disabled -AutomaticStartAction Nothing `
        -AutomaticStopAction TurnOff -Notes 'ProxyDock VPN bypass'
}} else {{
    if ($vm.State -ne 'Off') {{ Stop-VM -VM $vm -TurnOff -Force }}
    Get-VMNetworkAdapter -VM $vm | Connect-VMNetworkAdapter -SwitchName $sw.Name
}}
Set-VM -VM $vm -StaticMemory -MemoryStartupBytes ({mb} * 1MB)
Write-Output $vm.Id.Guid
Write-Output $sw.Name
"#,
                gk = q(GUEST_SERVICES),
                services = services.join(", "),
                adapter = q(adapter),
                vm = q(&vm_name(name)),
            );
            let out = powershell(&script)?;
            let mut lines = out.lines().map(str::trim).filter(|l| !l.is_empty());
            let id = lines.next().unwrap_or("").to_string();
            let switch = lines.next().unwrap_or("").to_string();
            Ok(json!({"vm_id": id, "switch": switch, "memory_mb": mb}))
        }

        /// the VM's id and state ("Off", "Running", ...), None when there is no such VM
        fn vm(&self, name: &str) -> Result<Option<(String, String)>, String> {
            let out = powershell(&format!(
                "$vm = Get-VM -Name {} -ErrorAction SilentlyContinue; if ($vm) {{ $vm.Id.Guid; [string]$vm.State }}",
                q(&vm_name(name))
            ))?;
            let mut l = out.lines().map(str::trim);
            Ok(match (l.next(), l.next()) {
                (Some(id), Some(st)) if !id.is_empty() => Some((id.to_string(), st.to_string())),
                _ => None,
            })
        }

        fn start(&mut self, name: &str, net: &Value) -> Result<Value, String> {
            let Some((id, state)) = self.vm(name)? else {
                return Err(tr("The direct line VM is not prepared yet (Settings > Direct line)"));
            };
            if state != "Running" {
                if state != "Off" {
                    powershell(&format!("Stop-VM -Name {} -TurnOff -Force", q(&vm_name(name))))?;
                }
                // a fresh differencing disk every start: the image is never written, and a newer image is picked
                // up without anything left over from the old one
                let disk = disk_path(&self.dir, name);
                powershell(&format!(
                    r#"
$vm = Get-VM -Name {vm}
Get-VMHardDiskDrive -VM $vm | Remove-VMHardDiskDrive
if (Test-Path -LiteralPath {disk}) {{ Remove-Item -LiteralPath {disk} -Force }}
New-VHD -Path {disk} -ParentPath {base} -Differencing | Out-Null
Add-VMHardDiskDrive -VM $vm -Path {disk}
Set-VMFirmware -VM $vm -FirstBootDevice (Get-VMHardDiskDrive -VM $vm | Select-Object -First 1)
Start-VM -VM $vm
"#,
                    vm = q(&vm_name(name)),
                    disk = q(&disk.display().to_string()),
                    base = q(&base_path(&self.dir).display().to_string()),
                ))?;
            }
            let until = Instant::now() + Duration::from_secs(60);
            loop {
                let e = match agent(&id, &json!({"op": "status"})) {
                    Ok(_) => break,
                    Err(e) => e,
                };
                if Instant::now() > until {
                    return Err(tr("The direct line VM did not answer: {e}").replace("{e}", &e));
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            if net.is_object() {
                let r = agent(&id, net)?;
                if r["ok"] != json!(true) {
                    return Err(r["error"].as_str().unwrap_or("network").to_string());
                }
            }
            // an address (DHCP may take a few seconds)
            let until = Instant::now() + Duration::from_secs(40);
            let status = loop {
                let r = agent(&id, &json!({"op": "status"}))?;
                if r["status"]["ip"].is_string()
                    && r["status"]["gateway"].is_string()
                    && r["status"]["singbox"] == json!(true)
                {
                    break r["status"].clone();
                }
                if Instant::now() > until {
                    let why = r["status"]["error"].as_str().map(String::from).unwrap_or_else(|| {
                        if r["status"]["link"] == json!(false) {
                            tr("the network card has no connection")
                        } else {
                            tr("no address from DHCP")
                        }
                    });
                    return Err(tr("The direct line VM got no network: {e}").replace("{e}", &why));
                }
                std::thread::sleep(Duration::from_millis(500));
            };
            let user = crate::singbox::process::random_hex(12);
            let pass = crate::singbox::process::random_hex(24);
            let r = agent(&id, &json!({"op": "socks_auth", "user": user, "pass": pass}))?;
            if r["ok"] != json!(true) {
                return Err(r["error"].as_str().unwrap_or("socks_auth").to_string());
            }
            // (sing-box in the VM restarts with the account: wait until it runs again)
            let until = Instant::now() + Duration::from_secs(10);
            while Instant::now() < until {
                std::thread::sleep(Duration::from_millis(300));
                if agent(&id, &json!({"op": "status"})).is_ok_and(|r| r["status"]["singbox"] == json!(true)) {
                    break;
                }
            }
            self.started.remove(name);
            let relay = relay(id.clone())?;
            let port = relay.port;
            self.started.insert(name.to_string(), relay);
            Ok(json!({"port": port, "user": user, "pass": pass, "vm_id": id, "status": status}))
        }

        fn stop(&mut self, name: &str) {
            self.started.remove(name);
            let Ok(Some((id, state))) = self.vm(name) else { return };
            if state == "Off" {
                return;
            }
            let _ = agent(&id, &json!({"op": "poweroff"}));
            for _ in 0..10 {
                std::thread::sleep(Duration::from_secs(1));
                if self.vm(name).ok().flatten().is_none_or(|(_, s)| s == "Off") {
                    return;
                }
            }
            let _ = powershell(&format!("Stop-VM -Name {} -TurnOff -Force", q(&vm_name(name))));
        }

        pub fn stop_all(&mut self) {
            let names: Vec<String> = self.started.keys().cloned().collect();
            for n in names {
                self.stop(&n);
            }
        }

        fn status(&mut self, name: &str) -> Result<Value, String> {
            let Some((id, state)) = self.vm(name)? else { return Ok(json!({"vm": Value::Null})) };
            let agent_status = if state == "Running" {
                agent(&id, &json!({"op": "status"})).map(|r| r["status"].clone()).ok()
            } else {
                None
            };
            Ok(
                json!({"vm": state, "vm_id": id, "relay": self.started.get(name).map(|r| r.port), "agent": agent_status}),
            )
        }

        fn remove(&mut self, name: &str) -> Result<Value, String> {
            self.stop(name);
            let disk = disk_path(&self.dir, name);
            powershell(&format!(
                r#"
$vm = Get-VM -Name {vm} -ErrorAction SilentlyContinue
$switch = $null
if ($vm) {{
    $switch = (Get-VMNetworkAdapter -VM $vm | Select-Object -First 1).SwitchName
    Remove-VM -VM $vm -Force
}}
if (Test-Path -LiteralPath {disk}) {{ Remove-Item -LiteralPath {disk} -Force }}
if ($switch -and $switch.StartsWith('ProxyDock (')) {{
    $users = Get-VMNetworkAdapter -All | Where-Object {{ $_.SwitchName -eq $switch -and -not $_.IsManagementOs }}
    if (-not $users) {{ Remove-VMSwitch -Name $switch -Force }}
}}
"#,
                vm = q(&vm_name(name)),
                disk = q(&disk.display().to_string()),
            ))?;
            Ok(json!({}))
        }
    }

    /// one request to pd-agent's control port
    fn agent(vm_id: &str, req: &Value) -> Result<Value, String> {
        let s = hvsock::connect_vm(vm_id, CONTROL_PORT, Duration::from_secs(3)).map_err(|e| e.to_string())?;
        let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
        (&s).write_all(format!("{req}\n").as_bytes()).map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(&s).read_line(&mut line).map_err(|e| e.to_string())?;
        serde_json::from_str(&line).map_err(|e| e.to_string())
    }

    /// 127.0.0.1:<port> -> the VM's data port, one Hyper-V socket per connection
    fn relay(vm_id: String) -> Result<Relay, String> {
        let l = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = l.local_addr().map_err(|e| e.to_string())?.port();
        l.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        std::thread::spawn(move || {
            while !st.load(Ordering::SeqCst) {
                match l.accept() {
                    Ok((c, _)) => {
                        let id = vm_id.clone();
                        std::thread::spawn(move || pipe_to_vm(c, &id));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(50))
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
        });
        Ok(Relay { port, stop })
    }

    fn pipe_to_vm(c: TcpStream, vm_id: &str) {
        let _ = c.set_nonblocking(false);
        let _ = c.set_nodelay(true);
        let Ok(v) = hvsock::connect_vm(vm_id, DATA_PORT, Duration::from_secs(5)) else { return };
        let (Ok(mut c2), Ok(mut v2)) = (c.try_clone(), v.try_clone()) else { return };
        let t = std::thread::spawn(move || {
            let _ = std::io::copy(&mut c2, &mut v2);
            let _ = v2.shutdown(Shutdown::Write);
        });
        let (mut v, mut c) = (v, c);
        let _ = std::io::copy(&mut v, &mut c);
        let _ = c.shutdown(Shutdown::Write);
        let _ = t.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_and_paths() {
        assert_eq!(q("it's"), "'it''s'");
        assert_eq!(q("이더넷"), "'이더넷'");
        let d = Path::new("C:/x");
        assert_eq!(disk_path(d, ""), Path::new("C:/x/vm-shared.vhdx"));
        assert_eq!(disk_path(d, "ab"), Path::new("C:/x/vm-6162.vhdx"));
    }
}
