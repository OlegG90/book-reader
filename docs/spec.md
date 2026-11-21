# Book Reader — Specification

A personal, compact e-book reader for Windows and Android, with a local library and access to a Calibre-Web-Automated (CWA) / Calibre library. It is a fork of [Simple Reader](https://github.com/OlegG90/simple-reader): reading stays the core, and the library and the servers are built around it.

Guiding principles: **no installation step on the desktop, small size, fast start, a limited feature set, offline-first.** The server is optional; reading never needs the network.

## Product

| | |
|---|---|
| Display name | Book Reader |
| Executable | `breader.exe` |
| Android package | `com.olegg90.bookreader` |
| Data folder | `book-reader` |
| Platforms | Windows 11 (WebView2 is part of the OS), Android (system WebView) |
| UI language | English only |
| Distribution | Windows: a single portable `breader.exe`, no installer, not code-signed, no auto-update. Android: an APK installed by hand (sideload), signed with a stable personal key so updates install over the previous version; no Google Play |
| Audience | Personal tool |
| License | MIT |

### Supported formats

- `.fb2` and `.fb2.zip` — encoding taken from the XML declaration (UTF-8, windows-1251 and others)
- `.epub` — EPUB 2 and EPUB 3, reflowable

### Out of scope

Markdown, fixed-layout EPUB, DRM-protected books, PDF, MOBI/AZW3, TXT, highlights and notes, dictionary / translation, text-to-speech, format conversion, syncing bookmarks or appearance settings, more than one library folder. Full-text search comes later (second priority).

## Reading

Inherited from Simple Reader unless noted:

- **Modes:** paginated (default) and scrolled.
- **Appearance:** theme (light / dark / sepia, follows the system by default), font, font size, line height, margins. Settings are per device and not synced.
- **Navigation:** table of contents, progress bar, footnotes in a pop-up, images.
- **Bookmarks:** new. Local to each device. `B` (or the topbar button) bookmarks the page or removes the bookmark on it; the list sits under the contents (`T`). A bookmark is a point in the text, so it stays found when the font or window size changes the pages. `--no-save` windows keep none.
- **Reading position:** saved automatically, synced between devices through the server (see [Progress sync](#progress-sync)).

### Android interaction

- Tap zones: left third — previous page, right third — next page, centre — menu.
- Swipe to turn pages.
- Volume keys turn pages (a setting, off by default).
- Full screen with the status bar hidden while reading.
- The screen stays on while a book is open.
- Orientation follows the system; no orientation lock in the first version.
- One window: a book opens in place of the library, and the library comes back in its place (the desktop keeps a window per book).
- Back, one step at a time: closes a footnote or a panel (contents, settings, Organize), then returns from the book to the library; on the library it sends the app to the background, as Android apps do.

## Library

### Library folder

- **One** library folder, stored in the config.
- **First run** asks which folder to use as the library:
  - Desktop: default is `lib` next to the exe. If the exe's folder is not writable, the default becomes `%USERPROFILE%\Documents\Book Reader`.
  - Android: `Documents/Books` in shared storage, through the **All files access** permission (`MANAGE_EXTERNAL_STORAGE`; fine for a sideloaded app, not allowed on Google Play). Android 11 (API 30) is the minimum: All files access starts there. The first-run button opens Android's setting for it; after turning it on the reader comes back and taps the button again. With it on, the library is a plain folder that the same Rust code scans and organizes as on Windows; if the access is later turned off, the library screen says how to turn it back on. Adding books from outside the library (Copy / Move) and choosing another folder come with the full Android version: Android's file picker hands over `content://` documents, not paths. The Storage Access Framework was not chosen: every file operation would have to go through `content://` URIs in Kotlin, a second library backend.
- The folder is scanned recursively for supported formats; metadata (title, author, series, cover) is cached.

### Library screen

- Grid of books with covers; search (title, author, series); sort by author (series in reading order), title or recently added; filter by reading status: not started, reading, finished (98% or more, since books often end in notes).
- "Recently added" is when the book came into the library (remembered by the scan cache), not the file date, which a copy keeps.
- The settings show the library folder with **Change…** (pick another) and **Organize…**.
- A "Catalogs" tab for the servers (see [Servers](#servers)).
- Grouping by series comes later.
- Desktop: the library is one window; each book opens in its own window (as in Simple Reader).

### File naming

Every file the app puts into the library follows one template:

```
{Last First}/{Series} {NN} - {Title}.{ext}     # book in a series
{Last First}/{Title}.{ext}                     # no series
Unknown Author/{original file name}            # no metadata
```

- First author only, name as `Last First`.
- Cyrillic and other Unicode characters are kept as is (no transliteration).
- A subtitle colon (`: `) becomes ` - `; other characters not allowed in file names (`\/:*?"<>|`) are replaced with `_`; names are truncated to 120 characters, trailing dots and spaces dropped, and Windows device names (`CON`, `COM1`…) prefixed with `_`.
- **Collision:** if a file with the same name but different content exists, ` (2)`, ` (3)`… is appended. If the library already holds the same book (same fingerprint, under any name), nothing is copied or moved and the window switches to the library copy. A file is never overwritten; a move that can't remove the original undoes its copy.
- **Existing files** in the chosen folder are never renamed automatically. A manual "Organize library" command shows a preview of all renames and applies them only after confirmation.

### Files outside the library (desktop)

Opening a file that is not inside the library folder shows an unobtrusive bar: **Copy to library**, **Move to library**, **Not now**, **Never for this file**.

- The last choice between copy and move becomes the primary button; both are always shown.
- A global setting turns the offer off.
- After a move the window keeps showing the book from its new path.
- The reading position survives copying and moving: book identity is a content fingerprint, not a path (as in Simple Reader).
- If no library folder has been chosen yet (the app was first started on a book), the bar offers **Choose library folder** instead, which opens the library's first-run choice.

## Servers

### Sources

- A list of OPDS sources, kept in **Settings → Catalogs**: name, server address, user name, password. Expected: CWA as the main library, the Calibre Content Server as a secondary one.
- The reader types the **server's address** (`http://host:8083`); saving finds the OPDS catalog behind it (the address itself, else `/opds`, where CWA, Calibre-Web and the Calibre Content Server keep it) and keeps that address. A refused login is reported on saving.
- One source is marked as the **progress-sync source** (KOSync; in practice CWA).
- Authentication: HTTP Basic sent with the first request (what CWA expects), Digest when the server answers with a Digest challenge (the Calibre Content Server).
- Passwords are stored in the OS credential store: Windows Credential Manager now, Android Keystore with the full Android version. The config file stays portable and holds no secrets.
- Plain `http://` is allowed. Saving a source whose host is outside private ranges, single-label home-network names, `.local` / `.lan` and Tailscale (`100.64.0.0/10`, `*.ts.net`) shows a warning.
- The app only ever requests addresses on a source's own server (scheme, host and port), so a feed or a book can't make it fetch anything else.

### Catalog browsing

- The library window has two tabs, **Library** and **Catalogs**. With several sources, a list at the top picks one.
- Navigation follows the server's OPDS feeds (new, authors, series, categories…), with **← Back** (Android's Back too), **More** for the next page of a long list, and OPDS search (the server's OpenSearch template).
- Covers in the lists, fetched through the app with the source's login.
- A mark on books the library seems to hold (same title and first author's surname: a hint; downloading checks the content).
- Download format: EPUB if available, otherwise FB2. CWA serves FB2 as `text/xml`, so the download link tells the format. (Choosing another format by hand: later.)
- Downloaded books go into the library folder under the [naming template](#file-naming), the same way as Copy / Move: a book the library already holds isn't added twice.

### Progress sync

- Only between the user's own devices (Windows ↔ Android); compatibility with KOReader or Kobo is not a goal.
- **Transport:** the KOSync protocol built into CWA, at `/kosync` on the sync source's server, with the CWA login over HTTP Basic (CWA doesn't accept KOReader's md5 key). It must be turned on in CWA's settings; when it's off, pushes and pulls fail quietly.
- **Document id:** KOReader's partial MD5 of the file (1 KiB samples at 0, 1K, 4K, 16K … 1G). Sync is therefore guaranteed only when both devices hold byte-identical files — in practice books downloaded from the same server; CWA computes the same hash for its library's files and matches them. A book without a match on the server simply does not sync; this is not an error.
- **Newer / another device:** each install has a device id; positions keep the time they were saved. A server position counts only if it comes from another device, differs from the local one and is newer.
- **Payload:** `progress` holds the foliate-js position (EPUB CFI); `percentage` is filled in truthfully, so other clients could at least jump by percent.
- **Push:** when a book is closed, when the app goes to the background, and once a minute while reading if the position changed. Closing the window waits 3 s at most for it. `--no-save` windows don't sync.
- **Pull:** when a book is opened. If the server has a different, newer position, a bar offers to jump to it, e.g. "Jump to 63% (Android, 2 h ago)?" — never a silent jump.
- **Offline:** failures are silent; pending pushes are retried later.

### Later: editing on the server (phase "e")

- Read / unread status and shelves through CWA's web endpoints (no public API, so they may break with CWA updates).
- Metadata editing through the Calibre Content Server JSON API (`/cdb/set-fields`).
- Details are written down before the phase starts.

## State

- **Desktop:** one JSON config next to the exe (`breader.json`), falling back to `%APPDATA%\book-reader` when the folder is not writable (as in Simple Reader). It holds settings, the library folder, sources (without passwords), reading positions, bookmarks, window geometry.
- **Android:** the same data in the app's private storage.
- **Book identity:** content fingerprint (from Simple Reader); the KOSync hash is computed separately for sync.
- **Library cache:** metadata and covers, rebuilt by rescanning.

## Technology

- **Shell:** Tauri 2 (Rust) on both platforms; WebView2 on Windows, system WebView on Android.
- **Frontend:** plain TypeScript, no framework.
- **Rendering:** [foliate-js](https://github.com/johnfactotum/foliate-js) (MIT), vendored; modules not needed (PDF, MOBI, comics, TTS) are removed.
- **Removed from Simple Reader:** Markdown support with marked, Mermaid and highlight.js.
- **Native pieces** (All files access, credential stores, keep-screen-on, volume keys) via small app-level Tauri plugins in Kotlin (`SystemPlugin.kt`) called from Rust (`platform_android.rs`).
- **One crate for both platforms:** `src-tauri` builds the Windows exe and the Android library; desktop-only parts (file associations, the console, single-instance, the command line, window placement) are behind `cfg(desktop)` / `cfg(windows)`.
- **Build:** Windows locally, `npm run build:win` → portable `breader.exe`. Android in GitHub Actions only (workflow *Android*): the Android SDK and NDK have no Windows-on-ARM64 builds. Each push that touches the app builds an arm64 APK (an artifact), installed with `adb install`. The Gradle project (`src-tauri/gen/android`) was made by `tauri android init` and is kept in the repo, changed by hand from there.
- **Version control:** private GitHub repository `OlegG90/book-reader`.

## Testing

- Automated tests for the app's own logic: config and data location, FB2 encodings, fingerprinting, the naming template (authors, series, invalid characters, collisions), library scanning, OPDS feed parsing, KOSync client and conflict rule, Digest/Basic auth.
- Rendering and touch interaction are checked by hand against sample files (EPUB, windows-1251 FB2, `.fb2.zip`) and a real CWA / Calibre instance.

See [roadmap.md](roadmap.md) for the stages and open questions.
