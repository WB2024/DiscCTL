//! Finding USB sticks: mounted removable filesystems, plus folders the user has allowed.
//!
//! Only filesystems that sit on a USB or removable device are offered, so the big data disks on
//! the same machine can never be picked by accident. (Anything else has to be allowed explicitly
//! in the settings.)

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use serde::Serialize;

use crate::error::Error;

#[derive(Debug, Clone, Serialize)]
pub struct Target {
    /// The mount point, which identifies the target.
    pub id: String,
    pub mount_point: String,
    pub label: String,
    pub device: Option<String>,
    pub fs_type: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    /// Allocation unit of the filesystem: every file takes a whole number of these.
    pub block_size: u64,
    pub read_only: bool,
    /// "usb" for a USB stick, "folder" for a folder the user allowed.
    pub kind: String,
    pub vendor: String,
    pub model: String,
    /// Files can't be bigger than 4 GiB - 1 (FAT32).
    pub max_file_bytes: Option<u64>,
    /// Names need to be safe for Windows filesystems.
    pub windows_names: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Unmounted {
    pub device: String,
    pub label: String,
    pub size_bytes: u64,
    pub fs_type: String,
    pub model: String,
}

// ── /proc/self/mountinfo ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct MountEntry {
    pub major_minor: String,
    pub mount_point: String,
    pub options: String,
    pub fs_type: String,
    pub source: String,
}

