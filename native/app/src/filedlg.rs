//! Windows' own open / save file dialogs (Python `QFileDialog.getOpenFileName / getSaveFileName`). They block
//! the thread they run on, so the window runs them on a thread of their own with itself as the owner.

/// Qt's filter text ("Certificates (*.crt *.pem);;All files (*)") as Windows wants it: label and patterns
/// pairs, each ending with a NUL, the list with one more
pub fn win_filter(qt: &str) -> Vec<u16> {
    let mut out = String::new();
    for part in qt.split(";;").map(str::trim).filter(|p| !p.is_empty()) {
        let patterns =
            part.rfind('(').and_then(|a| part[a + 1..].find(')').map(|b| &part[a + 1..a + 1 + b])).unwrap_or("*");
        let patterns: Vec<&str> = patterns.split_whitespace().map(|p| if p == "*" { "*.*" } else { p }).collect();
        out.push_str(part);
        out.push('\0');
        out.push_str(&patterns.join(";"));
        out.push('\0');
    }
    out.push('\0');
    out.encode_utf16().collect()
}

#[cfg(windows)]
fn run(owner: usize, title: &str, filter: &str, name: &str, save: bool) -> Option<std::path::PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, GetSaveFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    let wide = |t: &str| t.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let filter = win_filter(filter);
    let title = wide(title);
    let mut file = vec![0u16; 4096];
    for (i, c) in name.encode_utf16().take(4000).enumerate() {
        file[i] = c;
    }
    let ext = wide(std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or(""));
    let mut o: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    o.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    o.hwndOwner = owner as _;
    o.lpstrFilter = filter.as_ptr();
    o.nFilterIndex = 1;
    o.lpstrFile = file.as_mut_ptr();
    o.nMaxFile = file.len() as u32;
    o.lpstrTitle = title.as_ptr();
    o.Flags =
        OFN_EXPLORER | OFN_NOCHANGEDIR | OFN_PATHMUSTEXIST | if save { OFN_OVERWRITEPROMPT } else { OFN_FILEMUSTEXIST };
    if save && ext.len() > 1 {
        o.lpstrDefExt = ext.as_ptr();
    }
    let ok = unsafe { if save { GetSaveFileNameW(&mut o) } else { GetOpenFileNameW(&mut o) } } != 0;
    if !ok {
        return None;
    }
    let len = file.iter().position(|c| *c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&file[..len]).into())
}

#[cfg(not(windows))]
fn run(_owner: usize, _title: &str, _filter: &str, _name: &str, _save: bool) -> Option<std::path::PathBuf> {
    None
}

/// "Open": the chosen file, or None when cancelled. `owner` = the window's HWND (0 = none).
pub fn open(owner: usize, title: &str, filter: &str) -> Option<std::path::PathBuf> {
    run(owner, title, filter, "", false)
}

/// "Save as", starting with the suggested file name; asks before replacing a file.
pub fn save(owner: usize, title: &str, filter: &str, name: &str) -> Option<std::path::PathBuf> {
    run(owner, title, filter, name, true)
}

#[cfg(test)]
mod tests {
    #[test]
    fn filter() {
        let w = String::from_utf16(&super::win_filter("Certificates (*.crt *.pem);;All files (*)")).unwrap();
        assert_eq!(w, "Certificates (*.crt *.pem)\0*.crt;*.pem\0All files (*)\0*.*\0\0");
    }
}
