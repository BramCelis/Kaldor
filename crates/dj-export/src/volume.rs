//! What kind of drive the export goes to: file system, free space, partition
//! style, and the removable drives to choose from.
//!
//! baken-export reads these on macOS and Linux only; on Windows Kaldor reads
//! them itself (to warn about the file system, check free
//! space, MBR). Everything here is read-only and best effort: what cannot be
//! read is `None`, and nothing is checked for it.

use baken_export::volume::{FileSystem, PartitionTable};
use std::path::{Path, PathBuf};

/// What could be read about the volume a path is on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VolumeInfo {
    pub filesystem: Option<FileSystem>,
    /// Bytes the current user may still write.
    pub free_bytes: Option<u64>,
    pub partition_table: Option<PartitionTable>,
    /// MBR partition type id (0x0b, 0x0c = FAT32), MBR only.
    pub partition_type: Option<u8>,
    /// The volume's label, for the drive list.
    pub label: Option<String>,
}

/// A drive the export can go to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drive {
    /// `E:\`
    pub root: PathBuf,
    pub label: String,
    pub removable: bool,
}

/// The file system name as Windows reports it, to baken's type.
pub fn filesystem_from_name(name: &str) -> FileSystem {
    match name.to_ascii_uppercase().as_str() {
        "FAT" | "FAT12" | "FAT16" | "FAT32" => FileSystem::Fat,
        "EXFAT" => FileSystem::ExFat,
        "NTFS" => FileSystem::Ntfs,
        _ => FileSystem::Other(name.to_string()),
    }
}

/// Reads what it can about the volume `dir` is on.
pub fn probe(dir: &Path) -> VolumeInfo {
    imp::probe(dir)
}

/// How a drive is shown in a list: its name and letter ("Music SSD (D:)"),
/// or what kind of drive it is when it has no name ("USB drive (E:)").
/// `None` off Windows (volumes are shown by their mount name there).
pub fn display_name(root: &Path) -> Option<String> {
    imp::display_name(root)
}

/// Drives the export can go to, removable ones first.
pub fn drives() -> Vec<Drive> {
    let mut v = imp::drives();
    v.sort_by_key(|d| (!d.removable, d.root.clone()));
    v
}

/// Ejects the removable drive at `root` (`E:\`) so it can be unplugged:
/// writes out what is cached, locks and dismounts the volume, then ejects
/// the media (Windows' "Eject"). Fails while a program still has a file
/// open on it, and for anything but the root of a removable drive.
pub fn eject(root: &Path) -> Result<(), String> {
    imp::eject(root)
}

#[cfg(windows)]
mod imp {
    use super::{Drive, VolumeInfo, filesystem_from_name};
    use baken_export::volume::PartitionTable;
    use std::path::{Component, Path, PathBuf, Prefix};
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FlushFileBuffers, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives,
        GetVolumeInformationW, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{
        FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, IOCTL_DISK_GET_PARTITION_INFO_EX,
        IOCTL_STORAGE_EJECT_MEDIA, IOCTL_STORAGE_MEDIA_REMOVAL, PARTITION_INFORMATION_EX,
        PARTITION_STYLE_GPT, PARTITION_STYLE_MBR, PREVENT_MEDIA_REMOVAL,
    };
    use windows::core::PCWSTR;

    /// `GetDriveTypeW`'s value for a removable drive (a USB stick).
    const DRIVE_REMOVABLE: u32 = 2;
    /// ... and for a fixed disk (USB hard disks often report this).
    const DRIVE_FIXED: u32 = 3;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The drive letter of a path (`E` for `E:\Music`), if it has one.
    fn letter(dir: &Path) -> Option<char> {
        match dir.components().next()? {
            Component::Prefix(p) => match p.kind() {
                Prefix::Disk(b) | Prefix::VerbatimDisk(b) => Some(char::from(b)),
                _ => None,
            },
            _ => None,
        }
    }

