//! Reformatting a USB stick. Deliberately hard to do by accident: only USB/removable partitions,
//! never mounted somewhere else that is busy, and the caller must repeat the device name.

use std::{path::Path, process::Command};

use serde::Serialize;

use super::devices;
use crate::error::Error;

#[derive(Debug, Clone, Serialize)]
pub struct FsChoice {
    pub id: &'static str,
    pub label: &'static str,
    pub note: &'static str,
    pub max_label: usize,
    /// The formatting tool is installed.
    pub available: bool,
}

struct Kind {
    id: &'static str,
    label: &'static str,
    note: &'static str,
    tool: &'static str,
    max_label: usize,
}

const KINDS: &[Kind] = &[
    Kind { id: "exfat", label: "exFAT", note: "Works on Windows, macOS, Linux and most players and cars. No 4 GB file limit. Best all-rounder for big collections.", tool: "mkfs.exfat", max_label: 15 },
    Kind { id: "fat32", label: "FAT32", note: "Works everywhere, including old car stereos and TVs. Files are limited to 4 GB.", tool: "mkfs.vfat", max_label: 11 },
    Kind { id: "ext4", label: "ext4", note: "Linux only. Fast and robust; most players and cars can't read it.", tool: "mkfs.ext4", max_label: 16 },
    Kind { id: "ntfs", label: "NTFS", note: "Windows and Linux (macOS reads only). Rarely needed for music.", tool: "mkfs.ntfs", max_label: 32 },
];

fn tool_exists(tool: &str) -> bool {
    let sbin = ["/sbin", "/usr/sbin"].iter().any(|d| Path::new(d).join(tool).is_file());
    sbin || std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file())).unwrap_or(false)
}

pub fn choices() -> Vec<FsChoice> {
    KINDS.iter().map(|k| FsChoice { id: k.id, label: k.label, note: k.note, max_label: k.max_label, available: tool_exists(k.tool) }).collect()
}

/// A volume label that every filesystem accepts.
pub fn clean_label(fs: &str, label: &str) -> Result<String, Error> {
    let kind = KINDS.iter().find(|k| k.id == fs).ok_or_else(|| Error::validation(format!("Unknown filesystem '{fs}'")))?;
    let label = label.trim();
    if label.chars().count() > kind.max_label {
        return Err(Error::validation(format!("A {} name can be at most {} characters", kind.label, kind.max_label)));
    }
    if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '_' || c == '-') {
        return Err(Error::validation("Use only letters, numbers, spaces, - and _ in the name"));
    }
    Ok(if fs == "fat32" { label.to_uppercase() } else { label.to_string() })
}

fn mountpoint_of(device: &str) -> Option<String> {
    let mounts = std::fs::read_to_string("/proc/self/mountinfo").ok().map(|t| devices::parse_mountinfo(&t))?;
    mounts.into_iter().find(|m| m.source == device).map(|m| m.mount_point)
}

/// Erase the partition and put a new, empty filesystem on it. `confirm` must be the device's
/// short name (e.g. "sdb1").
pub fn format_device(device: &str, fs: &str, label: &str, confirm: &str) -> Result<(), Error> {
    if !device.starts_with("/dev/") || device.contains("..") || device.contains(char::is_whitespace) {
        return Err(Error::validation("Invalid device"));
    }
    let short = device.rsplit('/').next().unwrap_or("");
    if confirm.trim() != short {
        return Err(Error::validation(format!("Type “{short}” to confirm erasing this stick")));
    }
    if !devices::is_usb_device(device) {
        return Err(Error::validation("Only USB / removable sticks can be reformatted here"));
    }
    if devices::has_partitions(device) {
        return Err(Error::validation(format!("{device} is a whole disk with partitions; choose one of its partitions")));
    }
    let label = clean_label(fs, label)?;
    let kind = KINDS.iter().find(|k| k.id == fs).expect("checked by clean_label");
    if !tool_exists(kind.tool) {
        return Err(Error::backend(format!("{} isn't installed here, so {} can't be created", kind.tool, kind.label)));
    }

    devices::ensure_node(device)?;
    if let Some(mp) = mountpoint_of(device) {
        let out = Command::new("umount").arg(&mp).output().map_err(|e| Error::device(format!("Could not run umount: {e}")))?;
        if !out.status.success() {
            return Err(Error::device(format!("Couldn't unmount {mp}: {}. Close anything using the stick and try again.", String::from_utf8_lossy(&out.stderr).trim())));
        }
    }

    run_mkfs(device, fs, &label)
}

