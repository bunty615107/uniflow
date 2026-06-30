//! Cross-platform path & metadata normalization (Deliverable 3).
//!
//! ONE place that reconciles the filesystem differences the Profiler discovers, so
//! a Windows→macOS or Android→iPhone move is correct and lossless. Everything here
//! is pure and unit-tested; the transfer core and adapters call into it rather than
//! special-casing platforms themselves.
//!
//! Edge cases handled:
//!   * path separators (`\` vs `/`)
//!   * path traversal / absolute-path injection (security)
//!   * case-sensitivity collisions (ext4 → APFS/NTFS)
//!   * Windows reserved device names (CON, PRN, NUL, COM1…) and trailing dot/space
//!   * max path length
//!   * timestamp resolution rounding (NTFS 100ns ↔ ext4 1ns ↔ FAT 2s)
//!   * which metadata can be preserved on the target FS

use crate::domain::profile::OsFsInfo;
use crate::error::{Result, UniFlowError};

/// Windows reserved device base names (case-insensitive), invalid as any path segment.
const WIN_RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Characters illegal in NTFS/FAT filenames.
const WIN_ILLEGAL: &[char] = &['<', '>', ':', '"', '|', '?', '*'];

/// Normalize a *relative* source path into a safe relative path for the target FS.
///
/// Returns an error (never a silently-mangled path) when the input cannot be made
/// safe — callers turn that into a per-file failure, never a corrupt write.
pub fn normalize_relative_path(rel: &str, target: &OsFsInfo) -> Result<String> {
    if rel.is_empty() {
        return Err(UniFlowError::Config("empty relative path".into()));
    }

    // Unify separators to '/' for processing.
    let unified = rel.replace('\\', "/");

    // Reject traversal and absolute paths outright (security: never escape the dest root).
    if unified.starts_with('/') || unified.contains("..") {
        return Err(UniFlowError::Config(format!(
            "unsafe path rejected (absolute or traversal): {rel}"
        )));
    }
    // Windows drive-letter injection (e.g. "C:foo").
    if unified.len() >= 2 && unified.as_bytes()[1] == b':' {
        return Err(UniFlowError::Config(format!("drive-letter path rejected: {rel}")));
    }

    let target_is_windows = target.platform == "windows";

    let mut out_segments = Vec::new();
    for seg in unified.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if target_is_windows {
            // Reserved device name (ignoring extension): "con.txt" is still reserved.
            let base = seg.split('.').next().unwrap_or(seg).to_ascii_lowercase();
            if WIN_RESERVED.contains(&base.as_str()) {
                return Err(UniFlowError::Config(format!(
                    "segment '{seg}' is a Windows reserved name; cannot create losslessly"
                )));
            }
            if seg.chars().any(|c| WIN_ILLEGAL.contains(&c)) {
                return Err(UniFlowError::Config(format!(
                    "segment '{seg}' contains characters illegal on the target NTFS/FAT volume"
                )));
            }
            // Trailing dot/space is stripped by the Win32 layer → would silently differ.
            if seg.ends_with(' ') || seg.ends_with('.') {
                return Err(UniFlowError::Config(format!(
                    "segment '{seg}' ends with space/dot (illegal/altered on Windows)"
                )));
            }
        }
        out_segments.push(seg);
    }

    if out_segments.is_empty() {
        return Err(UniFlowError::Config(format!("path resolves to empty: {rel}")));
    }

    // Reassemble with the target's native separator.
    let sep = if target_is_windows { "\\" } else { "/" };
    let result = out_segments.join(sep);

    if result.len() as u32 > target.max_path_len {
        return Err(UniFlowError::Config(format!(
            "path length {} exceeds target max {}",
            result.len(),
            target.max_path_len
        )));
    }

    Ok(result)
}

/// Detect case-insensitive collisions when moving from a case-sensitive source to a
/// case-insensitive target (e.g. `Foo.txt` and `foo.txt` both exist on ext4 →
/// would clobber on NTFS/APFS-default). Returns the colliding pairs so the caller
/// can fail loudly instead of silently overwriting.
pub fn case_collisions(paths: &[String], target: &OsFsInfo) -> Vec<(String, String)> {
    let mut collisions = Vec::new();
    if target.case_sensitive_fs {
        return collisions; // target preserves case distinctions
    }
    let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for p in paths {
        let folded = p.to_lowercase();
        if let Some(prev) = seen.get(&folded) {
            if prev != p {
                collisions.push((prev.clone(), p.clone()));
            }
        } else {
            seen.insert(folded, p.clone());
        }
    }
    collisions
}

