//! Update check against the GitHub releases of ProxyDock, and the in-app update: pick the file for this computer,
//! download it, check it against the release's SHA256SUMS.txt, run the installer (Python `update.py`).
//!
//! Only a generic User-Agent ("ProxyDock/<version>") is sent: no settings, IDs or other data.

use crate::i18n::tr;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const API_URL: &str = "https://api.github.com/repos/NenkaLab/proxy-dock/releases/latest";
pub const RELEASES_URL: &str = "https://github.com/NenkaLab/proxy-dock/releases";
/// the automatic check at startup: at most once a day
pub const CHECK_INTERVAL: i64 = 24 * 3600;
pub const SUMS_NAME: &str = "SHA256SUMS.txt";
const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_JSON: u64 = 2 * 1024 * 1024;
const MAX_SUMS: u64 = 64 * 1024;

/// this program's version (the app crate sets it at build time)
pub fn current_version() -> &'static str {
    option_env!("PROXYDOCK_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

fn user_agent() -> String {
    format!("ProxyDock/{}", current_version())
}

fn agent(timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(timeout).user_agent(user_agent()).build().into()
}

/// GET url -> bytes, at most `limit` (Python `http_get`)
fn http_get(url: &str, limit: u64, accept: &str) -> Result<Vec<u8>, String> {
    let r = agent(Some(TIMEOUT)).get(url).header("Accept", accept).call();
    let mut r = r.map_err(|e| tr("Could not reach GitHub: {e}").replace("{e}", &e.to_string()))?;
    let data = r.body_mut().with_config().limit(limit + 1).read_to_vec().map_err(|e| tr("Could not reach GitHub: {e}").replace("{e}", &e.to_string()))?;
    if data.len() as u64 > limit {
        return Err(tr("The response from GitHub is too large"));
    }
    Ok(data)
}

/// "v2.2.1" / "2.2.1" -> (2, 2, 1); None for anything else (prerelease suffixes included)
pub fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let t = tag.trim();
    let t = t.strip_prefix('v').unwrap_or(t);
    let parts: Vec<&str> = t.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    Some((parts[0].parse().ok()?, parts[1].parse().ok()?, parts[2].parse().ok()?))
}

pub fn is_newer(tag: &str, current: &str) -> bool {
    matches!((parse_version(tag), parse_version(current)), (Some(n), Some(c)) if n > c)
}

/// A published release: version, tag, name, notes (Markdown), page, files {name: (url, size)}
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub tag: String,
    pub name: String,
    pub notes: String,
    pub url: String,
    pub assets: BTreeMap<String, (String, u64)>,
}

/// GitHub release JSON -> Release, or None for a draft, a prerelease or a tag that is no plain version
pub fn parse_release(data: &Value) -> Result<Option<Release>, String> {
    let Some(d) = data.as_object() else { return Err(tr("GitHub returned an unexpected answer")) };
    if d.get("draft").is_some_and(crate::secret::truthy) || d.get("prerelease").is_some_and(crate::secret::truthy) {
        return Ok(None);
    }
    let tag = d.get("tag_name").and_then(Value::as_str).unwrap_or("").to_string();
    let Some(v) = parse_version(&tag) else { return Ok(None) };
    let mut assets = BTreeMap::new();
    for a in d.get("assets").and_then(Value::as_array).into_iter().flatten() {
        let (Some(name), Some(url)) = (a["name"].as_str(), a["browser_download_url"].as_str()) else { continue };
        if url.starts_with("https://") {
            assets.insert(name.to_string(), (url.to_string(), a["size"].as_u64().unwrap_or(0)));
        }
    }
    let url = d.get("html_url").and_then(Value::as_str).filter(|u| u.starts_with("https://")).unwrap_or("").to_string();
    Ok(Some(Release {
        version: format!("{}.{}.{}", v.0, v.1, v.2),
        name: d.get("name").and_then(Value::as_str).map(String::from).unwrap_or_else(|| tag.clone()),
        tag,
        notes: d.get("body").and_then(Value::as_str).unwrap_or("").trim().to_string(),
        url,
        assets,
    }))
}