/// Create the filesystem on `device` (which must already exist and be unmounted).
fn run_mkfs(device: &str, fs: &str, label: &str) -> Result<(), Error> {
    let label = label.to_string();
    let mut cmd = match fs {
        "fat32" => {
            let mut c = Command::new(tool_path("mkfs.vfat"));
            c.args(["-F", "32"]);
            if !label.is_empty() { c.args(["-n", label.as_str()]); }
            c
        }
        "exfat" => {
            let mut c = Command::new(tool_path("mkfs.exfat"));
            if !label.is_empty() { c.args(["-L", &label]); }
            c
        }
        "ext4" => {
            let mut c = Command::new(tool_path("mkfs.ext4"));
            c.args(["-F", "-q"]);
            if !label.is_empty() { c.args(["-L", &label]); }
            c
        }
        _ => {
            let mut c = Command::new(tool_path("mkfs.ntfs"));
            c.args(["-f", "-Q"]);
            if !label.is_empty() { c.args(["-L", &label]); }
            c
        }
    };
    let out = cmd.arg(device).output().map_err(|e| Error::device(format!("Could not run the formatter: {e}")))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        let msg = if msg.trim().is_empty() { String::from_utf8_lossy(&out.stdout).to_string() } else { msg.to_string() };
        return Err(Error::device(format!("Formatting failed: {}", msg.trim())));
    }
    let _ = Command::new("sync").status();
    Ok(())
}

fn unmount_all(disk: &str) -> Result<(), Error> {
    let mounts = std::fs::read_to_string("/proc/self/mountinfo").ok().map(|t| devices::parse_mountinfo(&t)).unwrap_or_default();
    for m in mounts.into_iter().filter(|m| m.source.starts_with(disk) && m.source[disk.len()..].chars().all(|c| c.is_ascii_digit() || c == 'p')) {
        let out = Command::new("umount").arg(&m.mount_point).output().map_err(|e| Error::device(format!("Could not run umount: {e}")))?;
        if !out.status.success() {
            return Err(Error::device(format!("Couldn't unmount {}: {}. Close anything using the stick and try again.", m.mount_point, String::from_utf8_lossy(&out.stderr).trim())));
        }
    }
    Ok(())
}

fn partition_name(disk: &str, n: u32) -> String {
    if disk.chars().last().is_some_and(|c| c.is_ascii_digit()) { format!("{disk}p{n}") } else { format!("{disk}{n}") }
}

