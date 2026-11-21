# Book Reader — Roadmap

## Stages

1. **Desktop + library** (Windows only)
   - Fork Simple Reader into this repository; rename to Book Reader / `breader.exe`.
   - Remove Markdown (marked, Mermaid, highlight.js) and unused foliate-js modules.
   - First-run library folder choice, library scan and metadata cache, library screen.
   - Naming template, Copy / Move to library, "Organize library" with preview.
   - Local bookmarks.
2. **Android spike** — done (21.11.2026). The Tauri app builds for Android from the same crate in GitHub Actions and runs on the phone; with **All files access** (Android 11+, so `minSdk` 30) the library is `Documents/Books` and the Rust library code works there unchanged (books in subfolders, FB2 in windows-1251, PDFs skipped). Findings:
   - The Android SDK and NDK have no Windows-on-ARM64 builds, so APKs come from CI (as in PswManager); `cargo check --target aarch64-linux-android` still catches compile errors locally.
   - Folder picking: the dialog plugin can't pick folders on Android; SAF would mean a second, Kotlin library backend over `content://` URIs. All files access it is (sideload only).
   - The debug APK is ~140 MB (debug symbols); its signing key changes per CI run, so a reinstall needs `adb uninstall` first. A stable release key comes with stage 4.
   - The page is drawn edge to edge: safe-area insets keep it clear of the system bars.
3. **Servers** (on Windows)
   - OPDS sources (Basic / Digest auth, credentials in the OS store), catalog browsing, search, download into the library.
   - KOSync progress sync against CWA.
4. **Full Android version** — library, tap zones / swipe / volume keys, full screen, keep screen on, servers and sync with Windows, signed APK.
5. **Shared core** — extract the rendering core shared with Simple Reader once its API has survived Android.
6. **Phase "e"** — read status and shelves in CWA, metadata editing through the Calibre Content Server.

Full-text search in a book and grouping by series fit after stage 4.

## Open questions

- **Organize and chains of renames:** a book whose template path is occupied by another book that the same run moves away gets ` (2)`; running Organize again fixes it. Ordering the moves would avoid the extra step.

- **Bookmark sync:** KOSync carries only the position; syncing bookmarks needs another mechanism.
- **Phase "e" details:** which fields, which server, how fragile the CWA web endpoints are.
- **Shared core with Simple Reader:** package boundary and how both repositories consume it (`file:` dependency, git submodule).
- **Android library folder other than `Documents/Books`:** a folder browser of our own over shared storage (as PswManager has), in stage 4.
