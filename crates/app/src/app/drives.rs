//! Drives offered by the directory switcher's drive section.
//!
//! Mirrors Far's drive menu: `Alt+F1` lists the drives reachable from the
//! leftmost panel's column, `Alt+F2` from the rightmost one.
//!
//! Only already-mounted filesystems are listed. Entering one therefore never
//! needs a mount privilege, so the list can be offered without a capability
//! check — an unmounted device is simply absent rather than shown greyed out.

use std::path::{Path, PathBuf};

/// One selectable drive: a mount point plus an optional human label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drive {
    /// Directory the panel navigates to when this drive is picked.
    pub path: PathBuf,
    /// Filesystem type or volume name, rendered dimmed after the path.
    pub label: Option<String>,
}

/// The root filesystem, offered on unix so the list is never empty — a
/// container mounts `/` as `overlay`, which the filter below drops.
#[cfg(unix)]
const ROOT: Option<&str> = Some("/");

/// Windows has no root path to fall back on: an empty drive list there means no
/// drive exists, and inventing `/` would offer a directory that is not a drive.
#[cfg(not(unix))]
const ROOT: Option<&str> = None;

/// Filesystem types that are kernel bookkeeping rather than storage a user
/// would pick as a drive. Names come from the mount table itself; anything
/// absent from this list is offered.
const PSEUDO_FILESYSTEMS: &[&str] = &[
    "autofs",
    "binfmt_misc",
    "bpf",
    "cgroup",
    "cgroup2",
    "configfs",
    "debugfs",
    "devpts",
    "devtmpfs",
    "efivarfs",
    "fusectl",
    "hugetlbfs",
    "mqueue",
    "nsfs",
    "overlay",
    "proc",
    "pstore",
    "ramfs",
    "rpc_pipefs",
    "securityfs",
    "squashfs",
    "sysfs",
    "tmpfs",
    "tracefs",
];

/// Already-mounted drives, sorted by path, without duplicates, root first.
///
/// Sorting is what puts the root first: `/` compares below every other
/// absolute path.
pub fn mounted_drives() -> Vec<Drive> {
    let mut drives = platform::mounted_drives();
    if let Some(root) = ROOT {
        drives.push(Drive {
            path: PathBuf::from(root),
            label: None,
        });
    }
    drives.sort_by(|a, b| a.path.cmp(&b.path));
    // Stable sort keeps a mount-table entry ahead of the pushed root, so the
    // real filesystem type survives when the root is listed twice.
    drives.dedup_by(|a, b| a.path == b.path);
    drives
}

/// True when `fs_type` names storage worth offering as a drive.
///
/// Pure on purpose: whether the mount point still exists is a separate,
/// environment-dependent check made by [`mounts_to_drives`].
fn is_storage_filesystem(fs_type: &str) -> bool {
    !fs_type.is_empty() && !PSEUDO_FILESYSTEMS.contains(&fs_type)
}

/// Split a mount table into `(mount point, filesystem type)` pairs.
///
/// Pure: no filesystem access, so the parsing rules are testable without
/// depending on the host's mounts.
fn parse_mounts(contents: &str) -> Vec<(String, String)> {
    contents
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _device = fields.next()?;
            let mount_point = fields.next()?;
            let fs_type = fields.next()?;
            Some((unescape_mount_field(mount_point), fs_type.to_string()))
        })
        .collect()
}

/// Keep the storage entries whose mount point is still a live directory.
fn mounts_to_drives(contents: &str) -> Vec<Drive> {
    parse_mounts(contents)
        .into_iter()
        .filter(|(mount_point, fs_type)| {
            is_storage_filesystem(fs_type) && Path::new(mount_point).is_dir()
        })
        .map(|(mount_point, fs_type)| Drive {
            path: PathBuf::from(mount_point),
            label: Some(fs_type),
        })
        .collect()
}

/// Decode the octal escapes `/proc/mounts` uses for space, tab, newline and
/// backslash in paths.
///
/// Byte-wise on purpose: an escape names a byte, not a character. Decoding
/// through `char` would map every value above 0x7F to the Latin-1 character of
/// the same code point, so a multi-byte escape would come back mojibake.
fn unescape_mount_field(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Only a full `\ooo` triple is an escape; anything shorter is literal.
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            if let Some(code) = octal_byte(&bytes[i + 1..i + 4]) {
                out.push(code);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // Bytes outside an escape are original UTF-8, so this only replaces a
    // malformed tail — which the kernel never writes.
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse exactly three octal digits into one byte.
fn octal_byte(digits: &[u8]) -> Option<u8> {
    let text = std::str::from_utf8(digits).ok()?;
    u8::from_str_radix(text, 8).ok()
}

/// Directories under `/Volumes`, `/media` and `/mnt` are where BSD-flavoured
/// unix systems hang mounted drives, and none of them exposes a mount table
/// through `std`.
#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
fn volumes_under(roots: &[&str]) -> Vec<Drive> {
    let mut drives = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                drives.push(Drive { path, label: None });
            }
        }
    }
    drives
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{mounts_to_drives, Drive};

    pub(super) fn mounted_drives() -> Vec<Drive> {
        // `/proc/self/mounts` is per-process and cannot disappear the way a
        // shared `/proc/mounts` can when a namespace is torn down. Absent
        // procfs leaves the list to the root entry every caller gets anyway.
        std::fs::read_to_string("/proc/self/mounts")
            .map(|contents| mounts_to_drives(&contents))
            .unwrap_or_default()
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{volumes_under, Drive};

    pub(super) fn mounted_drives() -> Vec<Drive> {
        volumes_under(&["/Volumes"])
    }
}

