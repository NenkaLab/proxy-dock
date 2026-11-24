//! Experimental setting "use the latest sing-box" (singbox_latest): ProxyDock downloads the newest SagerNet
//! sing-box release, alpha / beta / rc included, into `<settings folder>/engines/sing-box/<version>/` and runs that
//! one instead of the bundled version. At most one check a day; when a check or download fails the copy already
//! there keeps working, and with none the bundled sing-box is used.
//!
//! Every archive is checked against the SHA-256 digest GitHub lists for the release asset, and the extracted
//! program must answer `sing-box version` with the release's version before it is used.

use super::version::{self, Version};
use super::{platform_dir, EXE};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// settings screen: the switch for singbox_latest, its "experimental" badge and its help text (translated with tr)
pub const SETTING_LABEL: &str = "Use the latest sing-box (alpha / beta included)";
pub const SETTING_BADGE: &str = "Experimental";
pub const SETTING_HELP: &str = "Downloads the newest sing-box release from GitHub (checked once a day, SHA-256 verified) and runs it instead of the bundled version. Pre-releases can have bugs or change behaviour. Certificate pinning needs sing-box 1.15 or later.";

pub const RELEASES_URL: &str = "https://api.github.com/repos/SagerNet/sing-box/releases?per_page=10";
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);

pub fn engines_dir() -> PathBuf {
    crate::paths::app_dir().join("engines").join("sing-box")
}

/// The sing-box release to use.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: Version,
    pub asset: String,
    pub url: String,
    /// lowercase hex SHA-256 of the archive
    pub digest: String,
    pub size: u64,
}

pub fn asset_name(version: &Version, platform: &str) -> String {
    let ext = if platform.starts_with("windows-") { "zip" } else { "tar.gz" };
    format!("sing-box-{version}-{platform}.{ext}")
}

/// The newest version (semver order, pre-releases included, drafts not) in a GitHub releases list that has an
/// archive for `platform` with a SHA-256 digest.
pub fn pick(releases: &Value, platform: &str) -> Option<Release> {
    let mut best: Option<Release> = None;
    for r in releases.as_array().into_iter().flatten() {
        if r["draft"].as_bool() == Some(true) {
            continue;
        }
        let Some(v) = r["tag_name"].as_str().and_then(Version::parse) else { continue };
        if best.as_ref().is_some_and(|b| b.version >= v) {
            continue;
        }
        let name = asset_name(&v, platform);
        let Some(a) = r["assets"].as_array().into_iter().flatten().find(|a| a["name"].as_str() == Some(&name)) else { continue };
        let digest = a["digest"].as_str().unwrap_or("").strip_prefix("sha256:").unwrap_or("").to_ascii_lowercase();
        let url = a["browser_download_url"].as_str().unwrap_or("");
        if digest.len() != 64 || !url.starts_with("https://") {
            continue;
        }
        best = Some(Release { version: v, asset: name, url: url.into(), digest, size: a["size"].as_u64().unwrap_or(0) });
    }
    best
}

/// the newest downloaded sing-box in `dir`: (version, executable)
pub fn installed_in(dir: &Path) -> Option<(Version, PathBuf)> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let v = Version::parse(&e.file_name().to_string_lossy())?;
            let exe = e.path().join(EXE);
            exe.is_file().then_some((v, exe))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
}

pub fn installed() -> Option<(Version, PathBuf)> {
    installed_in(&engines_dir())
}

/// url, Accept header -> body
pub type Fetch<'a> = &'a dyn Fn(&str, &str) -> Result<Vec<u8>, String>;

