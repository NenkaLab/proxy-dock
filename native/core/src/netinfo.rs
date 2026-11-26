//! The PC's network interfaces (for an outbound's "Network interface") and the programs running now (for
//! router program rules): Python `platform_utils.default_route_ifaces / running_programs` and
//! `ui/proxies.network_interfaces`.

/// One network interface that is up (loopback left out).
#[derive(Clone, Debug, PartialEq)]
pub struct Iface {
    /// its name as Windows shows it (what the outbound saves), e.g. "Wi-Fi", "Tailscale"
    pub name: String,
    /// its first IPv4 address ("" = none; link-local 169.254.x.x left out)
    pub ip: String,
    /// it has a default route, i.e. can reach the internet (true when the routing table could not be read,
    /// so nothing is flagged by mistake)
    pub routed: bool,
}

/// Python `_is_default_route`: 0.0.0.0/0, or one half of the 0.0.0.0/1 + 128.0.0.0/1 pair VPN apps
/// install instead (dest and mask as the network-order DWORD Windows uses)
pub fn is_default_route(dest: u32, mask: u32) -> bool {
    mask == 0 || (mask == 0x80 && (dest == 0 || dest == 0x80))
}

/// The interfaces that are up, sorted by name (Python `network_interfaces`).
pub fn interfaces() -> Vec<Iface> {
    let mut out = imp::interfaces();
    out.sort_by_key(|i| i.name.to_lowercase());
    out
}

/// Sorted names of the programs running now (e.g. chrome.exe), Windows' own processes left out.
pub fn running_programs() -> Vec<String> {
    const SKIP: [&str; 14] = [
        "system idle process",
        "system",
        "registry",
        "smss.exe",
        "csrss.exe",
        "wininit.exe",
        "services.exe",
        "lsass.exe",
        "svchost.exe",
        "fontdrvhost.exe",
        "dwm.exe",
        "memory compression",
        "conhost.exe",
        "tasklist.exe",
    ];
    let mut names: Vec<String> =
        imp::process_names().into_iter().filter(|n| !SKIP.contains(&n.to_lowercase().as_str())).collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

#[cfg(windows)]
mod imp {
    use super::Iface;
    use std::collections::HashSet;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_BUFFER_OVERFLOW, ERROR_INSUFFICIENT_BUFFER, INVALID_HANDLE_VALUE, NO_ERROR,
    };
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
        GetIpForwardTable, IP_ADAPTER_ADDRESSES_LH, MIB_IPFORWARDTABLE,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_UNSPEC, SOCKADDR_IN};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };

    /// the indexes of the interfaces with a default route (None = could not tell)
    fn default_route_ifaces() -> Option<HashSet<u32>> {
        let mut size = 0u32;
        unsafe { GetIpForwardTable(std::ptr::null_mut(), &mut size, 0) };
        // (the table can grow between the two calls)
        for _ in 0..3 {
            let mut buf = vec![0u64; (size as usize).div_ceil(8).max(1)];
            let table = buf.as_mut_ptr().cast::<MIB_IPFORWARDTABLE>();
            match unsafe { GetIpForwardTable(table, &mut size, 0) } {
                NO_ERROR => {
                    let n = unsafe { (*table).dwNumEntries } as usize;
                    let rows = unsafe { std::slice::from_raw_parts((*table).table.as_ptr(), n) };
                    return Some(
                        rows.iter()
                            .filter(|r| super::is_default_route(r.dwForwardDest, r.dwForwardMask))
                            .map(|r| r.dwForwardIfIndex)
                            .collect(),
                    );
                }
                ERROR_INSUFFICIENT_BUFFER => continue,
                _ => return None,
            }
        }
        None
    }

    fn wide(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
    }

    pub fn interfaces() -> Vec<Iface> {
        let routes = default_route_ifaces();
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut size = 16 * 1024u32;
        let mut buf: Vec<u64>;
        let mut tries = 0;
        loop {
            buf = vec![0u64; (size as usize).div_ceil(8)];
            let rc = unsafe {
                GetAdaptersAddresses(AF_UNSPEC as u32, flags, std::ptr::null(), buf.as_mut_ptr().cast(), &mut size)
            };
            if rc == NO_ERROR {
                break;
            }
            tries += 1;
            if rc != ERROR_BUFFER_OVERFLOW || tries > 3 {
                return vec![];
            }
        }
        let mut out = vec![];
        let mut a = buf.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !a.is_null() {
            let ad = unsafe { &*a };
            a = ad.Next;
            // up (IfOperStatusUp), not the loopback (IF_TYPE_SOFTWARE_LOOPBACK)
            if ad.OperStatus != 1 || ad.IfType == 24 {
                continue;
            }
            let mut ip = String::new();
            let mut u = ad.FirstUnicastAddress;
            while !u.is_null() {
                let ua = unsafe { &*u };
                u = ua.Next;
                let sa = ua.Address.lpSockaddr;
                if sa.is_null() || unsafe { (*sa).sa_family } != AF_INET {
                    continue;
                }
                let sin = unsafe { &*sa.cast::<SOCKADDR_IN>() };
                let addr = std::net::Ipv4Addr::from(u32::from_be(unsafe { sin.sin_addr.S_un.S_addr }));
                if !addr.is_link_local() {
                    ip = addr.to_string();
                    break;
                }
            }
            let index = unsafe { ad.Anonymous1.Anonymous.IfIndex };
            let index = if index != 0 { index } else { ad.Ipv6IfIndex };
            let routed = routes.as_ref().is_none_or(|r| r.contains(&index));
            out.push(Iface { name: wide(ad.FriendlyName), ip, routed });
        }
        out
    }

    pub fn process_names() -> Vec<String> {
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            return vec![];
        }
        let mut names = vec![];
        let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = unsafe { Process32FirstW(snap, &mut e) } != 0;
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]);
            if name.to_lowercase().ends_with(".exe") {
                names.push(name);
            }
            ok = unsafe { Process32NextW(snap, &mut e) } != 0;
        }
        unsafe { CloseHandle(snap) };
        names
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Iface;

    pub fn interfaces() -> Vec<Iface> {
        vec![]
    }

    pub fn process_names() -> Vec<String> {
        let out = std::process::Command::new("ps")
            .args(["-A", "-o", "comm="])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        out.lines()
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(|x| x.rsplit('/').next().unwrap_or(x).to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_routes() {
        assert!(is_default_route(0, 0));
        assert!(is_default_route(0, 0x80));
        assert!(is_default_route(0x80, 0x80));
        assert!(!is_default_route(0x0a, 0xff));
    }

    #[test]
    fn lists_something() {
        // (this test process itself is running)
        if cfg!(windows) {
            assert!(!running_programs().is_empty());
            assert!(interfaces().iter().all(|i| !i.name.is_empty()));
        }
    }
}