    fn from_wide(buf: &[u16]) -> String {
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    pub fn display_name(root: &Path) -> Option<String> {
        let l = letter(root)?;
        let w = wide(&format!("{l}:\\"));
        // SAFETY: `w` is a NUL-terminated UTF-16 string for the call.
        let kind = unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) };
        let what = match kind {
            DRIVE_REMOVABLE => "USB drive",
            DRIVE_FIXED => "Local disk",
            4 => "Network drive",
            5 => "CD drive",
            _ => "Drive",
        };
        // A network drive that is not reachable can keep the call waiting:
        // only local drives are asked for their name.
        let label = if kind == DRIVE_REMOVABLE || kind == DRIVE_FIXED {
            let mut label = [0u16; 261];
            // SAFETY: `w` outlives the call; `label` is valid for its whole
            // length; the other out-parameters are not requested.
            let ok = unsafe {
                GetVolumeInformationW(PCWSTR(w.as_ptr()), Some(&mut label), None, None, None, None)
            };
            ok.is_ok()
                .then(|| from_wide(&label))
                .filter(|s| !s.trim().is_empty())
        } else {
            None
        };
        Some(format!("{} ({l}:)", label.as_deref().unwrap_or(what)))
    }

    pub fn probe(dir: &Path) -> VolumeInfo {
        let mut info = VolumeInfo::default();
        let Some(l) = letter(dir) else {
            return info;
        };
        let root = wide(&format!("{l}:\\"));
        let mut fs_name = [0u16; 64];
        let mut label = [0u16; 261];
        // SAFETY: `root` is a NUL-terminated UTF-16 string that outlives the
        // call; the two buffers are valid for their whole length, which the
        // slices pass; the other out-parameters are not requested.
        let ok = unsafe {
            GetVolumeInformationW(
                PCWSTR(root.as_ptr()),
                Some(&mut label),
                None,
                None,
                None,
                Some(&mut fs_name),
            )
        };
        if ok.is_ok() {
            info.filesystem = Some(filesystem_from_name(&from_wide(&fs_name)));
            let l = from_wide(&label);
            info.label = (!l.is_empty()).then_some(l);
        }
        let mut free = 0u64;
        // SAFETY: `root` as above; `free` is a valid u64 for the call.
        let ok = unsafe { GetDiskFreeSpaceExW(PCWSTR(root.as_ptr()), Some(&mut free), None, None) };
        if ok.is_ok() {
            info.free_bytes = Some(free);
        }
        let (table, kind) = partition(l);
        info.partition_table = table;
        info.partition_type = kind;
        info
    }

    /// Partition style and MBR type of the volume, from the volume device.
    /// Opened with no access rights (enough for this query, no admin needed).
    fn partition(l: char) -> (Option<PartitionTable>, Option<u8>) {
        let dev = wide(&format!("\\\\.\\{l}:"));
        // SAFETY: `dev` is a NUL-terminated UTF-16 string that outlives the
        // call; no security attributes or template handle are passed.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(dev.as_ptr()),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        };
        let Ok(handle) = handle else {
            return (None, None);
        };
        let mut out = PARTITION_INFORMATION_EX::default();
        let mut returned = 0u32;
        // SAFETY: `handle` is open; `out` is a properly sized and aligned
        // PARTITION_INFORMATION_EX that the call fills; no input buffer and
        // no overlapped I/O.
        let ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_DISK_GET_PARTITION_INFO_EX,
                None,
                0,
                Some((&raw mut out).cast()),
                std::mem::size_of::<PARTITION_INFORMATION_EX>() as u32,
                Some(&mut returned),
                None,
            )
        };
        // SAFETY: `handle` came from CreateFileW above and is closed once.
        unsafe {
            let _ = CloseHandle(handle);
        }
        if ok.is_err() {
            return (None, None);
        }
        if out.PartitionStyle == PARTITION_STYLE_MBR {
            // SAFETY: with the MBR style the `Mbr` member of the union is
            // the one the call filled.
            let kind = unsafe { out.Anonymous.Mbr.PartitionType };
            (Some(PartitionTable::Mbr), Some(kind))
        } else if out.PartitionStyle == PARTITION_STYLE_GPT {
            (Some(PartitionTable::Gpt), None)
        } else {
            (None, None)
        }
    }

    pub fn eject(root: &Path) -> Result<(), String> {
        let not_root = || format!("{} is not the root of a drive", root.display());
        let l = letter(root).ok_or_else(not_root)?;
        if root.components().count() > 2 {
            return Err(not_root());
        }
        let w = wide(&format!("{l}:\\"));
        // SAFETY: `w` is a NUL-terminated UTF-16 string for the call.
        if unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) } != DRIVE_REMOVABLE {
            return Err(format!(
                "{l}: is not a removable drive; use Windows' \"Safely remove\""
            ));
        }
        let dev = wide(&format!("\\\\.\\{l}:"));
        // SAFETY: as in `partition`; read and write access is needed to
        // flush, lock and dismount the volume.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(dev.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        }
        .map_err(|e| format!("cannot open {l}: ({e})"))?;
        let ioctl = |code: u32, input: Option<&PREVENT_MEDIA_REMOVAL>| {
            let mut returned = 0u32;
            let (ptr, len) = match input {
                Some(p) => (
                    Some(std::ptr::from_ref(p).cast()),
                    std::mem::size_of::<PREVENT_MEDIA_REMOVAL>() as u32,
                ),
                None => (None, 0),
            };
            // SAFETY: `handle` is open; the input, when given, is a valid
            // PREVENT_MEDIA_REMOVAL for its size; no output buffer, no
            // overlapped I/O.
            unsafe { DeviceIoControl(handle, code, ptr, len, None, 0, Some(&mut returned), None) }
        };
        let result = (|| {
            // SAFETY: `handle` is an open volume handle.
            unsafe { FlushFileBuffers(handle) }
                .map_err(|e| format!("flushing {l}: failed ({e})"))?;
            // A program may hold a file open for a moment: try a few times.
            let mut locked = ioctl(FSCTL_LOCK_VOLUME, None);
            for _ in 0..4 {
                if locked.is_ok() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
                locked = ioctl(FSCTL_LOCK_VOLUME, None);
            }
            locked.map_err(|_| {
                format!("{l}: is in use (a window or program has a file open on it); close it and try again")
            })?;
            ioctl(FSCTL_DISMOUNT_VOLUME, None)
                .map_err(|e| format!("dismounting {l}: failed ({e})"))?;
            let allow = PREVENT_MEDIA_REMOVAL {
                PreventMediaRemoval: false,
            };
            ioctl(IOCTL_STORAGE_MEDIA_REMOVAL, Some(&allow))
                .map_err(|e| format!("{l}: removal not allowed ({e})"))?;
            ioctl(IOCTL_STORAGE_EJECT_MEDIA, None)
                .map_err(|e| format!("ejecting {l}: failed ({e})"))
        })();
        // SAFETY: `handle` came from CreateFileW above and is closed once
        // (which also unlocks the volume if a later step failed).
        unsafe {
            let _ = CloseHandle(handle);
        }
        result
    }

    pub fn drives() -> Vec<Drive> {
        // SAFETY: no arguments; returns a bit mask of drive letters.
        let mask = unsafe { GetLogicalDrives() };
        let mut out = Vec::new();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let l = char::from(b'A' + i as u8);
            let root = format!("{l}:\\");
            let w = wide(&root);
            // SAFETY: `w` is a NUL-terminated UTF-16 string for the call.
            let kind = unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) };
            if kind != DRIVE_REMOVABLE && kind != DRIVE_FIXED {
                continue;
            }
            // The system drive is never an export target.
            if std::env::var("SystemDrive").is_ok_and(|s| s.eq_ignore_ascii_case(&format!("{l}:")))
            {
                continue;
            }
            let info = probe(Path::new(&root));
            out.push(Drive {
                root: PathBuf::from(&root),
                label: info.label.unwrap_or_default(),
                removable: kind == DRIVE_REMOVABLE,
            });
        }
        out
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Drive, VolumeInfo};
    use std::path::Path;

    /// On macOS and Linux baken-export reads the volume itself.
    pub fn probe(_: &Path) -> VolumeInfo {
        VolumeInfo::default()
    }

    pub fn drives() -> Vec<Drive> {
        Vec::new()
    }

    pub fn display_name(_: &Path) -> Option<String> {
        None
    }

    pub fn eject(_: &Path) -> Result<(), String> {
        Err("eject the stick in Finder or your file manager".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_names() {
        assert_eq!(filesystem_from_name("FAT32"), FileSystem::Fat);
        assert_eq!(filesystem_from_name("exFAT"), FileSystem::ExFat);
        assert_eq!(filesystem_from_name("NTFS"), FileSystem::Ntfs);
        assert_eq!(
            filesystem_from_name("ReFS"),
            FileSystem::Other("ReFS".into())
        );
    }

    /// Only the root of a removable drive is ejected; nothing is opened for
    /// these (the checks come first).
    #[cfg(windows)]
    #[test]
    fn eject_refuses_folders_and_fixed_drives() {
        let tmp = std::env::temp_dir();
        assert!(eject(&tmp.join("x")).unwrap_err().contains("not the root"));
        assert!(
            eject(Path::new("relative"))
                .unwrap_err()
                .contains("not the root")
        );
        let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let e = eject(Path::new(&format!("{sys}\\"))).unwrap_err();
        assert!(e.contains("not a removable drive"), "{e}");
    }

    /// The drive the project is on has a name for the list: its label, or
    /// what kind of drive it is, then its letter.
    #[cfg(windows)]
    #[test]
    fn drives_have_a_display_name() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let name = display_name(here).expect("a drive letter");
        let letter = here
            .to_string_lossy()
            .chars()
            .next()
            .unwrap()
            .to_ascii_uppercase();
        assert!(name.ends_with(&format!(" ({letter}:)")), "{name}");
        assert!(name.len() > 5, "{name}");
        assert_eq!(display_name(Path::new("relative")), None);
    }

    /// Reads the drive the temp folder is on (a real call, read-only).
    #[cfg(windows)]
    #[test]
    fn probes_the_temp_drive() {
        let info = probe(&std::env::temp_dir());
        assert!(info.filesystem.is_some(), "{info:?}");
        assert!(info.free_bytes.is_some_and(|b| b > 0), "{info:?}");
    }
}
