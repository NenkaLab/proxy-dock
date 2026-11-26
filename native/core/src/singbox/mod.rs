//! Native engines (Python: `proxydock/singbox/`). Each running proxy card is one sing-box process.

pub mod compile;
pub mod guard;
pub mod helper;
pub mod hysteria;
pub mod latest;
pub mod process;
pub mod rulesets;
pub mod version;

use std::path::{Path, PathBuf};

pub const SINGBOX_VERSION: &str = "1.14.4";
pub const HYSTERIA_VERSION: &str = "2.14.0";
pub const EXE: &str = if cfg!(windows) { "sing-box.exe" } else { "sing-box" };
pub const HY_EXE: &str = if cfg!(windows) { "hysteria.exe" } else { "hysteria" };

/// "windows-amd64", "darwin-arm64", ... (the bin/ sub folder of this OS and CPU)
pub fn platform_dir() -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{os}-{arch}")
}

/// `exe` in a custom path (setting), else in bin/<platform>/ or bin/ under one of `bases` (the app folder, the
/// repository root in development), else on PATH.
pub fn find(exe: &str, custom: &str, bases: &[PathBuf]) -> Option<PathBuf> {
    if !custom.is_empty() {
        let p = PathBuf::from(custom);
        return p.is_file().then_some(p);
    }
    for base in bases {
        for rel in [Path::new("bin").join(platform_dir()).join(exe), Path::new("bin").join(exe)] {
            let p = base.join(rel);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(exe)).find(|p| p.is_file()))
}

/// `exe` from bin/ next to this program only: no setting, no download, no PATH (the elevated TUN helper must
/// not run a program an unelevated process could have replaced)
pub fn find_bundled(exe: &str) -> Option<PathBuf> {
    default_bases().iter().find_map(|base| {
        [Path::new("bin").join(platform_dir()).join(exe), Path::new("bin").join(exe)]
            .into_iter()
            .map(|r| base.join(r))
            .find(|p| p.is_file())
    })
}

/// Where bundled engines are looked for: the folder of this executable and its parents (development builds run
/// from native/target/<profile>/, the engines are in the repository's bin/).
pub fn default_bases() -> Vec<PathBuf> {
    let Ok(exe) = std::env::current_exe() else { return vec![] };
    exe.ancestors().skip(1).take(5).map(Path::to_path_buf).collect()
}

/// the sing-box executable: setting singbox_path, else the downloaded newest release when the experimental
/// setting singbox_latest is on (and one was downloaded), else bundled, else PATH
pub fn find_binary(glob: &serde_json::Map<String, serde_json::Value>) -> Option<PathBuf> {
    let custom = glob.get("singbox_path").and_then(|v| v.as_str()).unwrap_or("");
    if custom.is_empty()
        && glob.get("singbox_latest").and_then(|v| v.as_bool()) == Some(true)
        && let Some((_, exe)) = latest::installed()
    {
        return Some(exe);
    }
    find(EXE, custom, &default_bases())
}

/// the official Hysteria2 core (setting hysteria_path, bundled, PATH)
pub fn find_hysteria(glob: &serde_json::Map<String, serde_json::Value>) -> Option<PathBuf> {
    find(HY_EXE, glob.get("hysteria_path").and_then(|v| v.as_str()).unwrap_or(""), &default_bases())
}

/// which native program runs a card
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// one sing-box process
    SingBox,
    /// the official Hysteria2 core in front of the card's sing-box (a Hysteria2 server open to other PCs)
    Hysteria,
}

/// every outbound of a chain, including the chains of a group's members (Python `_chain`)
fn chain<'a>(ob: &'a serde_json::Value, out: &mut Vec<&'a serde_json::Value>) {
    let mut cur = Some(ob);
    while let Some(o) = cur.filter(|o| o.is_object()) {
        out.push(o);
        for m in o["members"].as_array().into_iter().flatten() {
            chain(&m["outbound"], out);
        }
        cur = o.get("via").filter(|v| !v.is_null());
    }
}

