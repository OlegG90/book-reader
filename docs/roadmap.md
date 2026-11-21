# Book Reader — Roadmap

## Stages

1. **Desktop + library** (Windows only)
   - Fork Simple Reader into this repository; rename to Book Reader / `breader.exe`.
   - Remove Markdown (marked, Mermaid, highlight.js) and unused foliate-js modules.
   - First-run library folder choice, library scan and metadata cache, library screen.
   - Naming template, Copy / Move to library, "Organize library" with preview.
   - Local bookmarks.
2. **Android spike** — the riskiest part, checked early: build the Tauri app for Android, pick a folder through SAF, open an EPUB and an FB2 from it and turn pages. Decide SAF vs "All files access".
3. **Servers** (on Windows)
   - OPDS sources (Basic / Digest auth, credentials in the OS store), catalog browsing, search, download into the library.
   - KOSync progress sync against CWA.
4. **Full Android version** — library, tap zones / swipe / volume keys, full screen, keep screen on, servers and sync with Windows, signed APK.
5. **Shared core** — extract the rendering core shared with Simple Reader once its API has survived Android.
6. **Phase "e"** — read status and shelves in CWA, metadata editing through the Calibre Content Server.

Full-text search in a book and grouping by series fit after stage 4.

## Open questions

- **Bookmark sync:** KOSync carries only the position; syncing bookmarks needs another mechanism.
- **Phase "e" details:** which fields, which server, how fragile the CWA web endpoints are.
- **Shared core with Simple Reader:** package boundary and how both repositories consume it (`file:` dependency, git submodule).
- **Android library storage:** SAF or "All files access" — settled by the stage 2 spike.
