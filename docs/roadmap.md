# Book Reader — Roadmap

## Stages

1. **Desktop + library** (Windows only)
   - Fork Simple Reader into this repository; rename to Book Reader / `breader.exe`.
   - Remove Markdown (marked, Mermaid, highlight.js) and unused foliate-js modules.
   - First-run library folder choice, library scan and metadata cache, library screen.
   - Naming template, Copy / Move to library, "Organize library" with preview.
   - Formats: EPUB, FB2 and zipped FB2 (`.fb2.zip`, and `.fbz` added with the format modules in 11.2026).
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
6. **UI refresh** (Windows + Android) — done (23.11.2026): the design in [docs/design/ui-refresh.md](design/ui-refresh.md) (desktop D1–D4, Android M1–M4), with its decisions. Same features, regrouped and restyled:
   - Theme: Cormorant Garamond / Lora (bundled), one gold accent, tokens in `style.css`; theme swatches.
   - Reader chrome: a slim top bar (Library, Contents · title · Bookmark, Aa, mode, ⋯) and a progress row with chapter ticks; on Android a top bar plus a bottom bar (progress slider, Contents / Aa / Mode / More), both toggled by a centre tap.
   - Settings split: the **Aa** popover (appearance, flow) while reading, a **Settings page** with sections (Reading, Library, Catalogs, System, Shortcuts) in the library.
   - Library: continue-reading cards, a status switch with counts, progress under each cover; on Android bottom navigation (Library · Catalogs · Add book).
   - Contents / Bookmarks as one drawer with a switch; the offer bar with one primary choice and ⋯; catalog breadcrumbs, an "In library" tag, outlined download buttons.
   - Order: theme and tokens first (everything else builds on them), then reader chrome, settings split, library, the rest; each part checked on Windows and the phone.
   - Added on the way, at the user's request: the settings as tabs (one section at a time); the desktop library window as a WinUI-style navigation pane (Continue reading, Library, Catalogs, Settings with its sections) and the phone's library with a bottom bar the same way; Continue reading and the books as separate pages, the history as cover cards with a 1–5 length setting; an interface font setting; catalog breadcrumbs named after the folders opened; shortcuts that work in any keyboard layout. reader-core 0.3.0 gives the page within the chapter and the chapter starts.
7. **Library actions** (Windows + Android): open a started book from Library with a choice (continue at NN% or from the beginning); delete a book from the library through its menu (right click / long press), to the Recycle Bin on Windows, keeping its position and bookmarks; a Series page in the pane and the bottom bar.
8. **Phase "e"** — read status and shelves in CWA, metadata editing through the Calibre Content Server.
9. **Optional modules** — done (11.2026): the PswManager-style module pattern (layer 0 core, layer 1 registry, `disabledModules`, Settings › Modules, guards). Switchable: `format:epub`, `format:fb2`, `capability:catalogs`, `capability:progress-sync`, `capability:organize`, `capability:file-types` (desktop only), `capability:update-check`.
10. **Cloud progress sync** (in progress): sync positions through a cloud store, independent of catalogs. Delivered (tracks #50–#54, PRs #57–#62): the intent/transport split (`capability:progress-sync` as the intent, KOSync needing catalogs, clouds only their own module), provider fan-out with per-transport isolation and retry, the `Transport` trait; Dropbox first (`capability:dropbox`): OAuth PKCE over loopback, refresh token in the credential store / Keystore, a fingerprint-keyed `sync-state.json` in the app's folder, newest-wins merge, offline edits converge on the next push — verified phone ↔ PC; then Google Drive (`capability:googledrive`): Desktop and Android OAuth clients, `sync-state.json` in `appDataFolder`, the phone login through the app's scheme. Settings › System shows `Cloud connection` with Disconnect plus `Add cloud` with a picker (OneDrive listed as coming soon); every transport has a Modules switch with a locked-reason. Locked decisions: provider order is Dropbox → OneDrive → Google (one PR per store); time is server-stamped where the server stamps it (KOSync), client clock for cloud records; conflicts resolve newest-wins, silently — the only prompt is the jump offer on pull. Still open: bookmarks-union, portable settings (per-key LWW), sources without passwords.
11. **Book file sync** — done (25.11.2026): reading books follow their positions into the cloud. A fingerprint-keyed manifest (`sync-books.json`) records uploaded books; closing a `reading` book uploads its file when the cloud lacks the fingerprint (verified on the phone). The library offers missing cloud books for download (never silently), landing under their manifest names (` (2)` on collision), bytes verified against the manifest; the `booksWifiOnly` setting (on by default) holds book transfers on metered networks. Transports: Dropbox (own folder) and Google Drive (`appDataFolder`).
12. **Library sync** — done (11.2026, decided 26.11.2026, issue #106): the device library becomes the synced unit instead of "reading books". The catalog is the big library; the library on a device is the small one, the same everywhere and offline at once. Adding a file to the library folder uploads it and other devices download it automatically (by the naming template, Wi-Fi permitting); removing it here only marks it *removed here*, and **Remove from clouds** (a Removed books list) deletes it from every cloud, after which other devices move their copies to the Recycle Bin / delete them. Every connected cloud is used at once (#103, UI #105, landed 11.2026). Cloud space is a separate limit with a per-book indicator (in the cloud / waiting / doesn't fit); uploads go books being read first, then newest added. Builds on: the backend sync engine with a persistent outbox (#101, landed 11.2026), per-device records in the cloud instead of read-modify-write (#102), the `CloudStore` trait (#99) and typed sync errors (#100). Replaces stage 11's upload-on-close and download offer.

Full-text search in a book fits after stage 4.

## Open questions

- **Disabled formats (locked when the modules landed):** books in a disabled format stay visible in the library but refuse to open. Hiding them as well would lose the hint that a switch brings them back.
- **Organize and chains of renames:** a book whose template path is occupied by another book that the same run moves away gets ` (2)`; running Organize again fixes it. Ordering the moves would avoid the extra step.

- **Bookmark sync:** KOSync carries only the position; syncing bookmarks needs another mechanism.
- **Library sync (stage 12):** where the Removed books list lives (the library's status switch or Settings › System); whether a removal coming from another device shows a one-line notice first; above how many books (or MB) the first download asks.
- **Phase "e" details:** which fields, which server, how fragile the CWA web endpoints are.
