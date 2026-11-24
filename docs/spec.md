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

- `.fb2`, `.fb2.zip` and `.fbz` — encoding taken from the XML declaration (UTF-8, windows-1251 and others)
- `.epub` — EPUB 2 and EPUB 3, reflowable

Both are layer-1 modules (`format:epub`, `format:fb2`) and can be turned off in Settings › Modules. A disabled format is skipped by the scan, refused by picking, opening and catalog downloads with a sentence, and left out of the file dialog; books already in the library stay visible but refuse to open.

### Out of scope

Markdown, fixed-layout EPUB, DRM-protected books, PDF, MOBI/AZW3, TXT, highlights and notes, dictionary / translation, text-to-speech, format conversion, syncing bookmarks or appearance settings, more than one library folder. Full-text search comes later (second priority).

### Optional modules

Layer 0 (the core) is never modular: book identity and fingerprints, positions and bookmarks, the library scan and cache, the naming template, the network policy, the settings merge across windows. Layer 1 can be turned off in Settings › Modules. The section lists every module with its switch, grouped into formats and capabilities, and says why a switch is locked. The settings keep `disabledModules`: a list of module ids (missing or corrupt entries are tolerated on load, like every other setting; writes refuse unknown, mandatory and in-use modules with a sentence).

A disabled module removes its interface entries and refuses its actions with a sentence (never a broken button):

- `"<Label> is turned off in Settings > Modules"` — every refused action, e.g. `FB2 is turned off in Settings > Modules` when opening, picking, adding or downloading an FB2 book with the format off; `Catalogs is turned off in Settings > Modules` for every `catalog_*` action.
- `"Catalogs is in use: delete its catalog sources first."`, `"Catalogs is in use: turn off \"Progress sync\" first."` — turning `catalogs` off is refused while sources exist or progress sync is still on.

Off behavior per module:

- `format:epub`, `format:fb2` (covers `.fb2/.fb2.zip/.fbz`) — reading formats (see [Supported formats](#supported-formats)).
- `capability:catalogs` — OPDS browsing and downloads. Turned off, the Catalogs page, its navigation items and the settings tab are gone.
- `capability:progress-sync` — the user's intent ("sync my positions"); the KOSync transport behind it needs catalogs, and each future transport declares its own `requires` edge instead. Turning it off stops push/pull and hides the sync badge and the jump offer; books still open, positions still save locally. A source marked for sync reports `Progress sync is turned off` instead of failing.
- `capability:organize` — the Organize button leaves Settings › System; preview and apply refuse.
- `capability:file-types` — the File types row leaves Settings › System (on a phone it is never offered); registering and unregistering refuse.
- `capability:update-check` — the Updates row leaves Settings › About; the check and the release page refuse.

No cloud stores, no `reader-core` changes: the modules only switch the app's own areas on and off.

## Reading

Inherited from Simple Reader unless noted:

- **Modes:** paginated (default) and scrolled.
- **Appearance:** theme (light / dark / sepia, follows the system by default; chosen with page swatches), font, font size, line spacing, line length. In a book they are in the **Aa** pop-up (`S` / `Ctrl+,`, or the Aa button; a sheet from the bottom on the phone), with Paginated / Scrolled and **All settings…**; in the library, under Settings › Reading. Settings are per device and not synced.
- **Reading chrome:** a slim top bar (← Library, Contents · the book's title and author · Bookmark, Aa, the mode, ⋯ with Shortcuts, About, Full screen) and a bottom bar: the chapter · a track with a tick at each chapter and a knob (click or drag to go there) · `p. 14 of 30 · 12%` (pages count within the chapter; the book-wide position is the percentage). Both come and go together: the mouse near the top or bottom edge, or a tap in the page's middle; while hidden, a hairline shows the progress. On the phone the top bar keeps ←, the title and the bookmark, and the bottom bar adds Contents / Appearance / Mode / More under the slider.
- **Navigation:** a drawer with **Contents | Bookmarks (n)** (`T`; the current chapter in the accent; the last tab stays for the session), the reading chrome's progress row, footnotes in a pop-up, images.
- **Bookmarks:** new. Local to each device. `B` (or the top bar's bookmark, filled when the page has one) bookmarks the page or removes the bookmark on it; the list is the drawer's Bookmarks tab. A bookmark is a point in the text, so it stays found when the font or window size changes the pages. `--no-save` windows keep none.
- **Reading position:** saved automatically, synced between devices through the server (see [Progress sync](#progress-sync)).

### Android interaction

- Tap zones: left third — previous page, right third — next page, centre — shows or hides the top and bottom bars.
- Swipe to turn pages.
- Volume keys turn pages while a book is open (a setting, off by default).
- Full screen with the status bar hidden while reading (a swipe from the top shows it for a moment).
- The screen stays on while a book is open.
- Orientation follows the system; no orientation lock in the first version.
- One window: a book opens in place of the library, and the library comes back in its place (the desktop keeps a window per book).
- Back, one step at a time: closes a footnote or a panel (contents, Aa, the ⋯ menu, settings, Organize), then returns from the book to the library; in the library it goes up a catalog folder, then from the catalogs or the settings to the books; at the books it sends the app to the background, as Android apps do.

## Library

### Library folder

- **One** library folder, stored in the config.
- **First run** asks which folder to use as the library:
  - Desktop: default is `lib` next to the exe. If the exe's folder is not writable, the default becomes `%USERPROFILE%\Documents\Book Reader`.
  - Android: `Documents/Books` in shared storage, through the **All files access** permission (`MANAGE_EXTERNAL_STORAGE`; fine for a sideloaded app, not allowed on Google Play). Android 11 (API 30) is the minimum: All files access starts there. The first-run button opens Android's setting for it; after turning it on the reader comes back and taps the button again. With it on, the library is a plain folder that the same Rust code scans and organizes as on Windows; if the access is later turned off, the library screen says how to turn it back on. **Change…** (in the settings, and on the first-run screen) opens Android's folder picker; the folder must be on the phone's own storage (not a memory card) and is then used as a plain path. The Storage Access Framework was not chosen as the library's backend: every file operation would have to go through `content://` URIs in Kotlin, a second library backend.
- The folder is scanned recursively for supported formats; metadata (title, author, series, cover) is cached.

### Library screen

- **Continue reading** and **Library** are separate pages. Continue reading shows the last books read (1–5, a setting; 3 by default; the app remembers the last 10) as cover cards; the library opens on it when there is a history, else on the books. A book there opens at its saved place, without a question.
- Library: a grid of books with covers and how far each is read as a line under the cover (`New`, `NN%`, `Finished`); a book in a series shows the series and its number in it, and a long series name is cut short so the number always shows; search (title, author, series); sort by author (series in reading order), title or recently added; a switch by reading status with counts: all, not started, reading, finished (98% or more, since books often end in notes).
- Opening a book from Library: a book already started asks first — **Continue · NN%** or **Start from the beginning** (the saved position stays until reading on saves a new one; a window already showing the book reads it again from the start). A new book just opens.
- A book's menu (right click on Windows, long press on the phone) has the same choices plus **Delete from library…**, which asks first: on Windows the file goes to the Recycle Bin, on the phone it is deleted. The reading position and bookmarks stay (they belong to the book's content), so the book picks up where it was if it comes back; Continue reading forgets it.
- **Series**: a page listing the library's series as cover cards (the first book's cover, how many books, how many of them finished, with a line for that); a series opens to its books in reading order, with a breadcrumb back to the list. Books without a series aren't there.
- "Recently added" is when the book came into the library (remembered by the scan cache), not the file date, which a copy keeps.
- Navigation, desktop: the library window has a navigation pane (WinUI style) — Continue reading, Library, Series, Catalogs (see [Servers](#servers)), and Settings folding out to Reading, Catalogs, Modules, System, Shortcuts and About; the content beside it shows the one chosen. The pane's edge drags to resize it (the width is kept in this PC's WebView storage, not the config; a double click resets it). Esc closes a book's menu, goes up a catalog folder or from a series to the list of them, then back to the books; `S` / `Ctrl+,` open the settings and go back to the page they left.
- **Settings → About:** the version; **Open on GitHub** opens the repository in the browser; **Check for updates** asks GitHub's API for the latest release, only when pressed (GitHub sees the device's address and the app's name and version, `BookReader/X.Y.Z`, nothing else), and says whether this copy is the latest; for a newer one, **Open the release** opens its page, with the exe and APK files. The app itself downloads and installs nothing.
- Navigation, phone: a bottom bar — Reading (the history), Library, Series, Catalogs, Settings (Back steps back the way Esc does); the settings' sections are chips at the top of their page, with arrows on the edges while more of them hide outside.
- Settings › System groups its rows: **Library** (the library folder with **Change…** and **Organize…**, the offer for books from elsewhere), **History** (how many books Continue reading shows, clearing the history), **Sync** (the cloud connection with Disconnect, adding a cloud with its picker); file types and the interface font (Classical — Lora with Cormorant Garamond headings — or System, or Georgia, Cambria, Segoe UI, Calibri on Windows) stand on their own.
- Desktop: each book opens in its own window (as in Simple Reader); the settings there are a page over the window (from Aa's **All settings…**).

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

### Files outside the library

Opening a file that is not inside the library folder shows an unobtrusive bar: **Move to library** and **Copy to library**, **Not now**, and **⋯**, which shows **Never for this file**.

- The last choice between copy and move comes first, outlined in the accent; both are always shown.
- A setting (Settings › System) turns the offer off.
- After a move the window keeps showing the book from its new path.
- The reading position survives copying and moving: book identity is a content fingerprint, not a path (as in Simple Reader).
- If no library folder has been chosen yet (the app was first started on a book), the bar offers **Choose library folder** instead, which opens the library's first-run choice.
- **Android:** books come from **Open with** / **Share** (EPUB and FB2; FB2 has no settled MIME type, so `application/octet-stream` is accepted too) and from **Add book…** in the library (Android's document picker). Android hands over a `content://` document, not a path, so the book is copied into the app's own storage (the 20 newest copies are kept, so **Not now** and the recent books keep working) and opened from there, with the same bar. Copy and Move both put the copy into the library; Move then also deletes the original where Android allows it, and says so when it can't (a document from another app's storage may stay, and after a restart the original is no longer known). When the library already holds the book, nothing is deleted.

## Servers

### Sources

- A list of OPDS sources, kept in **Settings → Catalogs**: name, server address, user name, password. Each catalog is a row like the other settings: its name and address, then **Edit** and **Remove**. Expected: CWA as the main library, the Calibre Content Server as a secondary one.
- The reader types the **server's address** (`http://host:8083`); saving finds the OPDS catalog behind it (the address itself, else `/opds`, where CWA, Calibre-Web and the Calibre Content Server keep it) and keeps that address. A refused login is reported on saving.
- One source is marked as the **progress-sync source** (KOSync; in practice CWA).
- Authentication: HTTP Basic sent with the first request (what CWA expects), Digest when the server answers with a Digest challenge (the Calibre Content Server).
- Passwords are stored in the OS credential store: the Windows Credential Manager, and on Android a key in the Android Keystore that encrypts them in the app's private storage. The config file stays portable and holds no secrets.
- Plain `http://` is allowed. Saving a source whose host is outside private ranges, single-label home-network names, `.local` / `.lan` and Tailscale (`100.64.0.0/10`, `*.ts.net`) shows a warning.
- The app only ever requests addresses on a source's own server (scheme, host and port), so a feed or a book can't make it fetch anything else.

### Catalog browsing

- **Catalogs** is a page of the library (the pane's item; the phone's bottom bar). With several sources, a list at the top picks one.
- Navigation follows the server's OPDS feeds (new, authors, series, categories…), with breadcrumbs of the way (the catalog's name, then each folder as it was named; each a step back; Esc and Android's Back go up one), **More** for the next page of a long list, and OPDS search (the server's OpenSearch template).
- Covers in the lists, fetched through the app with the source's login.
- An **In library** tag on books the library seems to hold (same title and first author's surname: a hint; downloading checks the content); outlined download buttons.
- Download format: EPUB if available, otherwise FB2. CWA serves FB2 as `text/xml`, so the download link tells the format. (Choosing another format by hand: later.)
- Downloaded books go into the library folder under the [naming template](#file-naming), the same way as Copy / Move: a book the library already holds isn't added twice.

### Progress sync

- Only between the user's own devices (Windows ↔ Android); compatibility with KOReader or Kobo is not a goal.
- **Transport:** the KOSync protocol built into CWA, at `/kosync` on the sync source's server, with the CWA login over HTTP Basic (CWA doesn't accept KOReader's md5 key). It must be turned on in CWA's settings; when it's off, pushes and pulls fail quietly.
- **Transports:** sync fans out to every enabled transport (KOSync, Dropbox, …). A dead transport reports nothing instead of failing the pull, and a failed push is retried for that transport alone — one down never blocks the others. When several transports answer, the newest position wins (ties keep the first); the device and newness rules below apply to the winner. Local saving never waits for any transport.
- **Where sync shows:** every transport has a switch in Settings › Modules with a locked-reason. The `sync` badge on a catalog source means KOSync syncs through it; clouds need no per-source badge (a cloud syncs every local book, its state lives in the System rows). The Catalogs status line names the blocker when the intent is off (`Progress sync is turned off`) or when nothing can sync (no sync-marked source and no connected cloud).
- **Dropbox:** the first cloud transport (`capability:dropbox`), independent of catalogs. Settings › System shows two rows: `Cloud connection` (which cloud is connected, with Disconnect) and `Add cloud` (a cloud picker with Connect: Dropbox and Google Drive; OneDrive is listed as coming soon and stays disabled). One cloud at a time: the picker locks while one is connected, and a cloud whose module is off cannot connect. Login once through Dropbox in the browser (OAuth PKCE over a loopback address, always `http://localhost:51943/dropbox` — register exactly this URI in the App Console with "Allow public clients" on, as the app has no secret); the refresh token is kept like a catalog password, never in the config (the app key lives in `dropbox::APP_KEY`). Positions live in `/sync-state.json` in the app's own Dropbox folder, keyed by fingerprint exactly like local ones; a push downloads, merges newest-per-book and uploads only on change, so airplane-mode edits converge on the next push. Works with catalogs turned off.
- **Google Drive:** the second cloud transport (`capability:googledrive`), independent of catalogs. The same shape as Dropbox: OAuth 2.0 for Desktop apps (external browser, loopback `http://localhost:51944/googledrive` — Desktop-type clients accept any loopback port, so it needs no registration; PKCE S256, offline access with consent), the refresh token kept like a catalog password, `sync-state.json` in the hidden per-app folder (`appDataFolder`, the narrow `drive.appdata` scope), newest-wins merge, offline edits converge on the next push. Client IDs bake in at build time from the environment, never committed. The phone uses its own Android OAuth client (package plus release-key SHA-1 in the console): the sign-in answer comes back on the app's scheme (`com.olegg90.bookreader:/oauth2redirect`, an intent filter) instead of loopback, and that client has no secret. Works with catalogs turned off.
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

- **Desktop:** one JSON config next to the exe (`breader.json`), falling back to `%APPDATA%\book-reader` when the folder is not writable (as in Simple Reader). It holds settings, the library folder, sources (without passwords), reading positions, bookmarks, window geometry. (The library pane's width is kept in the WebView's storage on that PC instead.)
- **Android:** the same data in the app's private storage. Catalog passwords are encrypted with a key kept in the Android Keystore.
- **Book identity:** content fingerprint (from Simple Reader); the KOSync hash is computed separately for sync.
- **Library cache:** metadata and covers, rebuilt by rescanning.

## Technology

- **Shell:** Tauri 2 (Rust) on both platforms; WebView2 on Windows, system WebView on Android.
- **Frontend:** plain TypeScript, no framework. The look follows `docs/design/ui-refresh.md` (the "Classical" tokens: warm neutrals, one gold accent used as strokes). Interface fonts Cormorant Garamond and Lora, bundled (woff2 from the Fontsource packages, Latin and Cyrillic only; their SIL OFL 1.1 licences ship in `licenses/`), never loaded from the internet; icons from Lucide (ISC), one inline SVG sprite.
- **Keyboard:** shortcuts read what a key types when that is Latin (AZERTY and QWERTZ keep their letters) and the key's place on the keyboard otherwise, so `T`, `B`, `S`, `Ctrl+,`… work in a Ukrainian layout too.
- **Rendering:** the shared [reader-core](https://github.com/OlegG90/reader-core) package (public, MIT), also used by Simple Reader: `BookView` over [foliate-js](https://github.com/johnfactotum/foliate-js) (vendored there, without PDF, MOBI, comics, fixed-layout EPUB and TTS), appearance, page layout, touch. An npm git dependency pinned to a tag (`git+https://github.com/OlegG90/reader-core.git#vX.Y.Z`; `.npmrc` allows this one git dependency, which npm 12 refuses by default). Key bindings, settings, the library and everything else stay in the app.
- **Removed from Simple Reader:** Markdown support with marked, Mermaid and highlight.js.
- **Native pieces** (All files access, credential stores, keep-screen-on, volume keys) via small app-level Tauri plugins in Kotlin (`SystemPlugin.kt`) called from Rust (`platform_android.rs`).
- **One crate for both platforms:** `src-tauri` builds the Windows exe and the Android library; desktop-only parts (file associations, the console, single-instance, the command line, window placement) are behind `cfg(desktop)` / `cfg(windows)`.
- **Build:** Windows locally, `npm run build:win` → portable `breader.exe`. Android in GitHub Actions only (workflow *Android*): the Android SDK and NDK have no Windows-on-ARM64 builds. Each push that touches the app builds an arm64 release APK (an artifact), signed with a personal release key kept in the repository's secrets (`ANDROID_KEYSTORE`, `ANDROID_KEYSTORE_PASSWORD`; alias `book-reader`, one password for the store and the key; a backup copy lives outside the repo; without the secrets the build fails), so a new APK installs over the previous one with `adb install -r`. The Gradle project (`src-tauri/gen/android`) was made by `tauri android init` and is kept in the repo, changed by hand from there.
- **Version control:** GitHub repository `OlegG90/book-reader`.

## Testing

- Automated tests for the app's own logic: config and data location, FB2 encodings, fingerprinting, the naming template (authors, series, invalid characters, collisions), library scanning, OPDS feed parsing, KOSync client and conflict rule, Digest/Basic auth.
- Rendering and touch interaction are checked by hand against sample files (EPUB, windows-1251 FB2, `.fb2.zip`) and a real CWA / Calibre instance.

See [roadmap.md](roadmap.md) for the stages and open questions.