/// the latest release if it is newer than this version, else None (Python `check`)
pub fn check() -> Result<Option<Release>, String> {
    // (debug builds: tests point it at a local server)
    let url = if cfg!(debug_assertions) { std::env::var("PROXYDOCK_UPDATE_URL").unwrap_or_else(|_| API_URL.into()) } else { API_URL.into() };
    let data = http_get(&url, MAX_JSON, "application/vnd.github+json")?;
    let v: Value = serde_json::from_slice(&data).map_err(|_| tr("GitHub returned an unexpected answer"))?;
    Ok(parse_release(&v)?.filter(|r| is_newer(&r.tag, current_version())))
}

/// the automatic startup check is due (switched on and the last one a day old) (Python `due`)
pub fn due(glob: &serde_json::Map<String, Value>, now: i64) -> bool {
    let last = glob.get("update_checked").and_then(Value::as_i64).unwrap_or(0);
    glob.get("check_updates") != Some(&Value::Bool(false)) && (now - last >= CHECK_INTERVAL || last > now)
}

/// (os name, arch) as in the release file names
pub fn platform_key() -> (&'static str, &'static str) {
    let os = if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "macos" } else { "linux" };
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    (os, arch)
}

/// How this copy updates itself (Python `current_install`): "installer" for a Windows install made by the setup
/// program (its uninstaller sits next to the program), else None (open the release page)
pub fn install_kind(exe: &Path) -> Option<&'static str> {
    (cfg!(windows) && exe.parent().is_some_and(|d| d.join("unins000.exe").is_file())).then_some("installer")
}

/// the release file to install (Python `pick_asset`)
pub fn pick_asset(rel: &Release, os: &str, arch: &str, kind: &str) -> Option<String> {
    let suffix = match kind {
        "installer" => "-setup.exe",
        "dmg" => ".dmg",
        "appimage" => ".AppImage",
        _ => return None,
    };
    let want = format!("ProxyDock-{}-{os}-{arch}{suffix}", rel.version);
    if rel.assets.contains_key(&want) {
        return Some(want);
    }
    let tail = format!("-{os}-{arch}{suffix}");
    rel.assets.keys().find(|n| n.starts_with("ProxyDock-") && n.ends_with(&tail)).cloned()
}

/// SHA256SUMS.txt -> {name: hex} (Python `parse_sums`)
pub fn parse_sums(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let l = line.trim();
        if l.len() < 66 || !l.is_char_boundary(64) {
            continue;
        }
        let (h, rest) = l.split_at(64);
        if !h.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let Some(name) = rest.strip_prefix("  ").or_else(|| rest.strip_prefix(" *")) else { continue };
        if !name.trim().is_empty() {
            out.insert(name.trim().to_string(), h.to_lowercase());
        }
    }
    out
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub enum FetchError {
    Cancelled,
    Failed(String),
}

