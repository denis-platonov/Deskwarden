use std::path::PathBuf;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};
use zeroize::Zeroize;

/// The DPAPI-wrapped Bitwarden session token for **one account**, at a path its
/// caller chooses.
///
/// Taking the path rather than resolving it is what makes this per-account: the
/// path is `accounts::session_path_for(config_dir, id)`, inside that account's
/// own directory. There is deliberately no constructor that resolves the config
/// directory itself — a `SessionStore` that knew where to put its own file
/// would be a second definition of the layout, and any account whose copy
/// resolved back to the old `<config_dir>\session.bin` would find, overwrite
/// and delete every other account's token.
pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    /// `path` is one account's `session.bin`; see the type's own doc. Its
    /// parent directory must already exist — the account directory is created
    /// when the account is (`accounts`/`migration`), not lazily here, so that
    /// a token can never be the thing that brings an account directory into
    /// being.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The `session.bin` this store reads and writes.
    ///
    /// Exists so an account switch's re-point is observable *at the moment the
    /// switch's sequence runs*, rather than only at the end. A switch that
    /// authenticated the new account while the store still addressed the old
    /// one would write the new session token over the account the user is
    /// leaving — a mutation no end-state assertion catches, because by then the
    /// store has been re-pointed anyway.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn save(&self, token: &str) -> std::io::Result<()> {
        let protected =
            protect(token.as_bytes()).map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        std::fs::write(&self.path, protected)
    }

    pub fn load(&self) -> Option<String> {
        let bytes = std::fs::read(&self.path).ok()?;
        let mut plain = unprotect(&bytes).ok()?;
        // Copy out, then wipe the intermediate plaintext buffer rather than
        // letting a decrypted session token linger in freed heap memory.
        let token = std::str::from_utf8(&plain).ok().map(|s| s.to_string());
        plain.zeroize();
        token
    }
}

/// DPAPI-wraps `data` for the current Windows user. `pub(crate)` because
/// `hello.rs` uses the same at-rest wrapping for its sealed blob.
pub(crate) fn protect(data: &[u8]) -> windows::core::Result<Vec<u8>> {
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

/// Inverse of [`protect`]; see there for why it is `pub(crate)`.
pub(crate) fn unprotect(data: &[u8]) -> windows::core::Result<Vec<u8>> {
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
        // Wipe DPAPI's own output buffer before handing it back to the heap:
        // `LocalFree` does not clear it, so the decrypted token would otherwise
        // stay readable in freed memory.
        std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize).zeroize();
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
            "deskwarden-test-session-{}.bin",
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
        let path = temp_dir().join("deskwarden-test-session-does-not-exist.bin");
        let store = SessionStore::new(path);
        assert_eq!(store.load(), None);
    }
}
