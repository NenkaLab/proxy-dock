//! sing-box versions: parsing and comparing "1.15.0-alpha.13" style tags (semver order, pre-releases before the
//! release), and asking an executable which version it is (`sing-box version`, cached per file).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// "" for a release, else "alpha.13", "beta.2", "rc.1"
    pub pre: String,
}

impl Version {
    /// "1.15.0-alpha.13", "v1.14.4", "1.14" (a build suffix after '+' is ignored)
    pub fn parse(text: &str) -> Option<Version> {
        let t = text.trim().trim_start_matches('v');
        let t = t.split('+').next()?;
        let (core, pre) = t.split_once('-').unwrap_or((t, ""));
        let mut n = core.split('.').map(|x| x.parse::<u64>().ok());
        let major = n.next()??;
        let minor = n.next()??;
        let patch = n.next().unwrap_or(Some(0))?;
        if n.next().is_some() {
            return None;
        }
        Some(Version { major, minor, patch, pre: pre.to_string() })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    /// at least major.minor, counting that minor's pre-releases (1.15.0-alpha.1 has the 1.15 options)
    pub fn at_least(&self, major: u64, minor: u64) -> bool {
        (self.major, self.minor) >= (major, minor)
    }

    /// Whole-certificate pinning (`certificate_sha256`, ProxyDock's pin_sha256) exists from sing-box 1.15.
    pub fn has_certificate_pin(&self) -> bool {
        self.at_least(1, 15)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre)?;
        }
        Ok(())
    }
}

fn cmp_pre(a: &str, b: &str) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    let (mut x, mut y) = (a.split('.'), b.split('.'));
    loop {
        match (x.next(), y.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) => {
                let o = match (p.parse::<u64>(), q.parse::<u64>()) {
                    (Ok(m), Ok(n)) => m.cmp(&n),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    _ => p.cmp(q),
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

impl Ord for Version {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(o.major, o.minor, o.patch)).then_with(|| cmp_pre(&self.pre, &o.pre))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// the version in `sing-box version` output ("sing-box version 1.15.0-alpha.13" on the first line)
pub fn parse_output(out: &str) -> Option<Version> {
    let line = out.lines().find(|l| l.trim_start().starts_with("sing-box version"))?;
    Version::parse(line.trim_start().trim_start_matches("sing-box version"))
}

type Key = (PathBuf, Option<SystemTime>, u64);
static CACHE: Mutex<Option<HashMap<Key, Option<Version>>>> = Mutex::new(None);

/// The version of a sing-box executable (runs it once per file; None when it does not answer).
pub fn of(exe: &Path) -> Option<Version> {
    let meta = std::fs::metadata(exe).ok()?;
    let key = (exe.to_path_buf(), meta.modified().ok(), meta.len());
    if let Some(v) = CACHE.lock().unwrap().get_or_insert_with(HashMap::new).get(&key) {
        return v.clone();
    }
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("version").stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let v = cmd.output().ok().and_then(|o| parse_output(&String::from_utf8_lossy(&o.stdout)));
    CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, v.clone());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn order_and_features() {
        let mut list = ["1.15.0", "1.15.0-alpha.13", "1.15.0-alpha.2", "1.15.0-beta.1", "1.14.4", "1.15.0-rc.1", "1.16.0-alpha.1"]
            .map(v)
            .to_vec();
        list.sort();
        let names: Vec<String> = list.iter().map(|x| x.to_string()).collect();
        assert_eq!(names, ["1.14.4", "1.15.0-alpha.2", "1.15.0-alpha.13", "1.15.0-beta.1", "1.15.0-rc.1", "1.15.0", "1.16.0-alpha.1"]);
        assert!(!v("1.14.4").has_certificate_pin());
        assert!(v("1.15.0-alpha.1").has_certificate_pin() && v("1.15.0").has_certificate_pin() && v("2.0").has_certificate_pin());
        assert_eq!(v("v1.14.4"), v("1.14.4"));
        assert!(Version::parse("x.1").is_none() && Version::parse("1").is_none() && Version::parse("1.2.3.4").is_none());
    }

    #[test]
    fn version_command_output() {
        let out = "sing-box version 1.15.0-alpha.13\n\nEnvironment: go1.25.3 windows/amd64\nTags: with_gvisor\n";
        assert_eq!(parse_output(out), Some(v("1.15.0-alpha.13")));
        assert_eq!(parse_output("nothing"), None);
    }
}