/// Download release file `name` into `folder` and check it against SHA256SUMS.txt (Python `fetch_verified`).
/// `progress(done, total)`; `cancelled()` stops it. A bad or partial file is deleted.
pub fn fetch_verified(
    rel: &Release,
    name: &str,
    folder: &Path,
    progress: &dyn Fn(u64, u64),
    cancelled: &dyn Fn() -> bool,
) -> Result<PathBuf, FetchError> {
    let fail = FetchError::Failed;
    let Some((sums_url, _)) = rel.assets.get(SUMS_NAME) else {
        return Err(fail(tr("This release has no {sums}, so its files cannot be verified").replace("{sums}", SUMS_NAME)));
    };
    let sums = parse_sums(&String::from_utf8_lossy(&http_get(sums_url, MAX_SUMS, "application/octet-stream").map_err(fail)?));
    let not_listed = || fail(tr("{name} is not listed in {sums}, so it cannot be verified").replace("{name}", name).replace("{sums}", SUMS_NAME));
    let Some(want) = sums.get(name).cloned() else { return Err(not_listed()) };
    let (url, size) = rel.assets.get(name).cloned().ok_or_else(not_listed)?;
    let base = Path::new(name).file_name().map(|b| b.to_os_string()).unwrap_or_default();
    let path = folder.join(base);
    let result = (|| {
        let failed = |e: String| fail(tr("Download failed: {e}").replace("{e}", &e));
        let mut r = agent(None).get(&url).header("Accept", "application/octet-stream").call().map_err(|e| failed(e.to_string()))?;
        let total = if size > 0 { size } else { r.body().content_length().unwrap_or(0) };
        let mut body = r.body_mut().with_config().limit(u64::MAX).reader();
        let mut f = std::fs::File::create(&path).map_err(|e| failed(e.to_string()))?;
        let (mut done, mut last) = (0u64, std::time::Instant::now());
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            if cancelled() {
                return Err(FetchError::Cancelled);
            }
            let n = body.read(&mut buf).map_err(|e| failed(e.to_string()))?;
            if n == 0 {
                break;
            }
            f.write_all(&buf[..n]).map_err(|e| failed(e.to_string()))?;
            done += n as u64;
            if last.elapsed() >= Duration::from_millis(100) {
                last = std::time::Instant::now();
                progress(done, total);
            }
        }
        if total > 0 && done != total {
            return Err(failed(tr("the file is incomplete")));
        }
        progress(done, done);
        drop(f);
        if sha256_file(&path).map_err(|e| failed(e.to_string()))? != want {
            return Err(fail(tr("The downloaded file is damaged or was changed (its SHA-256 does not match)")));
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(path),
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            Err(e)
        }
    }
}

/// inside Program Files (an all-users install: setup asks for administrator rights) (Python `under_program_files`)
pub fn under_program_files(path: &Path) -> bool {
    let p = path.display().to_string().to_lowercase().trim_end_matches('\\').to_string() + "\\";
    ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"].iter().any(|k| {
        std::env::var(k).ok().is_some_and(|b| !b.is_empty() && p.starts_with(&(b.to_lowercase().trim_end_matches('\\').to_string() + "\\")))
    })
}

