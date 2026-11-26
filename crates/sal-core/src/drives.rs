//! Drive enumeration and free-space queries (replaces `drivelst.cpp` basics).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveType {
    Fixed,
    Removable,
    Network,
    CdRom,
    RamDisk,
    Unknown,
}

impl DriveType {
    /// The drive type as the drive selector shows it (translated there).
    pub fn label(self) -> &'static str {
        match self {
            DriveType::Fixed => "Local disk",
            DriveType::Removable => "Removable drive",
            DriveType::Network => "Network drive",
            DriveType::CdRom => "CD/DVD drive",
            DriveType::RamDisk => "RAM disk",
            DriveType::Unknown => "Drive",
        }
    }

    /// Whether the drive is in this computer (the drive list shows these first).
    pub fn is_local(self) -> bool {
        !matches!(self, DriveType::Network)
    }
}

#[derive(Debug, Clone)]
pub struct Drive {
    pub root: PathBuf,
    /// e.g. `C:`
    pub letter: String,
    pub label: String,
    pub kind: DriveType,
    /// For mapped network drives: the share, e.g. `\\server\share`.
    pub remote: Option<String>,
}

impl Drive {
    /// One-line description, e.g. `Z: data — Network drive (\\nas\data)`.
    pub fn describe(&self) -> String {
        let mut s = self.letter.clone();
        if !self.label.is_empty() {
            s.push(' ');
            s.push_str(&self.label);
        }
        s.push_str(" — ");
        s.push_str(self.kind.label());
        if let Some(r) = &self.remote {
            s.push_str(&format!(" ({r})"));
        }
        s
    }
}

/// Type of the volume a panel root belongs to: UNC roots (`\\server\share`)
/// are network locations, drive roots are looked up in `drives`.
pub fn kind_of_root(root: &Path, drives: &[Drive]) -> DriveType {
    let s = root.to_string_lossy();
    if s.starts_with(r"\\?\UNC\") || crate::location::unc_server(root).is_some() || crate::location::unc_share_root(root).is_some() {
        return DriveType::Network;
    }
    let letter = s.trim_start_matches(r"\\?\").chars().next().map(|c| c.to_ascii_uppercase());
    drives.iter().find(|d| d.letter.chars().next() == letter).map_or(DriveType::Unknown, |d| d.kind)
}

/// A NUL-terminated UTF-16 string for the Windows API.
#[cfg(windows)]
fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// The drives there are: letter, label, type and, for network drives, the
/// share.
#[cfg(windows)]
pub fn list() -> Vec<Drive> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = char::from(b'A' + i as u8);
        let root = PathBuf::from(format!("{letter}:\\"));
        let w = wide(root.as_os_str());
        // DRIVE_REMOVABLE=2, FIXED=3, REMOTE=4, CDROM=5, RAMDISK=6
        let kind = match unsafe { GetDriveTypeW(w.as_ptr()) } {
            2 => DriveType::Removable,
            3 => DriveType::Fixed,
            4 => DriveType::Network,
            5 => DriveType::CdRom,
            6 => DriveType::RamDisk,
            _ => DriveType::Unknown,
        };
        let mut label = [0u16; 261];
        // don't touch removable/CD media: querying an empty drive can block
        let ok = matches!(kind, DriveType::Fixed | DriveType::RamDisk)
            && unsafe {
                GetVolumeInformationW(w.as_ptr(), label.as_mut_ptr(), label.len() as u32, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0)
            } != 0;
        let label = if ok {
            let n = label.iter().position(|&c| c == 0).unwrap_or(0);
            String::from_utf16_lossy(&label[..n])
        } else {
            String::new()
        };
        let remote = if kind == DriveType::Network { remote_name(letter) } else { None };
        out.push(Drive { root, letter: format!("{letter}:"), label, kind, remote });
    }
    out
}

/// Share a mapped drive letter points to (reads the local mapping table,
/// does not touch the network).
#[cfg(windows)]
fn remote_name(letter: char) -> Option<String> {
    mapped(letter).map(|(remote, _)| remote)
}

/// The share the drive `letter` is mapped to, and whether it is connected
/// now (`false`: remembered, but not connected: Explorer's red cross);
/// `None` for a letter that isn't a mapped network drive. Reads the local
/// mapping table, doesn't touch the network.
#[cfg(windows)]
pub fn mapped(letter: char) -> Option<(String, bool)> {
    use windows_sys::Win32::NetworkManagement::WNet::WNetGetConnectionW;
    let local = wide(format!("{letter}:").as_ref());
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let r = unsafe { WNetGetConnectionW(local.as_ptr(), buf.as_mut_ptr(), &mut len) };
    // ERROR_CONNECTION_UNAVAIL: remembered, not connected (the name is given)
    let connected = match r {
        0 => true,
        1201 => false,
        _ => return None,
    };
    let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some((String::from_utf16_lossy(&buf[..n]), connected)).filter(|(s, _)| !s.is_empty())
}

/// Elsewhere no drive letters.
#[cfg(not(windows))]
pub fn mapped(_letter: char) -> Option<(String, bool)> {
    None
}

