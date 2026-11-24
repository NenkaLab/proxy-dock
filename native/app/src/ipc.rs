//! The connection between the core process (proxydock.exe, which runs the cards) and the window process
//! (proxydock-ui.exe): one TCP connection on 127.0.0.1, one JSON object per line.
//!
//! The core starts the window with PROXYDOCK_IPC="<port> <token>". The window's first line is
//! {"cmd": "hello", "token": ...}; the core answers {"ev": "hello", ...} with everything to draw, then sends
//! events ("card", "stats", "log") as they happen. Commands: start / stop (id), stop_all.

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

pub const ENV: &str = "PROXYDOCK_IPC";

pub fn send(stream: &mut TcpStream, v: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(v)?;
    line.push(b'\n');
    stream.write_all(&line)
}

/// the JSON objects read from a stream, until it closes (lines that are not JSON are skipped)
pub fn messages(stream: TcpStream) -> impl Iterator<Item = Value> {
    BufReader::new(stream).lines().map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok())
}

/// constant-time comparison of the token
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
