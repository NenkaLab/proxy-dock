//! Password storage (Python: `config._dpapi`, `_seal`, `_unseal`). On Windows the secret fields are saved as
//! `<field>_dpapi` = base64 of a DPAPI blob for the current user; elsewhere they stay plain in a 0600 file.

use serde_json::{Map, Value};

/// Fields saved encrypted (subscription URLs carry tokens, the server list has passwords).
pub const SECRET_FIELDS: [&str; 9] = [
    "password",
    "ss_password",
    "hy2_password",
    "hy2_obfs_password",
    "wg_private_key",
    "wg_preshared_key",
    "wg_peers",
    "sub_url",
    "sub_servers",
];

#[derive(Debug)]
pub struct DpapiError(pub u32);

#[cfg(windows)]
fn dpapi(data: &[u8], protect: bool) -> Result<Vec<u8>, DpapiError> {
    use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    use std::ptr::{null, null_mut};
    let ok = unsafe {
        if protect {
            CryptProtectData(&input, null(), null(), null(), null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        } else {
            CryptUnprotectData(&input, null_mut(), null(), null(), null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        }
    };
    if ok == 0 {
        return Err(DpapiError(unsafe { GetLastError() }));
    }
    let out = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    unsafe { LocalFree(output.pbData as _) };
    Ok(out)
}

#[cfg(windows)]
pub fn protect(plain: &str) -> Result<String, DpapiError> {
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.encode(dpapi(plain.as_bytes(), true)?))
}

#[cfg(windows)]
pub fn unprotect(encoded: &str) -> Option<String> {
    use base64::Engine;
    let blob = base64::engine::general_purpose::STANDARD.decode(encoded.trim()).ok()?;
    String::from_utf8(dpapi(&blob, false).ok()?).ok()
}

/// A secret that could not be decrypted (another user / PC): left empty, the user enters it again.
#[derive(Debug, Clone, PartialEq)]
pub struct Undecryptable {
    pub field: String,
}

/// Saved dict -> plain dict. Returns (dict, legacy, failures); legacy = a secret was stored unencrypted (the file
/// should be saved again so it gets encrypted).
pub fn unseal(d: &Value) -> (Value, bool, Vec<Undecryptable>) {
    let Value::Object(map) = d else { return (d.clone(), false, vec![]) };
    let mut out = map.clone();
    let mut legacy = false;
    let mut failed = vec![];
    for k in SECRET_FIELDS {
        let enc = out.shift_remove(&format!("{k}_dpapi"));
        match enc {
            Some(Value::String(s)) if !s.is_empty() => {
                #[cfg(windows)]
                let plain = unprotect(&s);
                #[cfg(not(windows))]
                let plain: Option<String> = {
                    let _ = s;
                    Some(String::new())
                };
                match plain {
                    Some(p) => {
                        out.insert(k.to_string(), Value::String(p));
                    }
                    None => {
                        failed.push(Undecryptable { field: k.to_string() });
                        out.insert(k.to_string(), Value::String(String::new()));
                    }
                }
            }
            _ => {
                if cfg!(windows) && out.get(k).is_some_and(truthy) {
                    legacy = true;
                }
            }
        }
    }
    (Value::Object(out), legacy, failed)
}

/// Plain dict -> dict to save (Windows: secret fields become `<field>_dpapi`).
pub fn seal(d: &Map<String, Value>) -> Result<Map<String, Value>, DpapiError> {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut out = d.clone();
    if !cfg!(windows) {
        return Ok(out);
    }
    #[cfg(windows)]
    for k in SECRET_FIELDS {
        if let Some(v) = out.shift_remove(k) {
            let plain = v.as_str().unwrap_or("");
            let enc = if plain.is_empty() { String::new() } else { protect(plain)? };
            out.insert(format!("{k}_dpapi"), Value::String(enc));
        }
    }
    Ok(out)
}

/// Python truthiness of a JSON value
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn seal_then_unseal_round_trips() {
        let plain = json!({"type": "shadowsocks", "ss_password": "비밀 pass", "password": ""});
        let sealed = seal(plain.as_object().unwrap()).unwrap();
        assert!(sealed.get("ss_password").is_none());
        assert_ne!(sealed["ss_password_dpapi"], json!(""));
        assert_eq!(sealed["password_dpapi"], json!(""));
        let (back, legacy, failed) = unseal(&Value::Object(sealed));
        assert!(!legacy && failed.is_empty());
        assert_eq!(back["ss_password"], json!("비밀 pass"));
    }

    #[test]
    fn plain_secret_is_legacy() {
        let (_, legacy, _) = unseal(&json!({"password": "x"}));
        assert!(legacy);
    }

    #[test]
    fn foreign_blob_is_reported_and_emptied() {
        let (back, _, failed) = unseal(&json!({"password_dpapi": "AAAA"}));
        assert_eq!(failed, vec![Undecryptable { field: "password".into() }]);
        assert_eq!(back["password"], json!(""));
    }
}
