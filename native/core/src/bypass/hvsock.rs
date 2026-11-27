//! Hyper-V sockets (AF_HYPERV) from the host to a VM: no IP involved, so the host's VPN and firewall never see
//! them. The guest listens with AF_VSOCK; a vsock port N is the service GUID `N-facb-11e6-bd58-64006a7986d3`,
//! which must be registered under GuestCommunicationServices on the host (administrator, see `helper`).

use std::net::TcpStream;
use std::os::windows::io::FromRawSocket;
use std::time::Duration;
use windows_sys::Win32::Networking::WinSock::{
    INVALID_SOCKET, SOCK_STREAM, SOCKADDR, WSAGetLastError, closesocket, connect, socket,
};
use windows_sys::core::GUID;

const AF_HYPERV: u16 = 34;
const HV_PROTOCOL_RAW: i32 = 1;
/// the template the guest's vsock ports are mapped onto
const FACADE: &str = "-facb-11e6-bd58-64006a7986d3";

#[repr(C)]
struct SockaddrHv {
    family: u16,
    reserved: u16,
    vm_id: GUID,
    service_id: GUID,
}

/// the service id of a guest vsock port
pub fn service_id(port: u32) -> String {
    format!("{port:08x}{FACADE}")
}

/// "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx" (braces allowed) -> GUID
pub fn parse_guid(s: &str) -> Option<GUID> {
    let hex: String = s.trim().trim_start_matches('{').trim_end_matches('}').chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(GUID::from_u128(u128::from_str_radix(&hex, 16).ok()?))
}

/// Connect to a guest vsock port. The stream is a std TcpStream wrapping the socket: reading, writing, timeouts
/// and shutdown are the same Winsock calls for any stream socket.
pub fn connect_vm(vm_id: &str, port: u32, timeout: Duration) -> std::io::Result<TcpStream> {
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad VM id");
    let addr = SockaddrHv {
        family: AF_HYPERV,
        reserved: 0,
        vm_id: parse_guid(vm_id).ok_or_else(bad)?,
        service_id: parse_guid(&service_id(port)).ok_or_else(bad)?,
    };
    // (Winsock must be started; std does that the first time it makes a socket)
    let _ = std::net::UdpSocket::bind("127.0.0.1:0");
    unsafe {
        let s = socket(AF_HYPERV as i32, SOCK_STREAM, HV_PROTOCOL_RAW);
        if s == INVALID_SOCKET {
            return Err(std::io::Error::from_raw_os_error(WSAGetLastError()));
        }
        // HVSOCKET_CONNECT_TIMEOUT (option 1 at level HV_PROTOCOL_RAW), in milliseconds
        let ms: u32 = timeout.as_millis().min(u32::MAX as u128) as u32;
        windows_sys::Win32::Networking::WinSock::setsockopt(
            s,
            HV_PROTOCOL_RAW,
            1,
            (&ms as *const u32).cast(),
            std::mem::size_of::<u32>() as i32,
        );
        let len = std::mem::size_of::<SockaddrHv>() as i32;
        if connect(s, (&addr as *const SockaddrHv).cast::<SOCKADDR>(), len) != 0 {
            let e = WSAGetLastError();
            closesocket(s);
            return Err(std::io::Error::from_raw_os_error(e));
        }
        Ok(TcpStream::from_raw_socket(s as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids() {
        assert_eq!(service_id(20548), "00005044-facb-11e6-bd58-64006a7986d3");
        let g = parse_guid("{2827092B-C323-4922-96AF-C75A939EA47B}").unwrap();
        assert_eq!(g.data1, 0x2827092b);
        assert_eq!(g.data2, 0xc323);
        assert_eq!(g.data4, [0x96, 0xaf, 0xc7, 0x5a, 0x93, 0x9e, 0xa4, 0x7b]);
        assert!(parse_guid("nope").is_none());
    }
}
