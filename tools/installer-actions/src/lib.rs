//! Custom actions of Kaldor's Windows installer (`installer/kaldor.wxs`),
//! built into a DLL that the MSI carries and Windows Installer calls.
//!
//! `KaldorFolder`: when a folder is chosen for Kaldor (Browse… OK, and
//! Next on the folder page), Kaldor goes into a folder of its own,
//! `<chosen>\Kaldor`, unless the chosen folder is empty (or does not exist
//! yet) or is already called Kaldor. So picking `D:\Apps` installs to
//! `D:\Apps\Kaldor`, and picking a new empty folder installs right there.
//!
//! The rule is [`install_folder`]; the export only reads and sets the
//! `INSTALLDIR` path. It never fails the installation: anything that goes
//! wrong leaves the path as it was.

use std::path::Path;

/// The name of Kaldor's own folder.
pub const FOLDER: &str = "Kaldor";

/// The folder to install into for the chosen folder `chosen`, if it must
/// change: `<chosen>\Kaldor\` when `has_entries(chosen)` (the folder has
/// files or folders in it). `None`: install into `chosen` as it is (empty,
/// missing, or already a folder called Kaldor). Paths end in a backslash,
/// as Windows Installer keeps them.
pub fn install_folder(chosen: &str, has_entries: impl Fn(&Path) -> bool) -> Option<String> {
    let trimmed = chosen.trim().trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        return None;
    }
    let last = trimmed.rsplit(['\\', '/']).next().unwrap_or(trimmed);
    if last.eq_ignore_ascii_case(FOLDER) {
        return None;
    }
    // A drive root ("D:") is looked at as "D:\".
    let dir = if trimmed.ends_with(':') {
        format!("{trimmed}\\")
    } else {
        trimmed.to_string()
    };
    if !has_entries(Path::new(&dir)) {
        return None;
    }
    Some(format!("{trimmed}\\{FOLDER}\\"))
}

/// Whether a folder exists and has anything in it.
pub fn has_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some())
}

#[cfg(windows)]
mod msi {
    use windows::Win32::System::ApplicationInstallationAndServicing::{
        MSIHANDLE, MsiGetPropertyW, MsiSetTargetPathW,
    };
    use windows::core::{HSTRING, PWSTR, w};

    const ERROR_SUCCESS: u32 = 0;
    const ERROR_MORE_DATA: u32 = 234;

    /// The value of property `INSTALLDIR`.
    fn install_dir(h: MSIHANDLE) -> Option<String> {
        let mut len: u32 = 0;
        let mut empty = [0u16; 1];
        // SAFETY: `empty` holds `len + 1` (1) elements; msi.dll writes at
        // most that and returns the needed length in `len`.
        let r = unsafe {
            MsiGetPropertyW(
                h,
                w!("INSTALLDIR"),
                Some(PWSTR(empty.as_mut_ptr())),
                Some(&mut len),
            )
        };
        if r != ERROR_MORE_DATA && r != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let mut cap = len + 1;
        // SAFETY: `buf` holds `cap` elements, as told to msi.dll.
        let r = unsafe {
            MsiGetPropertyW(
                h,
                w!("INSTALLDIR"),
                Some(PWSTR(buf.as_mut_ptr())),
                Some(&mut cap),
            )
        };
        (r == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&buf[..cap as usize]))
    }

    /// The custom action (DLL entry point `KaldorFolder`).
    ///
    /// # Safety
    ///
    /// Called by Windows Installer with the handle of the running install.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn KaldorFolder(h: u32) -> u32 {
        let h = MSIHANDLE(h);
        let result = std::panic::catch_unwind(|| {
            if let Some(cur) = install_dir(h)
                && let Some(new) = super::install_folder(&cur, super::has_entries)
            {
                // SAFETY: a valid handle and two strings that live through
                // the call.
                let _ = unsafe { MsiSetTargetPathW(h, w!("INSTALLDIR"), &HSTRING::from(new)) };
            }
        });
        let _ = result;
        // Never fail the installation over this.
        ERROR_SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_with_things_in_it_gets_a_kaldor_folder() {
        let full = |_: &Path| true;
        let empty = |_: &Path| false;
        assert_eq!(
            install_folder(r"D:\Apps\", full).as_deref(),
            Some(r"D:\Apps\Kaldor\")
        );
        assert_eq!(
            install_folder(r"D:\Apps", empty),
            None,
            "empty or new: as it is"
        );
        assert_eq!(install_folder(r"C:\Program Files\Kaldor\", full), None);
        assert_eq!(install_folder(r"E:\music\KALDOR", full), None);
        assert_eq!(install_folder("D:\\", full).as_deref(), Some(r"D:\Kaldor\"));
        assert_eq!(install_folder("", full), None);
        // The drive root is looked at with its backslash.
        let seen = std::cell::RefCell::new(String::new());
        install_folder("D:", |p: &Path| {
            *seen.borrow_mut() = p.display().to_string();
            true
        });
        assert_eq!(seen.into_inner(), "D:\\");
    }

    #[test]
    fn has_entries_looks_at_the_folder() {
        let dir =
            std::env::temp_dir().join(format!("kaldor-installer-actions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!has_entries(&dir), "missing");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!has_entries(&dir), "empty");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        assert!(has_entries(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
