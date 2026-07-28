use std::path::PathBuf;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn save(&self, token: &str) -> std::io::Result<()> {
        let protected =
            protect(token.as_bytes()).map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        std::fs::write(&self.path, protected)
    }

    pub fn load(&self) -> Option<String> {
        let bytes = std::fs::read(&self.path).ok()?;
        let plain = unprotect(&bytes).ok()?;
        String::from_utf8(plain).ok()
    }
}

fn protect(data: &[u8]) -> windows::core::Result<Vec<u8>> {
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        CryptProtectData(
            &input,
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData as _));
        Ok(result)
    }
}

fn unprotect(data: &[u8]) -> windows::core::Result<Vec<u8>> {
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData as _));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[test]
    fn round_trips_a_token_through_dpapi_and_disk() {
        let path = temp_dir().join(format!(
            "nodewarden-test-session-{}.bin",
            std::process::id()
        ));
        let store = SessionStore::new(path.clone());

        store.save("super-secret-session-token").unwrap();
        let loaded = store.load();

        assert_eq!(loaded.as_deref(), Some("super-secret-session-token"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn load_returns_none_when_file_missing() {
        let path = temp_dir().join("nodewarden-test-session-does-not-exist.bin");
        let store = SessionStore::new(path);
        assert_eq!(store.load(), None);
    }
}
