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

#[cfg(windows)]
fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

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
    use windows_sys::Win32::NetworkManagement::WNet::WNetGetConnectionW;
    let local = wide(format!("{letter}:").as_ref());
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let r = unsafe { WNetGetConnectionW(local.as_ptr(), buf.as_mut_ptr(), &mut len) };
    if r != 0 {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buf[..n])).filter(|s| !s.is_empty())
}

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

#[cfg(not(windows))]
pub fn free_space(_p: &Path) -> Option<(u64, u64)> {
    None
}

/// Root of the drive/share containing `p` (`C:\`, `\\server\share\`, `/`).
pub fn root_of(p: &Path) -> PathBuf {
    // std does not treat a bare `\\server` as a UNC root (it has no share)
    if let Some(server) = crate::location::unc_server(p) {
        return crate::location::server_path(&server);
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
