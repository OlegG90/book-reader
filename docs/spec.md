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
- **Bookmarks:** new. Local to each device.
- **Reading position:** saved automatically, synced between devices through the server (see [Progress sync](#progress-sync)).

### Android interaction

- Tap zones: left third — previous page, right third — next page, centre — menu.
- Swipe to turn pages.
- Volume keys turn pages (a setting, off by default).
- Full screen with the status bar hidden while reading.
- The screen stays on while a book is open.
- Orientation follows the system; no orientation lock in the first version.

## Library

### Library folder

- **One** library folder, stored in the config.
- **First run** asks which folder to use as the library:
  - Desktop: default is `lib` next to the exe. If the exe's folder is not writable, the default becomes `%USERPROFILE%\Documents\Book Reader`.
  - Android: the system folder picker (Storage Access Framework), suggesting `Documents/Books`. "Skip" uses the app's private storage, with a warning that those books are deleted together with the app. If SAF turns out too painful in Tauri, the fallback is the "All files access" permission (acceptable for a sideloaded personal app).
- The folder is scanned recursively for supported formats; metadata (title, author, series, cover) is cached.

### Library screen

- Grid or list of books with covers; search; sort; filter by reading status (not started / reading / finished).
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
- Characters not allowed in file names (`\/:*?"<>|`) are replaced with `_`; names are truncated to a reasonable length.
- **Collision:** if a file with the same name but different content exists, ` (2)`, ` (3)`… is appended. If the content is identical (same fingerprint), nothing is copied and the existing book is offered instead.
- **Existing files** in the chosen folder are never renamed automatically. A manual "Organize library" command shows a preview of all renames and applies them only after confirmation.

### Files outside the library (desktop)

Opening a file that is not inside the library folder shows an unobtrusive bar: **Copy to library**, **Move to library**, **Not now**, **Never for this file**.

- The last choice between copy and move becomes the primary button; both are always shown.
- A global setting turns the offer off.
- After a move the window keeps showing the book from its new path.
- The reading position survives copying and moving: book identity is a content fingerprint, not a path (as in Simple Reader).

## Servers

### Sources

- A list of OPDS sources, each with a name, URL, user name and password. Expected: CWA as the main library, the Calibre Content Server as a secondary one.
- One source is marked as the **progress-sync source** (KOSync; in practice CWA).
- Authentication: HTTP Basic (CWA) and Digest (Calibre Content Server).
- Passwords are stored in the OS credential store: Windows Credential Manager, Android Keystore. The config file stays portable and holds no secrets.
- Plain `http://` is allowed. Saving a source whose host is outside private ranges and Tailscale (`100.64.0.0/10`, `*.ts.net`) shows a warning.

### Catalog browsing

- Navigation follows the server's OPDS feeds (new, authors, series, categories…), plus OPDS search.
- Covers in the lists.
- A mark on books that are already in the local library.
- Download format: EPUB if available, otherwise FB2; another format can be chosen by hand.
- Downloaded books go into the library folder under the [naming template](#file-naming).

### Progress sync

- Only between the user's own devices (Windows ↔ Android); compatibility with KOReader or Kobo is not a goal.
- **Transport:** the KOSync protocol built into CWA.
- **Document id:** the KOSync hash of the file. Sync is therefore guaranteed only when both devices hold byte-identical files — in practice books downloaded from the same server. A book without a match on the server simply does not sync; this is not an error.
- **Payload:** `progress` holds the foliate-js position (EPUB CFI); `percentage` is filled in truthfully, so other clients could at least jump by percent.
- **Push:** when a book is closed, when the app goes to the background, and once a minute while reading if the position changed.
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
- **Native pieces** (SAF, credential stores, keep-screen-on, volume keys) via Tauri plugins or small custom Kotlin / Rust code.
- **Build:** local scripts, `npm run build:win` → portable `breader.exe`, `npm run build:android` → signed APK. CI on GitHub may be added later.
- **Version control:** local git repository; a GitHub repository is optional later.

## Testing

- Automated tests for the app's own logic: config and data location, FB2 encodings, fingerprinting, the naming template (authors, series, invalid characters, collisions), library scanning, OPDS feed parsing, KOSync client and conflict rule, Digest/Basic auth.
- Rendering and touch interaction are checked by hand against sample files (EPUB, windows-1251 FB2, `.fb2.zip`) and a real CWA / Calibre instance.

See [roadmap.md](roadmap.md) for the stages and open questions.