/// every outbound a compiled card can use: its own chain and every router rule's
pub fn all_outbounds(proxy: &serde_json::Value) -> Vec<&serde_json::Value> {
    let mut out = vec![];
    chain(&proxy["outbound"], &mut out);
    for r in proxy["routes"].as_array().into_iter().flatten() {
        chain(&r["outbound"], &mut out);
    }
    out
}

/// an HTTPS / Hysteria2 outbound that pins its server certificate (pin_sha256)
pub fn uses_certificate_pin(proxy: &serde_json::Value) -> bool {
    all_outbounds(proxy).iter().any(|ob| {
        matches!(ob["type"].as_str(), Some("https" | "hysteria2"))
            && ob["pin_sha256"].as_str().is_some_and(|p| !p.is_empty())
    })
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || guard::is_loopback_ip(host)
}

/// The program that runs a card and the sing-box to use, or why the card cannot run (Python `engine_for`; the Rust
/// app has no built-in engine, so a card the Python app ran on it cannot run here).
pub fn engine_for(
    proxy: &serde_json::Value,
    glob: &serde_json::Map<String, serde_json::Value>,
) -> Result<(Kind, PathBuf), String> {
    use crate::i18n::tr;
    let exe = find_binary(glob).ok_or_else(|| tr("sing-box was not found"))?;
    if uses_certificate_pin(proxy) && !version::of(&exe).is_some_and(|v| v.has_certificate_pin()) {
        return Err(tr("certificate pinning needs sing-box 1.15 or later"));
    }
    let ib = &proxy["inbound"];
    if ib["type"] == "hysteria2" && !is_loopback_host(ib["host"].as_str().unwrap_or("")) {
        if find_hysteria(glob).is_none() {
            return Err(tr(
                "a Hysteria2 server open to other PCs needs the official Hysteria2 core, which was not found",
            ));
        }
        return Ok((Kind::Hysteria, exe));
    }
    Ok((Kind::SingBox, exe))
}

/// The version in `sing-box version` ("sing-box version 1.14.4") or `hysteria version` ("Version:\tv2.14.0")
/// output (Python `parse_core_version`)
pub fn parse_core_version(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find("version") {
        let rest = &text[from + i + 7..];
        let gap = rest.len() - rest.trim_start_matches([':', ' ', '\t', '\r', '\n']).len();
        if gap > 0 {
            let v = rest[gap..].strip_prefix(['v', 'V']).unwrap_or(&rest[gap..]);
            let end = v.find(|c: char| !(c.is_alphanumeric() || "._+-".contains(c))).unwrap_or(v.len());
            let cand = &v[..end];
            let mut parts = cand.splitn(3, '.');
            let digits = |s: Option<&str>| s.is_some_and(|p| p.chars().next().is_some_and(|c| c.is_ascii_digit()));
            let (a, b, c) = (parts.next(), parts.next(), parts.next());
            if a.is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
                && b.is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
                && digits(c)
            {
                return Some(cand.to_string());
            }
        }
        from += i + 7;
    }
    None
}

/// Run `<exe> version` and read its version number (Python `core_version`; blocks, call it off the UI thread)
pub fn core_version(exe: &Path) -> Option<String> {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("version").stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().ok()?;
    parse_core_version(&format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[cfg(test)]
mod version_text_tests {
    #[test]
    fn core_versions() {
        use super::parse_core_version as p;
        assert_eq!(p("sing-box version 1.14.4\n\nEnvironment: go1.24").as_deref(), Some("1.14.4"));
        assert_eq!(p("Version:\tv2.6.1\nBuildDate:").as_deref(), Some("2.6.1"));
        assert_eq!(p("sing-box version 1.15.0-alpha.13").as_deref(), Some("1.15.0-alpha.13"));
        assert_eq!(p("no version here"), None);
    }
}