/// `\040` style escapes used for spaces and other odd characters in mount points.
fn unescape_octal(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            let v = (b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0');
            out.push(v);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

pub fn parse_mountinfo(text: &str) -> Vec<MountEntry> {
    text.lines()
        .filter_map(|line| {
            let (before, after) = line.split_once(" - ")?;
            let f: Vec<&str> = before.split(' ').collect();
            if f.len() < 6 {
                return None;
            }
            let a: Vec<&str> = after.split(' ').collect();
            Some(MountEntry {
                major_minor: f[2].to_string(),
                mount_point: unescape_octal(f[4]),
                options: f[5].to_string(),
                fs_type: a.first()?.to_string(),
                source: unescape_octal(a.get(1).copied().unwrap_or("")),
            })
        })
        .collect()
}

/// Filesystems that can hold a music collection on a stick.
fn is_storage_fs(fs: &str) -> bool {
    matches!(
        fs,
        "vfat" | "msdos" | "exfat" | "ntfs" | "ntfs3" | "fuseblk" | "ext2" | "ext3" | "ext4" | "btrfs" | "xfs" | "f2fs" | "hfsplus" | "hfs" | "apfs"
    )
}

fn sys_block_dir(major_minor: &str) -> Option<PathBuf> {
    std::fs::canonicalize(format!("/sys/dev/block/{major_minor}")).ok()
}

/// Is this block device on the USB bus (or flagged removable)?
fn is_usb_or_removable(major_minor: &str) -> bool {
    let Some(dir) = sys_block_dir(major_minor) else { return false };
    if dir.to_string_lossy().contains("/usb") {
        return true;
    }
    // removable flag lives on the whole disk, not the partition
    for d in [Some(dir.as_path()), dir.parent()].into_iter().flatten() {
        if std::fs::read_to_string(d.join("removable")).map(|s| s.trim() == "1").unwrap_or(false) {
            return true;
        }
    }
    false
}

fn read_sys(dir: &Path, rel: &str) -> String {
    for d in [Some(dir), dir.parent()].into_iter().flatten() {
        if let Ok(s) = std::fs::read_to_string(d.join(rel)) {
            let t = s.trim().to_string();
            if !t.is_empty() {
                return t;
            }
        }
    }
    String::new()
}

// ── Sizes ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FsStats {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub block_size: u64,
}

/// `df -B1 --output=size,avail` output: a header line and one line of numbers.
pub fn parse_df(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().rev().find(|l| l.trim().chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let mut it = line.split_whitespace();
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

pub fn fs_stats(path: &Path) -> Result<FsStats, Error> {
    let out = Command::new("df")
        .args(["-B1", "--output=size,avail"])
        .arg(path)
        .output()
        .map_err(|e| Error::device(format!("Could not run df: {e}")))?;
    let (total, free) = parse_df(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| Error::device(format!("Could not read the size of {}", path.display())))?;
    let block = Command::new("stat")
        .args(["-f", "-c", "%S"])
        .arg(path)
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
        .filter(|b| *b >= 512)
        .unwrap_or(4096);
    Ok(FsStats { total_bytes: total, free_bytes: free, block_size: block })
}

fn fs_rules(fs: &str) -> (Option<u64>, bool) {
    match fs {
        "vfat" | "msdos" => (Some(4 * 1024 * 1024 * 1024 - 1), true),
        "exfat" | "ntfs" | "ntfs3" | "fuseblk" | "hfsplus" | "apfs" => (None, true),
        _ => (None, false),
    }
}

// ── Listing ───────────────────────────────────────────────────────────────────

fn label_of(device: &str, mount_point: &str) -> String {
    let from_lsblk = Command::new("lsblk")
        .args(["-no", "LABEL", device])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    from_lsblk.unwrap_or_else(|| {
        Path::new(mount_point).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| mount_point.to_string())
    })
}

/// Every USB stick that is mounted, plus the folders the user allowed.
pub fn list_targets(extra_folders: &[PathBuf]) -> Vec<Target> {
    let mut out: Vec<Target> = Vec::new();
    let mounts = std::fs::read_to_string("/proc/self/mountinfo").map(|t| parse_mountinfo(&t)).unwrap_or_default();

    for m in mounts {
        if !is_storage_fs(&m.fs_type) || !is_usb_or_removable(&m.major_minor) {
            continue;
        }
        if out.iter().any(|t| t.mount_point == m.mount_point) {
            continue;
        }
        let Ok(stats) = fs_stats(Path::new(&m.mount_point)) else { continue };
        let sys = sys_block_dir(&m.major_minor);
        let (vendor, model) = match &sys {
            Some(d) => (read_sys(d, "device/vendor"), read_sys(d, "device/model")),
            None => (String::new(), String::new()),
        };
        let (max_file, windows) = fs_rules(&m.fs_type);
        out.push(Target {
            id: m.mount_point.clone(),
            label: label_of(&m.source, &m.mount_point),
            mount_point: m.mount_point.clone(),
            device: Some(m.source.clone()).filter(|s| s.starts_with("/dev/")),
            fs_type: m.fs_type.clone(),
            total_bytes: stats.total_bytes,
            free_bytes: stats.free_bytes,
            block_size: stats.block_size,
            read_only: m.options.split(',').any(|o| o == "ro"),
            kind: "usb".into(),
            vendor,
            model,
            max_file_bytes: max_file,
            windows_names: windows,
        });
    }

    // Sticks mounted inside a folder the user allowed (e.g. /usb/STICK1) are targets of their own.
    let mounts2 = std::fs::read_to_string("/proc/self/mountinfo").map(|t| parse_mountinfo(&t)).unwrap_or_default();
    for folder in extra_folders {
        let prefix = format!("{}/", folder.to_string_lossy().trim_end_matches('/'));
        for m in &mounts2 {
            if !m.mount_point.starts_with(&prefix) || !is_storage_fs(&m.fs_type) || out.iter().any(|t| t.mount_point == m.mount_point) {
                continue;
            }
            let Ok(stats) = fs_stats(Path::new(&m.mount_point)) else { continue };
            let (max_file, windows) = fs_rules(&m.fs_type);
            out.push(Target {
                id: m.mount_point.clone(),
                label: label_of(&m.source, &m.mount_point),
                mount_point: m.mount_point.clone(),
                device: Some(m.source.clone()).filter(|s| s.starts_with("/dev/")),
                fs_type: m.fs_type.clone(),
                total_bytes: stats.total_bytes,
                free_bytes: stats.free_bytes,
                block_size: stats.block_size,
                read_only: m.options.split(',').any(|o| o == "ro"),
                kind: "folder".into(),
                vendor: String::new(),
                model: String::new(),
                max_file_bytes: max_file,
                windows_names: windows,
            });
        }
    }

    for folder in extra_folders {
        let mp = folder.to_string_lossy().to_string();
        if out.iter().any(|t| t.mount_point == mp) || !folder.is_dir() {
            continue;
        }
        let Ok(stats) = fs_stats(folder) else { continue };
        let fs = filesystem_of(folder).unwrap_or_default();
        let (max_file, windows) = fs_rules(&fs);
        out.push(Target {
            id: mp.clone(),
            label: folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| mp.clone()),
            mount_point: mp,
            device: None,
            fs_type: fs,
            total_bytes: stats.total_bytes,
            free_bytes: stats.free_bytes,
            block_size: stats.block_size,
            read_only: false,
            kind: "folder".into(),
            vendor: String::new(),
            model: String::new(),
            max_file_bytes: max_file,
            windows_names: windows,
        });
    }
    out
}

/// The filesystem type a path sits on, from the mount table (longest matching mount point).
pub fn filesystem_of(path: &Path) -> Option<String> {
    let real = std::fs::canonicalize(path).ok()?;
    let mounts = std::fs::read_to_string("/proc/self/mountinfo").ok().map(|t| parse_mountinfo(&t))?;
    mounts
        .into_iter()
        .filter(|m| real.starts_with(&m.mount_point))
        .max_by_key(|m| m.mount_point.len())
        .map(|m| m.fs_type)
}

/// USB partitions that exist but aren't mounted (so can't be written to yet).
pub fn list_unmounted() -> Vec<Unmounted> {
    let Ok(out) = Command::new("lsblk").args(["-J", "-b", "-o", "NAME,PATH,SIZE,TYPE,RM,TRAN,FSTYPE,LABEL,MODEL,MOUNTPOINT"]).output() else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else { return Vec::new() };
    let mut found = Vec::new();
    for disk in v["blockdevices"].as_array().cloned().unwrap_or_default() {
        let removable = disk["rm"] == true || disk["rm"] == "1" || disk["rm"] == 1 || disk["tran"] == "usb";
        if !removable || disk["type"] != "disk" {
            continue;
        }
        let model = disk["model"].as_str().unwrap_or("").trim().to_string();
        let parts: Vec<serde_json::Value> = disk["children"].as_array().cloned().unwrap_or_else(|| vec![disk.clone()]);
        for p in parts {
            let mounted = p["mountpoint"].as_str().is_some_and(|m| !m.is_empty());
            let fs = p["fstype"].as_str().unwrap_or("").to_string();
            if mounted || !is_storage_fs(&fs) {
                continue;
            }
            found.push(Unmounted {
                device: p["path"].as_str().unwrap_or("").to_string(),
                label: p["label"].as_str().unwrap_or("").to_string(),
                size_bytes: p["size"].as_u64().or_else(|| p["size"].as_str().and_then(|s| s.parse().ok())).unwrap_or(0),
                fs_type: fs,
                model: model.clone(),
            });
        }
    }
    found
}

/// Mount a USB partition. Uses `udisksctl` (works without root on a desktop) and falls back to
/// `mount` when running as root. Returns the mount point.
pub fn mount_device(device: &str) -> Result<String, Error> {
    if !device.starts_with("/dev/") || device.contains("..") || device.contains(char::is_whitespace) {
        return Err(Error::validation("Invalid device"));
    }
    if !list_unmounted().iter().any(|u| u.device == device) {
        return Err(Error::validation("That isn't an unmounted USB partition"));
    }
    // In a container the device node may not exist even though the stick is plugged into the
    // host: the kernel still lists it in /sys, so make the node (this needs the MKNOD capability
    // and a device cgroup rule allowing block devices).
    let name = device.rsplit('/').next().unwrap_or("");
    if !Path::new(device).exists() {
        let numbers = std::fs::read_to_string(format!("/sys/class/block/{name}/dev")).unwrap_or_default();
        if let Some((maj, min)) = numbers.trim().split_once(':') {
            let out = Command::new("mknod").args([device, "b", maj, min]).output();
            match out {
                Ok(o) if o.status.success() => {}
                Ok(o) => {
                    return Err(Error::device(format!(
                        "The stick's device node ({device}) doesn't exist in here and couldn't be created: {}. Mount the stick on the host instead, or start this program with the MKNOD and SYS_ADMIN capabilities (see the README).",
                        String::from_utf8_lossy(&o.stderr).trim()
                    )));
                }
                Err(e) => return Err(Error::device(format!("Could not run mknod: {e}"))),
            }
        }
    }
    if let Ok(out) = Command::new("udisksctl").args(["mount", "-b", device]).output() {
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        if out.status.success() {
            // "Mounted /dev/sdb1 at /run/media/will/STICK"
            if let Some(idx) = text.find(" at ") {
                return Ok(text[idx + 4..].trim().trim_end_matches('.').to_string());
            }
        }
    }
    let label = list_unmounted().iter().find(|u| u.device == device).map(|u| u.label.clone()).filter(|l| !l.is_empty());
    let dir = format!("/run/media/rustydisc/{}", label.unwrap_or_else(|| name.to_string()).replace('/', "_"));
    std::fs::create_dir_all(&dir).map_err(|e| Error::device(format!("Can't create {dir}: {e}")))?;
    let out = Command::new("mount").arg(device).arg(&dir).output().map_err(|e| Error::device(format!("Could not run mount: {e}")))?;
    if out.status.success() {
        Ok(dir)
    } else {
        Err(Error::device(format!(
            "Couldn't mount {device}: {}. Mount the stick on the host and make the folder available to this program instead (see the README).",
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// Unmount (and power off, where possible) so the stick is safe to pull out.
pub fn eject(target: &Target) -> Result<(), Error> {
    if let Some(dev) = &target.device {
        if let Ok(out) = Command::new("udisksctl").args(["unmount", "-b", dev]).output() {
            if out.status.success() {
                let _ = Command::new("udisksctl").args(["power-off", "-b", dev]).output();
                return Ok(());
            }
        }
    }
    let out = Command::new("umount").arg(&target.mount_point).output().map_err(|e| Error::device(format!("Could not run umount: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Error::device(format!(
            "Couldn't unmount {}: {}. The data has been written (the copy ends with a sync); unmount it from where it was mounted.",
            target.mount_point,
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTINFO: &str = "\
22 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw
40 22 8:17 / /run/media/will/My\\040Stick rw,nosuid,nodev,relatime shared:80 - vfat /dev/sdb1 rw,fmask=0022,codepage=437
41 22 0:30 / /proc rw,nosuid shared:14 - proc proc rw
42 22 8:33 /sub /mnt/usb rw,relatime - exfat /dev/sdc1 rw,uid=1000
";

    #[test]
    fn parses_the_mount_table() {
        let m = parse_mountinfo(MOUNTINFO);
        assert_eq!(m.len(), 4);
        assert_eq!(m[1].mount_point, "/run/media/will/My Stick");
        assert_eq!((m[1].fs_type.as_str(), m[1].source.as_str(), m[1].major_minor.as_str()), ("vfat", "/dev/sdb1", "8:17"));
        assert_eq!(m[3].fs_type, "exfat");
        assert!(is_storage_fs("vfat") && is_storage_fs("exfat") && !is_storage_fs("proc") && !is_storage_fs("tmpfs") && !is_storage_fs("overlay"));
    }

    #[test]
    fn reads_df_output() {
        assert_eq!(parse_df("       1B-blocks        Avail\n 62537883648  62514696192\n"), Some((62537883648, 62514696192)));
        assert_eq!(parse_df("garbage"), None);
    }

    #[test]
    fn filesystem_rules() {
        assert_eq!(fs_rules("vfat"), (Some(4294967295), true));
        assert_eq!(fs_rules("exfat"), (None, true));
        assert_eq!(fs_rules("ext4"), (None, false));
    }

    #[test]
    fn unescapes_mount_points() {
        assert_eq!(unescape_octal("/mnt/a\\040b\\134c"), "/mnt/a b\\c");
        assert_eq!(unescape_octal("/plain"), "/plain");
    }

    #[test]
    fn reads_the_size_of_a_folder_and_finds_its_filesystem() {
        let s = fs_stats(&std::env::temp_dir()).unwrap();
        assert!(s.total_bytes > 0 && s.free_bytes <= s.total_bytes && s.block_size >= 512);
        assert!(filesystem_of(&std::env::temp_dir()).is_some());
    }
}