/// Round a source mtime (nanoseconds since epoch) to what the target FS can store,
/// so a round-trip comparison doesn't spuriously flag a difference.
pub fn round_timestamp_ns(source_ns: u64, target: &OsFsInfo) -> u64 {
    let res = target.timestamp_resolution_ns.max(1);
    (source_ns / res) * res
}

/// Whether to attempt preserving unix permission bits on the target.
pub fn should_preserve_perms(target: &OsFsInfo) -> bool {
    target.preserves_unix_perms
}

/// Whether symlinks can be recreated as symlinks (else they must be dereferenced).
pub fn should_preserve_symlinks(target: &OsFsInfo) -> bool {
    target.preserves_symlinks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::profile::AsyncIoBackend;

    fn win() -> OsFsInfo {
        OsFsInfo {
            platform: "windows".into(),
            case_sensitive_fs: false,
            max_path_len: 260,
            max_open_fds: 8192,
            async_io: AsyncIoBackend::Iocp,
            preserves_unix_perms: false,
            preserves_symlinks: false,
            timestamp_resolution_ns: 100,
        }
    }
    fn linux() -> OsFsInfo {
        OsFsInfo {
            platform: "linux".into(),
            case_sensitive_fs: true,
            max_path_len: 4096,
            max_open_fds: 1024,
            async_io: AsyncIoBackend::IoUring,
            preserves_unix_perms: true,
            preserves_symlinks: true,
            timestamp_resolution_ns: 1,
        }
    }

    #[test]
    fn separators_unified_to_target() {
        // Windows source path → linux target uses '/'.
        assert_eq!(normalize_relative_path("a\\b\\c.txt", &linux()).unwrap(), "a/b/c.txt");
        // linux source path → windows target uses '\'.
        assert_eq!(normalize_relative_path("a/b/c.txt", &win()).unwrap(), "a\\b\\c.txt");
    }

    #[test]
    fn traversal_and_absolute_rejected() {
        assert!(normalize_relative_path("../etc/passwd", &linux()).is_err());
        assert!(normalize_relative_path("/etc/passwd", &linux()).is_err());
        assert!(normalize_relative_path("a/../../b", &linux()).is_err());
        assert!(normalize_relative_path("C:secret", &win()).is_err());
    }

    #[test]
    fn windows_reserved_names_rejected() {
        assert!(normalize_relative_path("dir/CON", &win()).is_err());
        assert!(normalize_relative_path("dir/con.txt", &win()).is_err());
        assert!(normalize_relative_path("dir/LPT1", &win()).is_err());
        // but fine on linux
        assert!(normalize_relative_path("dir/con.txt", &linux()).is_ok());
    }

    #[test]
    fn windows_illegal_chars_and_trailing_dot() {
        assert!(normalize_relative_path("a/b:c", &win()).is_err());
        assert!(normalize_relative_path("a/name ", &win()).is_err());
        assert!(normalize_relative_path("a/name.", &win()).is_err());
    }

    #[test]
    fn max_path_enforced() {
        let long = "a/".repeat(200) + "file";
        assert!(normalize_relative_path(&long, &win()).is_err()); // > 260
        assert!(normalize_relative_path(&long, &linux()).is_ok()); // < 4096
    }

    #[test]
    fn case_collisions_only_on_insensitive_target() {
        let paths = vec!["Foo.txt".to_string(), "foo.txt".to_string(), "bar".to_string()];
        assert!(case_collisions(&paths, &linux()).is_empty()); // case-sensitive: no collision
        assert_eq!(case_collisions(&paths, &win()).len(), 1); // case-insensitive: 1 collision
    }

    #[test]
    fn timestamp_rounds_to_target_resolution() {
        // NTFS 100ns resolution truncates finer timestamps.
        assert_eq!(round_timestamp_ns(123_456_789, &win()), 123_456_700);
        // ext4 1ns keeps full precision.
        assert_eq!(round_timestamp_ns(123_456_789, &linux()), 123_456_789);
    }
}