/// The installer's command line (Python `installer_command`): silent with progress, into the current folder for the
/// same users, closing the running app, and starting ProxyDock again when done (/RELAUNCH=1: it then opens its
/// window, as it is started without --hidden)
pub fn installer_command(setup: &Path, app_dir: &Path, language: &str) -> Vec<String> {
    vec![
        setup.display().to_string(),
        "/SILENT".into(),
        "/SUPPRESSMSGBOXES".into(),
        "/NORESTART".into(),
        "/CLOSEAPPLICATIONS".into(),
        "/FORCECLOSEAPPLICATIONS".into(),
        if under_program_files(app_dir) { "/ALLUSERS".into() } else { "/CURRENTUSER".into() },
        format!("/DIR={}", app_dir.display()),
        format!("/LANG={language}"),
        "/RELAUNCH=1".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn versions_releases_assets_sums() {
        assert_eq!(parse_version("v2.10.3"), Some((2, 10, 3)));
        assert_eq!(parse_version("2.2.1-rc1"), None);
        assert!(is_newer("v3.0.0", "2.6.0") && !is_newer("v2.6.0", "2.6.0") && !is_newer("x", "1.0.0"));
        let rel = parse_release(&json!({
            "tag_name": "v3.1.0", "name": "ProxyDock 3.1.0", "body": " notes ", "html_url": "https://github.com/x",
            "assets": [
                {"name": "ProxyDock-3.1.0-windows-x64-setup.exe", "browser_download_url": "https://dl/a", "size": 10},
                {"name": "SHA256SUMS.txt", "browser_download_url": "https://dl/s", "size": 1},
                {"name": "evil.exe", "browser_download_url": "http://dl/e"},
            ],
        })).unwrap().unwrap();
        assert_eq!((rel.version.as_str(), rel.notes.as_str(), rel.assets.len()), ("3.1.0", "notes", 2));
        assert_eq!(pick_asset(&rel, "windows", "x64", "installer").as_deref(), Some("ProxyDock-3.1.0-windows-x64-setup.exe"));
        assert_eq!(pick_asset(&rel, "windows", "arm64", "installer"), None);
        assert_eq!(parse_release(&json!({"tag_name": "v3.1.0", "prerelease": true})).unwrap(), None);
        let h = "a".repeat(64);
        let sums = parse_sums(&format!("{h}  ProxyDock-3.1.0-windows-x64-setup.exe\n{h} *other.zip\nbad line\n"));
        assert_eq!(sums.len(), 2);
        let glob: serde_json::Map<String, Value> = json!({"check_updates": true, "update_checked": 1000}).as_object().unwrap().clone();
        assert!(due(&glob, 1000 + CHECK_INTERVAL) && !due(&glob, 1001));
        let off: serde_json::Map<String, Value> = json!({"check_updates": false}).as_object().unwrap().clone();
        assert!(!due(&off, 1 << 40));
        let cmd = installer_command(Path::new(r"C:\t\setup.exe"), Path::new(r"C:\Users\u\AppData\Local\ProxyDock"), "ko");
        assert_eq!(cmd.last().map(String::as_str), Some("/RELAUNCH=1"));
        assert!(cmd.contains(&"/LANG=ko".to_string()));
    }

    /// a server on loopback that answers each request with the next body (HTTP/1.0, no keep-alive)
    fn serve(bodies: Vec<Vec<u8>>) -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for body in bodies {
                let Ok((mut s, _)) = l.accept() else { return };
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = s.write_all(format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes());
                let _ = s.write_all(&body);
            }
        });
        port
    }

    #[test]
    fn download_checks_the_hash() {
        let file = vec![7u8; 300_000];
        let hash: String = Sha256::digest(&file).iter().map(|b| format!("{b:02x}")).collect();
        let name = "ProxyDock-9.0.0-windows-x64-setup.exe";
        let release = |port: u16| Release {
            version: "9.0.0".into(), tag: "v9.0.0".into(), name: String::new(), notes: String::new(), url: String::new(),
            assets: [
                (SUMS_NAME.to_string(), (format!("http://127.0.0.1:{port}/sums"), 0)),
                (name.to_string(), (format!("http://127.0.0.1:{port}/file"), 0)),
            ].into_iter().collect(),
        };
        let dir = std::env::temp_dir().join(format!("pd-upd-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // good: saved, progress up to the whole size
        let port = serve(vec![format!("{hash}  {name}
").into_bytes(), file.clone()]);
        let last = std::sync::Mutex::new((0, 0));
        let got = fetch_verified(&release(port), name, &dir, &|d, t| *last.lock().unwrap() = (d, t), &|| false);
        let path = got.ok().expect("downloaded");
        assert_eq!(std::fs::read(&path).unwrap(), file);
        assert_eq!(*last.lock().unwrap(), (300_000, 300_000));
        // changed on the way: refused and deleted
        let port = serve(vec![format!("{hash}  {name}
").into_bytes(), vec![8u8; 300_000]]);
        assert!(matches!(fetch_verified(&release(port), name, &dir, &|_, _| {}, &|| false), Err(FetchError::Failed(_))));
        assert!(!path.exists());
        // not in the sums: refused before downloading
        let port = serve(vec![b"nothing".to_vec()]);
        assert!(matches!(fetch_verified(&release(port), name, &dir, &|_, _| {}, &|| false), Err(FetchError::Failed(_))));
        // cancelled
        let port = serve(vec![format!("{hash}  {name}
").into_bytes(), file]);
        assert!(matches!(fetch_verified(&release(port), name, &dir, &|_, _| {}, &|| true), Err(FetchError::Cancelled)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}