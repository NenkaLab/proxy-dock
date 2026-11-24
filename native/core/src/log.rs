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