#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
mod platform {
    use super::{volumes_under, Drive};

    pub(super) fn mounted_drives() -> Vec<Drive> {
        volumes_under(&["/Volumes", "/media", "/mnt"])
    }
}

#[cfg(windows)]
mod platform {
    use super::Drive;
    use std::path::{Path, PathBuf};

    pub(super) fn mounted_drives() -> Vec<Drive> {
        drive_letters()
            .into_iter()
            .map(|letter| Drive {
                path: PathBuf::from(format!("{letter}:\\")),
                label: None,
            })
            .collect()
    }

    /// Windows has no mount table to read, so each letter is probed. A mapped
    /// but disconnected network drive stays listed: it exists as a path.
    fn drive_letters() -> Vec<char> {
        ('A'..='Z')
            .filter(|letter| Path::new(&format!("{letter}:\\")).is_dir())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_filesystems_are_not_storage() {
        assert!(!is_storage_filesystem("proc"));
        assert!(!is_storage_filesystem("tmpfs"));
        assert!(!is_storage_filesystem("cgroup2"));
        assert!(!is_storage_filesystem("overlay"));
        // An empty type is a malformed entry, not a drive.
        assert!(!is_storage_filesystem(""));
    }

    #[test]
    fn real_filesystems_are_storage() {
        assert!(is_storage_filesystem("ext4"));
        assert!(is_storage_filesystem("xfs"));
        assert!(is_storage_filesystem("vfat"));
        assert!(is_storage_filesystem("nfs4"));
    }

    #[test]
    fn parse_mounts_reads_the_first_three_fields() {
        let table = "/dev/sda2 / ext4 rw,relatime 0 0\nproc /proc proc rw 0 0\n";
        assert_eq!(
            parse_mounts(table),
            vec![
                ("/".to_string(), "ext4".to_string()),
                ("/proc".to_string(), "proc".to_string()),
            ]
        );
    }

    #[test]
    fn parse_mounts_skips_truncated_lines() {
        assert!(parse_mounts("/dev/sda2 / ext4\n").len() == 1);
        assert!(parse_mounts("/dev/sda2\n").is_empty());
        assert!(parse_mounts("\n\n").is_empty());
    }

    #[test]
    fn parse_mounts_decodes_octal_escapes() {
        // `/proc/mounts` writes a space in a path as `\040`.
        let table = "/dev/sdb1 /media/My\\040Disk ext4 rw 0 0\n";
        assert_eq!(
            parse_mounts(table),
            vec![("/media/My Disk".to_string(), "ext4".to_string())]
        );
    }

    #[test]
    fn unescape_leaves_a_lone_backslash_and_non_octal_digits_alone() {
        assert_eq!(unescape_mount_field("a\\b"), "a\\b");
        assert_eq!(unescape_mount_field("a\\9x"), "a\\9x");
        assert_eq!(unescape_mount_field("plain"), "plain");
    }

    #[test]
    fn unescape_preserves_multibyte_characters() {
        assert_eq!(unescape_mount_field("/media/Диск\\0401"), "/media/Диск 1");
    }

    #[test]
    fn unescape_decodes_escapes_byte_wise() {
        // `\303\251` is the two bytes of `é` in UTF-8. Decoding each escape as a
        // character instead would yield the Latin-1 pair `Ã©`.
        assert_eq!(unescape_mount_field("a\\303\\251"), "aé");
        // A tab and a backslash are the other two escapes the kernel writes.
        assert_eq!(unescape_mount_field("a\\011b"), "a\tb");
        assert_eq!(unescape_mount_field("a\\134b"), "a\\b");
        // A backslash the kernel did not escape stays literal: only a full
        // `\ooo` triple is an escape, and `C:\` has no digits after it.
        assert_eq!(unescape_mount_field("C:\\"), "C:\\");
    }

    #[test]
    fn mount_entries_without_a_live_mount_point_are_dropped() {
        let table = "/dev/sda1 /definitely/not/mounted/here ext4 rw 0 0\n";
        assert!(mounts_to_drives(table).is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn mounted_drives_always_contains_the_root_once() {
        let drives = mounted_drives();
        let roots = drives.iter().filter(|d| d.path == Path::new("/")).count();
        assert_eq!(roots, 1, "root must be listed exactly once");
        assert!(!drives.is_empty());
    }

    #[test]
    fn mounted_drives_is_sorted_and_unique() {
        let drives = mounted_drives();
        for pair in drives.windows(2) {
            assert!(
                pair[0].path < pair[1].path,
                "{:?} must sort before {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn mounted_drives_never_offers_a_kernel_filesystem() {
        for drive in mounted_drives() {
            if let Some(label) = &drive.label {
                assert!(
                    is_storage_filesystem(label),
                    "{label} must not be offered as a drive"
                );
            }
        }
    }
}
