//! The user's `PATH`, as the install and uninstall edit it.
//!
//! On Windows this is the per-user `Path` value in the registry
//! (`HKCU\Environment`), changed in place and announced to running programs.
//! Elsewhere swarmux does not edit shell start-up files; the install says
//! what to add instead.

use std::path::Path;

use swarmux_application::dist::DistResult;

/// Whether `dir` is on the `PATH` this process sees (or, on Windows, on the
/// user's stored one).
pub fn has(dir: &Path) -> bool {
    let dir = dir.to_string_lossy();
    let sep = if cfg!(windows) { ';' } else { ':' };
    let fold = cfg!(windows);
    let live = std::env::var("PATH").unwrap_or_default();
    swarmux_domain::dist::path_list_has(&live, &dir, sep, fold) || imp::stored_has(&dir)
}

/// Adds `dir` to the user's stored `PATH`. False when not supported here
/// or when it was there already.
pub fn add(dir: &Path) -> DistResult<bool> {
    imp::add(&dir.to_string_lossy())
}

pub fn remove(dir: &Path) -> DistResult<bool> {
    imp::remove(&dir.to_string_lossy())
}

#[cfg(not(windows))]
mod imp {
    use swarmux_application::dist::DistResult;

    pub fn stored_has(_dir: &str) -> bool {
        false
    }
    pub fn add(_dir: &str) -> DistResult<bool> {
        Ok(false)
    }
    pub fn remove(_dir: &str) -> DistResult<bool> {
        Ok(false)
    }
}

#[cfg(windows)]
mod imp {
    use std::ptr::{null, null_mut};

    use swarmux_application::dist::{DistError, DistResult};
    use swarmux_domain::dist::{path_list_has, path_list_with, path_list_without};
    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_SZ, RegCloseKey,
        RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: the handle was opened by RegOpenKeyExW and is closed once.
            unsafe { RegCloseKey(self.0) };
        }
    }

    fn open(access: u32) -> Result<Key, u32> {
        let mut key: HKEY = null_mut();
        // SAFETY: valid NUL-terminated name and out pointer.
        let rc = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide("Environment").as_ptr(),
                0,
                access,
                &mut key,
            )
        };
        if rc == 0 { Ok(Key(key)) } else { Err(rc) }
    }

    /// The stored `Path` text and its registry type; empty when absent.
    fn read(key: &Key) -> Result<(String, u32), u32> {
        let name = wide("Path");
        let mut kind = 0u32;
        let mut len = 0u32;
        // SAFETY: size query with a null buffer.
        let rc = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                null(),
                &mut kind,
                null_mut(),
                &mut len,
            )
        };
        if rc == ERROR_FILE_NOT_FOUND {
            return Ok((String::new(), REG_EXPAND_SZ));
        }
        if rc != 0 {
            return Err(rc);
        }
        let mut buf = vec![0u16; (len as usize).div_ceil(2) + 1];
        let mut got = (buf.len() * 2) as u32;
        // SAFETY: buf is writable for `got` bytes.
        let rc = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                null(),
                &mut kind,
                buf.as_mut_ptr().cast(),
                &mut got,
            )
        };
        if rc != 0 {
            return Err(rc);
        }
        let units = (got as usize / 2).min(buf.len());
        let text = String::from_utf16_lossy(&buf[..units]);
        Ok((text.trim_end_matches('\0').to_string(), kind))
    }

    fn write(key: &Key, text: &str, kind: u32) -> Result<(), u32> {
        let kind = if kind == REG_SZ {
            REG_SZ
        } else {
            REG_EXPAND_SZ
        };
        let data = wide(text);
        // SAFETY: data is valid for its byte length.
        let rc = unsafe {
            RegSetValueExW(
                key.0,
                wide("Path").as_ptr(),
                0,
                kind,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        if rc == 0 { Ok(()) } else { Err(rc) }
    }

    fn announce() {
        let env = wide("Environment");
        let mut result = 0usize;
        // SAFETY: broadcast with a valid NUL-terminated string; a program
        // that does not answer within the timeout is skipped.
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                env.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                2000,
                &mut result,
            );
        }
    }

    fn fail(what: &str, code: u32) -> DistError {
        DistError::Io(format!(
            "cannot {what} the user PATH (registry error {code})"
        ))
    }

    pub fn stored_has(dir: &str) -> bool {
        open(KEY_READ)
            .and_then(|k| read(&k))
            .is_ok_and(|(text, _)| path_list_has(&text, dir, ';', true))
    }

    pub fn add(dir: &str) -> DistResult<bool> {
        let key = open(KEY_READ | KEY_WRITE).map_err(|c| fail("open", c))?;
        let (text, kind) = read(&key).map_err(|c| fail("read", c))?;
        let Some(new) = path_list_with(&text, dir, ';', true) else {
            return Ok(false);
        };
        write(&key, &new, kind).map_err(|c| fail("write", c))?;
        announce();
        Ok(true)
    }

    pub fn remove(dir: &str) -> DistResult<bool> {
        let key = open(KEY_READ | KEY_WRITE).map_err(|c| fail("open", c))?;
        let (text, kind) = read(&key).map_err(|c| fail("read", c))?;
        let Some(new) = path_list_without(&text, dir, ';', true) else {
            return Ok(false);
        };
        write(&key, &new, kind).map_err(|c| fail("write", c))?;
        announce();
        Ok(true)
    }
}