pub fn http_fetch(url: &str, accept: &str) -> Result<Vec<u8>, String> {
    let (timeout, limit) = if accept.contains("json") { (30, 16 << 20) } else { (300, 200 << 20) };
    crate::http::get(url, accept, Duration::from_secs(timeout), limit)
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// checked less than a day ago: nothing asked
    NotDue,
    /// the newest release is the one already there
    UpToDate(Version),
    /// a newer release was downloaded and checked
    Installed(Version),
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn checked_at(dir: &Path) -> u64 {
    std::fs::read_to_string(dir.join("checked")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// Check for a newer sing-box release and install it (`force`: even if checked less than a day ago).
pub fn update_in(dir: &Path, fetch: Fetch, force: bool) -> Result<Outcome, String> {
    let have = installed_in(dir);
    if !force && have.is_some() && now_secs().saturating_sub(checked_at(dir)) < CHECK_EVERY.as_secs() {
        return Ok(Outcome::NotDue);
    }
    let list: Value = serde_json::from_slice(&fetch(RELEASES_URL, "application/vnd.github+json")?).map_err(|e| e.to_string())?;
    let rel = pick(&list, &platform_dir()).ok_or_else(|| format!("no sing-box release for {}", platform_dir()))?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mark = |d: &Path| std::fs::write(d.join("checked"), now_secs().to_string());
    if have.as_ref().is_some_and(|(v, _)| *v >= rel.version) {
        let _ = mark(dir);
        return Ok(Outcome::UpToDate(have.unwrap().0));
    }
    let data = fetch(&rel.url, "application/octet-stream")?;
    let got = hex(&Sha256::digest(&data));
    if got != rel.digest {
        return Err(format!("SHA-256 mismatch for {}: expected {}, got {got}", rel.asset, rel.digest));
    }
    let target = dir.join(rel.version.to_string());
    let tmp = dir.join(format!("{}.tmp", rel.version));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let result = extract(&data, &rel.asset, &tmp).and_then(|()| {
        let exe = tmp.join(EXE);
        match version::of(&exe) {
            Some(v) if v == rel.version => Ok(()),
            other => Err(format!("{} reports version {other:?}, expected {}", rel.asset, rel.version)),
        }
    });
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
    let _ = mark(dir);
    prune(dir, 2);
    Ok(Outcome::Installed(rel.version))
}

/// update_in for the real folder and GitHub
pub fn update(force: bool) -> Result<Outcome, String> {
    update_in(&engines_dir(), &http_fetch, force)
}

/// keep the newest `keep` versions (an older one still running cannot be removed on Windows: it stays for now)
fn prune(dir: &Path, keep: usize) {
    let mut vs: Vec<(Version, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((Version::parse(&e.file_name().to_string_lossy())?, e.path())))
        .filter(|(_, p)| p.is_dir())
        .collect();
    vs.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in vs.into_iter().skip(keep) {
        let _ = std::fs::remove_dir_all(p);
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// the sing-box program and its LICENSE from the release archive, into `out`
fn extract(data: &[u8], name: &str, out: &Path) -> Result<(), String> {
    let wanted = |base: &str| base == EXE || base == "LICENSE";
    if name.ends_with(".zip") {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
        for i in 0..z.len() {
            let mut f = z.by_index(i).map_err(|e| e.to_string())?;
            let fname = f.name().map_err(|e| e.to_string())?.to_string();
            let base = Path::new(&fname).file_name().map(|b| b.to_string_lossy().to_string()).unwrap_or_default();
            if f.is_file() && wanted(&base) {
                let mut w = std::fs::File::create(out.join(&base)).map_err(|e| e.to_string())?;
                std::io::copy(&mut f, &mut w).map_err(|e| e.to_string())?;
            }
        }
    } else {
        #[cfg(not(windows))]
        {
            let mut t = tar::Archive::new(flate2::read::GzDecoder::new(data));
            for e in t.entries().map_err(|e| e.to_string())? {
                let mut e = e.map_err(|e| e.to_string())?;
                let path = e.path().map_err(|e| e.to_string())?.to_path_buf();
                let base = path.file_name().map(|b| b.to_string_lossy().to_string()).unwrap_or_default();
                if e.header().entry_type().is_file() && wanted(&base) {
                    let mut w = std::fs::File::create(out.join(&base)).map_err(|e| e.to_string())?;
                    std::io::copy(&mut e, &mut w).map_err(|e| e.to_string())?;
                }
            }
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(out.join(EXE), std::fs::Permissions::from_mode(0o755));
        }
        #[cfg(windows)]
        return Err(format!("unexpected archive {name}"));
    }
    if !out.join(EXE).is_file() {
        return Err(format!("{EXE} is not in {name}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rel(tag: &str, draft: bool, assets: &[&str]) -> Value {
        let v = tag.trim_start_matches('v');
        json!({"tag_name": tag, "draft": draft, "assets": assets.iter().map(|p| json!({
            "name": format!("sing-box-{v}-{p}"), "size": 1,
            "browser_download_url": format!("https://example.invalid/{v}/{p}"),
            "digest": format!("sha256:{}", "AB".repeat(32)),
        })).collect::<Vec<_>>()})
    }

    #[test]
    fn setting_texts_are_translated() {
        crate::i18n::set_language("ko");
        for t in [SETTING_LABEL, SETTING_BADGE, SETTING_HELP] {
            assert_ne!(crate::i18n::tr(t), t, "{t}");
        }
        crate::i18n::set_language("en");
    }

    #[test]
    fn picks_newest_with_an_asset() {
        let list = json!([
            rel("v1.14.5", false, &["windows-amd64.zip"]),
            rel("v1.16.0-alpha.1", true, &["windows-amd64.zip"]),
            rel("v1.15.0-alpha.13", false, &["windows-amd64.zip", "windows-amd64-legacy-windows-7.zip", "linux-amd64.tar.gz"]),
            rel("v1.15.0-alpha.12", false, &["windows-amd64.zip"]),
            rel("v1.15.0-beta.1", false, &["linux-amd64.tar.gz"]),
        ]);
        let r = pick(&list, "windows-amd64").unwrap();
        assert_eq!(r.version.to_string(), "1.15.0-alpha.13");
        assert_eq!(r.asset, "sing-box-1.15.0-alpha.13-windows-amd64.zip");
        assert_eq!(r.digest, "ab".repeat(32));
        assert_eq!(pick(&list, "linux-amd64").unwrap().version.to_string(), "1.15.0-beta.1");
        assert!(pick(&list, "darwin-arm64").is_none());
    }

    #[test]
    fn bad_digest_is_refused_and_nothing_installed() {
        let dir = std::env::temp_dir().join(format!("proxydock-latest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let list = json!([rel("v1.15.0-alpha.13", false, &[&format!("{}.{}", platform_dir(), if cfg!(windows) { "zip" } else { "tar.gz" })])]);
        let fetch = |url: &str, _: &str| if url == RELEASES_URL { Ok(list.to_string().into_bytes()) } else { Ok(b"not it".to_vec()) };
        let e = update_in(&dir, &fetch, false).unwrap_err();
        assert!(e.contains("SHA-256 mismatch"), "{e}");
        assert!(installed_in(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
