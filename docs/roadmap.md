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
   - Pages turn with a swipe (foliate-js's own gesture). Tapping a page edge, which turns pages with a mouse on Windows, does nothing on the phone yet: tap zones come in stage 4.
3. **Servers** (on Windows) — done (21.11.2026), checked against CWA and a Calibre Content Server:
   - OPDS sources (Basic / Digest auth, passwords in the Windows Credential Manager), catalog browsing, search, download into the library.
   - KOSync progress sync against CWA (two profiles on one PC stood in for two devices).
   - Findings: readers type a server's address, not its `/opds` path, so saving discovers the catalog; CWA's KOSync takes the plain login over Basic and serves FB2 as `text/xml`; the TLS library (rustls/ring) needs clang on Windows ARM64 (LLVM) and the NDK's clang for Android, so Android compile errors now show up in CI only.
4. **Full Android version** — done (22.11.2026): library, volume keys, full screen, keep screen on, servers and sync with Windows (catalog passwords in the Android Keystore), choosing another library folder, adding books from outside it ("Open with", "Share", "Add book…"), a release APK signed with a personal key. Done earlier: one window and Back, tap zones, a top bar for a finger.
5. **Shared core** — done (22.11.2026): the reading part of the frontend (BookView with touch, appearance, layout, page labels, DOM helpers, foliate-js) is the public package [reader-core](https://github.com/OlegG90/reader-core); Book Reader and Simple Reader depend on it by tag. The Rust backends stay per app: they have diverged (fingerprint, store, data folder, command line).
6. **UI refresh** (Windows + Android) — the design in [docs/design/ui-refresh.md](design/ui-refresh.md) (desktop D1–D4, Android M1–M4), with its decisions. Same features, regrouped and restyled:
   - Theme: Cormorant Garamond / Lora (bundled), one gold accent, tokens in `style.css`; theme swatches.
   - Reader chrome: a slim top bar (Library, Contents · title · Bookmark, Aa, mode, ⋯) and a progress row with chapter ticks; on Android a top bar plus a bottom bar (progress slider, Contents / Aa / Mode / More), both toggled by a centre tap.
   - Settings split: the **Aa** popover (appearance, flow) while reading, a **Settings page** with sections (Reading, Library, Catalogs, System, Shortcuts) in the library.
   - Library: continue-reading cards, a status switch with counts, progress under each cover; on Android bottom navigation (Library · Catalogs · Add book).
   - Contents / Bookmarks as one drawer with a switch; the offer bar with one primary choice and ⋯; catalog breadcrumbs, an "In library" tag, outlined download buttons.
   - Order: theme and tokens first (everything else builds on them), then reader chrome, settings split, library, the rest; each part checked on Windows and the phone.
7. **Phase "e"** — read status and shelves in CWA, metadata editing through the Calibre Content Server.

Full-text search in a book and grouping by series fit after stage 4.

## Open questions

- **Organize and chains of renames:** a book whose template path is occupied by another book that the same run moves away gets ` (2)`; running Organize again fixes it. Ordering the moves would avoid the extra step.

- **Bookmark sync:** KOSync carries only the position; syncing bookmarks needs another mechanism.
- **Phase "e" details:** which fields, which server, how fragile the CWA web endpoints are.
