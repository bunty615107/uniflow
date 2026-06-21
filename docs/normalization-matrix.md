# Cross-Platform Normalization Matrix

This document details how UniFlow handles cross-platform edge cases during file transfers, ensuring correctness through normalization. The constraints are lossless preservation when possible and explicitly-degraded behavior (with no silent failures) when exact translation is impossible.

## 1. Path Separators
Paths use different separators depending on the OS (e.g., `\` on Windows vs `/` on POSIX systems).
*   **Normalization Strategy:** Paths are internally converted into a canonical representation using standard directory separators (`/`), removing `./` and `/../` logically.
*   **Edge Cases:**
    *   **Windows `\` separators:** Replaced with `/` during ingestion or logically handled in component breakdown.
    *   **Trailing separators:** Stripped to canonicalize paths.

## 2. Case-Sensitivity Collisions
File systems have varying rules around case sensitivity. For instance, Linux (ext4/APFS) is generally case-sensitive, while Windows (NTFS by default) and FAT32 are case-insensitive.
*   **Normalization Strategy:** When transferring to a case-insensitive file system from a case-sensitive one, collisions (e.g., `File.txt` and `file.txt`) must be detected.
*   **Edge Cases:**
    *   **macOS / Windows Target:** If two files resolve to the same name case-insensitively, UniFlow will flag a conflict. We do not silently overwrite.
    *   **Linux Target:** Transfers are native and preserve case explicitly.

## 3. Timestamp Resolution Rounding
Timestamp precision varies widely (e.g., nanoseconds on APFS/ext4 vs. 2 seconds on FAT32).
*   **Normalization Strategy:** To ensure accurate delta transfer and verification, timestamps are truncated to a resolution supported by both the source and target filesystems.
*   **Edge Cases:**
    *   **FAT32 Target:** Timestamps are truncated (rounded down) to a 2-second resolution to prevent infinite sync loops.
    *   **macOS/Linux/Windows (NTFS):** Typically safe to synchronize up to the millisecond, rounded down.

## 4. Unix Permissions and Symlinks
Permissions and symlinks do not map exactly across all operating systems.
*   **Normalization Strategy:**
    *   **Symlinks:** Preserved when supported by the target. If unsupported (e.g., certain Windows or Android setups), they degrade to their resolved target or an error is explicitly logged.
    *   **Permissions:** Preserved natively where supported (POSIX). When transferring across POSIX and non-POSIX filesystems (like FAT32 or NTFS), they are explicitly degraded to reasonable defaults (e.g., `0644` for files and `0755` for directories), never granting unintended elevated access (e.g., `0777`).
