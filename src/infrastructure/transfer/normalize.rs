use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// File systems handled explicitly in normalization
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileSystemKind {
    /// Case-sensitive, high precision (ext4/APFS)
    Posix,
    /// Case-insensitive, high precision (NTFS)
    WindowsNtfs,
    /// Case-insensitive, low precision (FAT32)
    Fat32,
}

/// Normalizes path separators and canonicalizes `/./` and `/../` components.
pub fn normalize_path(path_str: &str) -> PathBuf {
    // Explicitly replace Windows-style backslashes with forward slashes
    // before parsing components, so this works cross-platform.
    let sanitized = path_str.replace('\\', '/');
    let path = Path::new(&sanitized);

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            _ => normalized.push(component),
        }
    }
    normalized
}

/// Detect case collisions safely: returns true if files would clash
/// on a case-insensitive target filesystem.
pub fn check_case_collision(path1: &str, path2: &str, target_fs: FileSystemKind) -> bool {
    if target_fs == FileSystemKind::Posix {
        return false; // Case sensitive: File.txt and file.txt don't collide
    }
    path1.to_lowercase() == path2.to_lowercase()
}

/// Resolves timestamps explicitly to avoid infinite sync loops on low-res filesystems.
pub fn normalize_timestamp(time: SystemTime, target_fs: FileSystemKind) -> SystemTime {
    match target_fs {
        FileSystemKind::Fat32 => {
            // FAT32 has a 2-second resolution. Round down to the nearest even second.
            if let Ok(duration) = time.duration_since(UNIX_EPOCH) {
                let secs = duration.as_secs();
                let rounded = secs - (secs % 2);
                UNIX_EPOCH + Duration::from_secs(rounded)
            } else {
                time
            }
        }
        _ => {
            // For NTFS/APFS/ext4, round to nearest millisecond (common delta boundary)
            if let Ok(duration) = time.duration_since(UNIX_EPOCH) {
                let millis = duration.as_millis() as u64;
                UNIX_EPOCH + Duration::from_millis(millis)
            } else {
                time
            }
        }
    }
}

/// Normalizes permissions to a safe degraded fallback if target cannot represent them.
pub fn normalize_permissions(mode: u32, is_dir: bool, target_fs: FileSystemKind) -> u32 {
    match target_fs {
        FileSystemKind::Posix => mode, // Preserve exact
        FileSystemKind::WindowsNtfs | FileSystemKind::Fat32 => {
            // Explicit degradation: 0755 for dirs, 0644 for files.
            // Avoids granting accidental 777.
            if is_dir {
                0o755
            } else {
                0o644
            }
        }
    }
}

/// Normalizes symlink behavior depending on target FS capability.
pub enum SymlinkNormalization {
    Preserved(PathBuf),
    /// Target does not support symlinks; degrade to a plain text file or log an error.
    DegradedToText(String),
}

pub fn normalize_symlink(target_path: &Path, target_fs: FileSystemKind) -> SymlinkNormalization {
    match target_fs {
        FileSystemKind::Posix | FileSystemKind::WindowsNtfs => {
            // Modern Windows (NTFS) often supports symlinks with developer mode,
            // and POSIX systems support them natively.
            SymlinkNormalization::Preserved(target_path.to_path_buf())
        }
        FileSystemKind::Fat32 => {
            // FAT32 does not support symlinks; explicitly degrade to text
            let content = format!("SYMLINK_TARGET: {}", target_path.display());
            SymlinkNormalization::DegradedToText(content)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path_separators() {
        let cases = vec![
            ("a/b/../c/./d", "a/c/d"),
            ("a\\b\\..\\c\\.\\d", "a/c/d"),
            ("foo/bar", "foo/bar"),
            ("/absolute/path", "/absolute/path"),
        ];

        for (input, expected) in cases {
            assert_eq!(normalize_path(input), PathBuf::from(expected));
        }
    }

    #[test]
    fn test_case_collisions() {
        let cases = vec![
            ("File.txt", "file.txt", FileSystemKind::Fat32, true),
            ("File.txt", "file.txt", FileSystemKind::WindowsNtfs, true),
            ("File.txt", "file.txt", FileSystemKind::Posix, false),
            ("a.txt", "b.txt", FileSystemKind::WindowsNtfs, false),
            ("MixedCASE", "mixedcase", FileSystemKind::WindowsNtfs, true),
        ];

        for (p1, p2, fs, expected) in cases {
            assert_eq!(check_case_collision(p1, p2, fs), expected, "Failed for {} vs {} on {:?}", p1, p2, fs);
        }
    }

    #[test]
    fn test_timestamp_rounding() {
        let base_time = UNIX_EPOCH + Duration::from_millis(1005500); // 1005.5 secs

        let cases = vec![
            (base_time, FileSystemKind::Fat32, UNIX_EPOCH + Duration::from_secs(1004)), // rounds down to 1004
            (base_time, FileSystemKind::Posix, UNIX_EPOCH + Duration::from_millis(1005500)), // no change
            (base_time, FileSystemKind::WindowsNtfs, UNIX_EPOCH + Duration::from_millis(1005500)), // no change
        ];

        for (input, fs, expected) in cases {
            assert_eq!(normalize_timestamp(input, fs), expected, "Failed for {:?} on {:?}", input, fs);
        }
    }

    #[test]
    fn test_permission_degradation() {
        let secret_file_mode = 0o600;
        let exec_file_mode = 0o755;

        let cases = vec![
            (secret_file_mode, false, FileSystemKind::Posix, 0o600),
            (secret_file_mode, false, FileSystemKind::WindowsNtfs, 0o644),
            (secret_file_mode, true, FileSystemKind::Fat32, 0o755),
            (exec_file_mode, false, FileSystemKind::Fat32, 0o644),
            (exec_file_mode, true, FileSystemKind::Posix, 0o755),
        ];

        for (mode, is_dir, fs, expected) in cases {
            assert_eq!(normalize_permissions(mode, is_dir, fs), expected, "Failed for mode {:o}, is_dir {}, fs {:?}", mode, is_dir, fs);
        }
    }

    #[test]
    fn test_symlink_normalization() {
        let target = Path::new("/var/log/syslog");

        // POSIX / NTFS preserves
        if let SymlinkNormalization::Preserved(p) = normalize_symlink(target, FileSystemKind::Posix) {
            assert_eq!(p, target);
        } else {
            panic!("POSIX should preserve symlink");
        }

        if let SymlinkNormalization::Preserved(p) = normalize_symlink(target, FileSystemKind::WindowsNtfs) {
            assert_eq!(p, target);
        } else {
            panic!("NTFS should preserve symlink");
        }

        // FAT32 degrades
        if let SymlinkNormalization::DegradedToText(content) = normalize_symlink(target, FileSystemKind::Fat32) {
            assert_eq!(content, format!("SYMLINK_TARGET: {}", target.display()));
        } else {
            panic!("FAT32 should degrade symlink");
        }
    }
}
