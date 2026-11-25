//! Log entries (Python: `common.log`, `LOG_*`). Entries go to whoever runs the core (the tray / core process
//! keeps them and shows them in the UI).

/// Log levels, same numbers as the Python app (they are saved in log files and filters).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// TCP connect / close
    Tcp = 0,
    /// internal error
    Error = 1,
    /// settings change
    Setting = 2,
    /// startup / status info
    Info = 3,
    /// UDP session
    Udp = 4,
    /// allow list, private network, IP ban
    Blocked = 5,
    /// authentication failure
    Auth = 6,
    /// remote connection failure (DNS, refused, timeout, upstream proxy error)
    Fail = 7,
    /// warning
    Warn = 8,
}

impl Level {
    pub fn tag(self) -> &'static str {
        match self {
            Level::Tcp => "TCP",
            Level::Udp => "UDP",
            Level::Blocked => "BLOCK",
            Level::Auth => "AUTH",
            Level::Fail => "FAIL",
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Setting => "SET",
            Level::Info => "INFO",
        }
    }

    /// levels whose message becomes the card's "last error" (shown as the reason in the connection check)
    pub fn is_problem(self) -> bool {
        matches!(self, Level::Fail | Level::Auth | Level::Blocked | Level::Error)
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// local time "YYYY-MM-DD HH:MM:SS"
    pub time: String,
    pub level: Level,
    pub message: String,
    pub proxy_id: String,
    pub proxy_name: String,
}

/// control characters become spaces (log lines stay one line each)
pub fn clean(message: &str) -> String {
    message.chars().map(|c| if (c as u32) < 32 || c as u32 == 0x7f { ' ' } else { c }).collect()
}

/// local time as "YYYY-MM-DD HH:MM:SS"
pub fn now_text() -> String {
    #[cfg(windows)]
    {
        let t = unsafe {
            let mut t = std::mem::zeroed();
            windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t);
            t
        };
        format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
    }
    #[cfg(not(windows))]
    {
        // UTC until the platform layer is ported (local time needs the C library's tz data)
        let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let (days, rem) = (s / 86400, s % 86400);
        let (y, m, d) = civil_from_days(days as i64);
        format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
    }
}

/// a Unix time as local "YYYY-MM-DD HH:MM" (Python `time.strftime("%Y-%m-%d %H:%M", time.localtime(t))`)
pub fn local_minute(unix: i64) -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
        use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
        // 100 ns ticks since 1601
        let ticks = (unix.max(0) as u64 + 11_644_473_600) * 10_000_000;
        let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
        let (mut utc, mut local): (SYSTEMTIME, SYSTEMTIME) = unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
        let ok = unsafe { FileTimeToSystemTime(&ft, &mut utc) != 0 && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) != 0 };
        let t = if ok { local } else { utc };
        format!("{:04}-{:02}-{:02} {:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute)
    }
    #[cfg(not(windows))]
    {
        let s = unix.max(0) as u64;
        let (y, m, d) = civil_from_days((s / 86400) as i64);
        format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", s % 86400 / 3600, s % 3600 / 60)
    }
}

#[cfg(not(windows))]
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// "1.5 KB" (Python `common.fmt_size`)
pub fn fmt_size(n: f64) -> String {
    let mut n = n;
    for unit in ["B", "KB", "MB", "GB"] {
        if n < 1024.0 {
            return if unit == "B" { format!("{n:.0} {unit}") } else { format!("{n:.1} {unit}") };
        }
        n /= 1024.0;
    }
    format!("{n:.2} TB")
}

/// one log line as the log page and the log file show it (Python `common.format_line`)
pub fn format_line(e: &Entry) -> String {
    let name = if e.proxy_name.is_empty() { String::new() } else { format!("[{}] ", e.proxy_name) };
    format!("[{}] {:<5} {}{}", e.time, e.level.tag(), name, e.message)
}

/// the log file's size limit and how many older files are kept (Python `RotatingFileHandler(maxBytes, backupCount)`)
pub const FILE_MAX_BYTES: u64 = 2 * 1024 * 1024;
pub const FILE_BACKUPS: u32 = 3;

/// "Save the log to a file": `<dir>/proxydock.log`, moved to `.1` (`.1` to `.2`, ...) when the next line would
/// reach the limit. The file opens with the first line (Python `delay=True`).
pub struct FileLog {
    path: std::path::PathBuf,
    file: Option<std::fs::File>,
}

impl FileLog {
    pub fn open(dir: &std::path::Path) -> std::io::Result<FileLog> {
        std::fs::create_dir_all(dir)?;
        Ok(FileLog { path: dir.join("proxydock.log"), file: None })
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn write(&mut self, line: &str) -> std::io::Result<()> {
        use std::io::{Seek, Write};
        if self.file.is_none() {
            self.file = Some(std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?);
        }
        let pos = self.file.as_mut().unwrap().seek(std::io::SeekFrom::End(0))?;
        // Python compares the byte position with the line's length in characters (+ "\n"); an empty file never rolls
        if pos > 0 && pos + line.chars().count() as u64 + 1 >= FILE_MAX_BYTES && self.path.is_file() {
            self.file = None;
            for i in (1..FILE_BACKUPS).rev() {
                let src = self.backup(i);
                if src.exists() {
                    let _ = std::fs::remove_file(self.backup(i + 1));
                    std::fs::rename(&src, self.backup(i + 1))?;
                }
            }
            let _ = std::fs::remove_file(self.backup(1));
            std::fs::rename(&self.path, self.backup(1))?;
            self.file = Some(std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?);
        }
        // a text-mode file: "\r\n" on Windows
        let nl = if cfg!(windows) { "\r\n" } else { "\n" };
        self.file.as_mut().unwrap().write_all(format!("{line}{nl}").as_bytes())
    }

    fn backup(&self, i: u32) -> std::path::PathBuf {
        let mut p = self.path.clone().into_os_string();
        p.push(format!(".{i}"));
        p.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_log_rolls_over_like_python() {
        let dir = std::env::temp_dir().join(format!("pd-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut f = FileLog::open(&dir).unwrap();
        let line = "x".repeat(1000);
        for _ in 0..(2093 * 4 + 10) {
            f.write(&line).unwrap();
        }
        let size = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.len()).ok();
        // the sizes Python 3.14's RotatingFileHandler leaves for the same lines (on Windows, with CR LF)
        if cfg!(windows) {
            assert_eq!(size(&dir.join("proxydock.log")), Some(14028));
            for i in 1..=3 {
                assert_eq!(size(&dir.join(format!("proxydock.log.{i}"))), Some(2096184));
            }
        }
        assert_eq!(size(&dir.join("proxydock.log.4")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