/// Wipe the whole stick: remove every partition, write a fresh partition table with one
/// partition filling it, and format that. Returns the new partition's device.
/// `table` is "dos" (MBR, the most compatible) or "gpt"; `confirm` must be the disk's short name.
pub fn format_disk(disk: &str, fs: &str, label: &str, table: &str, confirm: &str) -> Result<String, Error> {
    if !disk.starts_with("/dev/") || disk.contains("..") || disk.contains(char::is_whitespace) {
        return Err(Error::validation("Invalid device"));
    }
    let short = disk.rsplit('/').next().unwrap_or("");
    if confirm.trim() != short {
        return Err(Error::validation(format!("Type “{short}” to confirm erasing the whole stick")));
    }
    if devices::parent_disk(disk).as_deref() != Some(disk) {
        return Err(Error::validation(format!("{disk} is a partition, not a whole disk")));
    }
    if !devices::is_usb_device(disk) {
        return Err(Error::validation("Only USB / removable sticks can be reformatted here"));
    }
    let label = clean_label(fs, label)?;
    let kind = KINDS.iter().find(|k| k.id == fs).expect("checked by clean_label");
    if !tool_exists(kind.tool) {
        return Err(Error::backend(format!("{} isn't installed here, so {} can't be created", kind.tool, kind.label)));
    }
    for t in ["sfdisk", "wipefs"] {
        if !tool_exists(t) {
            return Err(Error::backend(format!("{t} isn't installed here, so the stick can't be repartitioned")));
        }
    }
    let size: u64 = std::fs::read_to_string(format!("/sys/class/block/{short}/size")).ok().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0) * 512;
    let table = match table {
        "gpt" => "gpt",
        _ if size > 2 * 1024 * 1024 * 1024 * 1024 => "gpt", // MBR can't address more than 2 TiB
        _ => "dos",
    };

    devices::ensure_node(disk)?;
    unmount_all(disk)?;
    for (part, _) in devices::partitions_of(disk) {
        let _ = Command::new(tool_path("wipefs")).args(["-a", "-f", &part]).output();
    }
    let out = Command::new(tool_path("wipefs")).args(["-a", "-f", disk]).output().map_err(|e| Error::device(format!("Could not run wipefs: {e}")))?;
    if !out.status.success() {
        return Err(Error::device(format!("Couldn't clear the stick: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }

    // One partition using all the space. Type: FAT32 LBA, NTFS/exFAT, or Linux.
    let ptype = match (table, fs) {
        ("dos", "fat32") => "c",
        ("dos", "ext4") => "83",
        ("dos", _) => "7",
        (_, "ext4") => "0FC63DAF-8483-4772-8E79-3D69D8477DE4",
        _ => "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7",
    };
    let script = format!("label: {table}\n,,{ptype}\n");
    let mut child = Command::new(tool_path("sfdisk"))
        .args(["--wipe", "always", "--wipe-partitions", "always", "-q", disk])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| Error::device(format!("Could not run sfdisk: {e}")))?;
    {
        use std::io::Write;
        child.stdin.take().expect("piped").write_all(script.as_bytes()).map_err(|e| Error::device(e.to_string()))?;
    }
    let out = child.wait_with_output().map_err(|e| Error::device(e.to_string()))?;
    if !out.status.success() {
        return Err(Error::device(format!("Couldn't partition the stick: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }
    let _ = Command::new("blockdev").args(["--rereadpt", disk]).output();
    let _ = Command::new("udevadm").arg("settle").output();

    // Wait for the new partition to appear (inside a container its device node has to be made).
    let part = partition_name(disk, 1);
    let pname = part.rsplit('/').next().unwrap_or("").to_string();
    let mut tries = 0;
    while !Path::new(&format!("/sys/class/block/{pname}")).exists() {
        tries += 1;
        if tries > 40 {
            return Err(Error::device("The new partition didn't appear. Unplug the stick, plug it back in and format the partition instead."));
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    devices::ensure_node(&part)?;
    run_mkfs(&part, fs, &label)?;
    Ok(part)
}

fn tool_path(tool: &str) -> String {
    for d in ["/sbin", "/usr/sbin"] {
        let p = Path::new(d).join(tool);
        if p.is_file() {
            return p.to_string_lossy().to_string();
        }
    }
    tool.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_checked_per_filesystem() {
        assert_eq!(clean_label("fat32", "my music").unwrap(), "MY MUSIC");
        assert!(clean_label("fat32", "twelve chars!").is_err());
        assert!(clean_label("exfat", "a/b").is_err());
        assert_eq!(clean_label("exfat", "").unwrap(), "");
        assert!(clean_label("zfs", "x").is_err());
    }

    #[test]
    fn whole_disk_format_refuses_bad_requests() {
        assert!(format_disk("/dev/nonexistent99", "exfat", "X", "dos", "nope").unwrap_err().to_string().contains("confirm"));
        assert!(format_disk("/dev/nonexistent99", "exfat", "X", "dos", "nonexistent99").is_err());
        assert!(format_disk("/etc", "exfat", "X", "dos", "etc").is_err());
        assert_eq!(partition_name("/dev/sdk", 1), "/dev/sdk1");
        assert_eq!(partition_name("/dev/mmcblk0", 1), "/dev/mmcblk0p1");
    }

    #[test]
    fn refuses_without_the_confirmation_or_for_non_usb_disks() {
        assert!(format_device("/dev/nonexistent99", "exfat", "X", "nope").unwrap_err().to_string().contains("confirm"));
        assert!(format_device("/dev/nonexistent99", "exfat", "X", "nonexistent99").unwrap_err().to_string().contains("USB"));
        assert!(format_device("/etc/passwd", "exfat", "X", "passwd").is_err());
        assert!(format_device("/dev/../etc/x", "exfat", "X", "x").is_err());
    }
}
