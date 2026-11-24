//! Native engines (Python: `proxydock/singbox/`). Each running proxy card is one sing-box process.

pub mod compile;
pub mod guard;
pub mod process;

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

/// Where bundled engines are looked for: the folder of this executable and its parents (development builds run
/// from native/target/<profile>/, the engines are in the repository's bin/).
pub fn default_bases() -> Vec<PathBuf> {
    let Ok(exe) = std::env::current_exe() else { return vec![] };
    exe.ancestors().skip(1).take(5).map(Path::to_path_buf).collect()
}

/// the sing-box executable (setting singbox_path, bundled, PATH)
pub fn find_binary(glob: &serde_json::Map<String, serde_json::Value>) -> Option<PathBuf> {
    find(EXE, glob.get("singbox_path").and_then(|v| v.as_str()).unwrap_or(""), &default_bases())
}

/// the official Hysteria2 core (setting hysteria_path, bundled, PATH)
pub fn find_hysteria(glob: &serde_json::Map<String, serde_json::Value>) -> Option<PathBuf> {
    find(HY_EXE, glob.get("hysteria_path").and_then(|v| v.as_str()).unwrap_or(""), &default_bases())
}