/// The drive letter of `p` (`C:\x`, `\\?\C:\x`), upper case.
pub fn letter_of(p: &Path) -> Option<char> {
    let s = p.to_string_lossy();
    let b = s.trim_start_matches(r"\\?\").as_bytes();
    (b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic()).then(|| (b[0] as char).to_ascii_uppercase())
}

/// Connect the mapped drive `letter` again, as Explorer does when one that
/// isn't connected is opened: Windows asks for a login if the server wants
/// one (over the window `owner`). `Ok(true)` connected, `Ok(false)` the user
/// cancelled.
#[cfg(windows)]
pub fn reconnect(letter: char, owner: isize) -> std::io::Result<bool> {
    #[link(name = "mpr")]
    extern "system" {
        fn WNetRestoreSingleConnectionW(hwnd: *mut std::ffi::c_void, device: *const u16, use_ui: i32) -> u32;
    }
    let device = wide(format!("{letter}:").as_ref());
    match unsafe { WNetRestoreSingleConnectionW(owner as _, device.as_ptr(), 1) } {
        0 => Ok(true),
        // ERROR_CANCELLED
        1223 => Ok(false),
        code => Err(std::io::Error::from_raw_os_error(code as i32)),
    }
}

/// Elsewhere no drive letters.
#[cfg(not(windows))]
pub fn reconnect(_letter: char, _owner: isize) -> std::io::Result<bool> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "only on Windows"))
}

/// Elsewhere the one root.
#[cfg(not(windows))]
pub fn list() -> Vec<Drive> {
    vec![Drive { root: PathBuf::from("/"), letter: "/".into(), label: String::new(), kind: DriveType::Fixed, remote: None }]
}

/// (free bytes available to the user, total bytes) for the volume holding `p`.
#[cfg(windows)]
pub fn free_space(p: &Path) -> Option<(u64, u64)> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let w = wide(p.as_os_str());
    let (mut free, mut total, mut _all_free) = (0u64, 0u64, 0u64);
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, &mut _all_free) };
    (ok != 0).then_some((free, total))
}

/// Elsewhere unknown.
#[cfg(not(windows))]
pub fn free_space(_p: &Path) -> Option<(u64, u64)> {
    None
}

/// Whether `p` is on the network: a UNC path or a mapped network drive.
/// Answered locally (`GetDriveTypeW`), without contacting the server, so
/// it is cheap even when the server is slow or gone.
pub fn is_remote(p: &Path) -> bool {
    if crate::location::virtual_parts(p).is_some() {
        return false;
    }
    let s = p.to_string_lossy();
    if s.starts_with(r"\\?\UNC\") || (s.starts_with(r"\\") && !s.starts_with(r"\\?\") && !s.starts_with(r"\\.\")) {
        return true;
    }
    #[cfg(windows)]
    {
        let b = s.trim_start_matches(r"\\?\").as_bytes();
        if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
            let root = wide(std::ffi::OsStr::new(&format!("{}:\\", b[0] as char)));
            // DRIVE_REMOTE = 4
            return unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) } == 4;
        }
    }
    false
}

/// Root of the drive/share containing `p` (`C:\`, `\\server\share\`, `/`).
pub fn root_of(p: &Path) -> PathBuf {
    // std does not treat a bare `\\server` as a UNC root (it has no share)
    if let Some(server) = crate::location::unc_server(p) {
        return crate::location::server_path(&server);
    }
    // a plugin's connection: its root
    if let Some((scheme, authority, _)) = crate::location::virtual_parts(p) {
        return crate::location::virtual_path(&scheme, &authority, &[]);
    }
    p.ancestors().last().map(Path::to_path_buf).unwrap_or_else(|| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(letter: char, kind: DriveType) -> Drive {
        Drive { root: PathBuf::from(format!("{letter}:\\")), letter: format!("{letter}:"), label: String::new(), kind, remote: None }
    }

    #[test]
    fn kinds_of_roots() {
        let list = [drive('C', DriveType::Fixed), drive('Z', DriveType::Network)];
        assert_eq!(kind_of_root(Path::new(r"C:\"), &list), DriveType::Fixed);
        assert_eq!(kind_of_root(Path::new(r"z:\"), &list), DriveType::Network);
        assert_eq!(kind_of_root(Path::new(r"\\server\share"), &list), DriveType::Network);
        assert_eq!(kind_of_root(Path::new(r"\\?\UNC\server\share"), &list), DriveType::Network);
        assert_eq!(kind_of_root(Path::new(r"\\server"), &list), DriveType::Network);
        assert_eq!(root_of(Path::new(r"\\server")), PathBuf::from(r"\\server"));
        assert_eq!(root_of(Path::new(r"\\server\share\a\b")), PathBuf::from(r"\\server\share\"));
        assert_eq!(kind_of_root(&root_of(Path::new(r"\\server\share\a")), &list), DriveType::Network);
        assert_eq!(kind_of_root(Path::new(r"\\?\C:\"), &list), DriveType::Fixed);
        assert_eq!(kind_of_root(Path::new(r"Q:\"), &list), DriveType::Unknown);
        assert!(DriveType::Fixed.is_local() && !DriveType::Network.is_local());
    }

    #[test]
    fn describe_network_drive() {
        let d = Drive { label: "data".into(), remote: Some(r"\\nas\data".into()), ..drive('Z', DriveType::Network) };
        assert_eq!(d.describe(), r"Z: data — Network drive (\\nas\data)");
        assert_eq!(drive('C', DriveType::Fixed).describe(), "C: — Local disk");
    }

    #[cfg(windows)]
    #[test]
    fn system_drive_is_listed_as_local() {
        let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).to_uppercase();
        let d = list().into_iter().find(|d| d.letter == sys).expect("system drive");
        assert_eq!(d.kind, DriveType::Fixed);
        assert!(d.remote.is_none());
    }
}
