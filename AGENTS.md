# Working on Book Reader

Instructions for coding agents (and people) changing this repository. `CLAUDE.md` adds what is specific
to Claude Code.

## The project

Book Reader (`breader.exe`) is a compact FB2 / EPUB reader for Windows and Android. It keeps a local
library folder, reads OPDS catalogs (Calibre-Web-Automated, the Calibre Content Server) and syncs reading
progress through KOSync. It started as a fork of Simple Reader. The source of truth is
[docs/spec.md](docs/spec.md) (behaviour) together with [docs/roadmap.md](docs/roadmap.md) (stages, open
questions). The UI design and its decisions are in [docs/design/ui-refresh.md](docs/design/ui-refresh.md).

| Path | What |
|---|---|
| `src` | Frontend: TypeScript with no framework, one page (`index.html`), styles in `src/style.css` |
| `src-tauri/src` | Rust backend (Tauri 2). One crate serves Windows and Android, split with `cfg(desktop)` / `cfg(mobile)` |
| `src-tauri/src/platform_desktop.rs`, `platform_android.rs` | Platform code; both files offer the same functions |
| `src-tauri/gen/android` | Gradle project (made by `tauri android init`, kept and edited by hand: `SystemPlugin.kt`, the manifest, signing) |
| `docs` | Spec, roadmap, design |
| `samples` | Sample books for development (binary in git) |
| `scripts` | `clean.mjs`, the FB2 sample generator |
| `.github/workflows` | `android.yml` (APK), `windows.yml` (exes and tests), `release.yml` (releases) |

The reading part of the frontend (BookView, touch, appearance, layout, page labels, DOM helpers such as
`el()`, the vendored foliate-js) is the separate public package
[reader-core](https://github.com/OlegG90/reader-core), shared with Simple Reader. Change it there, not in
`node_modules`.

## Rules

- **English** for everything in the repository: code, comments, docs, commit messages, PRs.
- **The spec changes with the code.** A change in behaviour updates `docs/spec.md` (and
  `docs/roadmap.md` when a stage moves) in the same branch.
- **Write like the surrounding code:** its naming, its comment density and its style. Every function
  gets a doc comment that says what it does for the reader in plain sentences, not how.
- **No formatters.** The project uses neither `rustfmt` / `cargo fmt` nor Prettier. Running them
  rewrites files that the change never touched. Keep each file's line endings.
- **The frontend never passes file paths.** It names books by keys: a library key, a recent-book
  fingerprint, a window's current book. The backend resolves them, so book content can't make the app
  read arbitrary files.
- **Keyboard shortcuts must work in any keyboard layout.** Use `e.key` when it is a printable ASCII
  character, else `e.code` (see `src/keys.ts`).
- Icons come from the sprite in `index.html` via `icon(name)` (`src/icons.ts`).
- Colours, fonts and radii are tokens on `:root` in `style.css`, defined per theme (light, sepia,
  dark). Don't hard-code colours in rules.
- Never commit secrets: keystores, passwords, tokens, catalog credentials or personal book files.
- Delete or overwrite files, branches, tags or data only after the maintainer confirms.

## Workflow

Every change goes through a branch and a pull request; nothing is committed to `main` directly.

1. **Sync:** `git fetch`, switch to `main`, `git pull --ff-only`. If the checkout has uncommitted
   changes, stop and ask; never stash or discard them silently.
2. **Branch** from the updated `main`: `feat/…`, `fix/…`, `docs/…`, `ci/…`.
3. **Implement** on that branch, with tests where logic changes (vitest next to the module, `cargo test`
   in the Rust module), and the spec updated.
4. **Review** the branch diff for bugs against the spec, and fix what the review confirms.
5. **Simplify:** a quality pass over the diff (reuse, simplification, efficiency, depth of the fix),
   with no change in behaviour.
6. **Pull request** against `main`. Say what changed and why, and how it was verified: tests, plus what
   was checked by hand on Windows and on the phone. Note any breaking change, such as renamed commands or
   settings migrations, and list review points left out on purpose.
7. **Merging is the maintainer's call.** Before anything that depends on a merge (a tag, deleting a
   branch), check that `gh pr view <n> --json state` says `MERGED`. After the merge, sync `main` and
   delete the branch.

## Building and testing

```
npm install
npx tauri dev -- -- samples/alice-in-wonderland.epub   # dev app (Vite on port 1420) with a sample book
npm test                 # vitest, then cargo test
npx tsc --noEmit         # types
cd src-tauri && cargo clippy
npm run build:x64        # src-tauri/target/x86_64-pc-windows-msvc/release/breader.exe
npm run build:arm64      # src-tauri/target/aarch64-pc-windows-msvc/release/breader.exe
npm run build            # this machine's architecture
npm run clean            # removes dist and src-tauri/target
```

- **Toolchain:** Node.js, Rust (MSVC), and Visual Studio Build Tools with the C++ workload. For ARM64,
  also LLVM (`winget install LLVM.LLVM`): the TLS library (ring, under rustls) compiles C with clang, and
  `src-tauri/.cargo/config.toml` points at `C:/Program Files/LLVM`.
- **Dev profile:** the dev app keeps its settings and positions in `src-tauri/target/debug/breader.json`.
  A release exe keeps them in `breader.json` next to itself, or in `%APPDATA%` if that folder is
  read-only.
- **Vite** watches only the page: `vite.config.ts` ignores `src-tauri`, `docs` and `samples`, and limits
  the dependency scan to `index.html`. Keep it that way, or the dev window can stay blank for a long time.
- `build.rs` writes the library list for `--version` from `Cargo.toml`/`Cargo.lock`, `package.json` and
  reader-core's `foliate-js` field.
- **Check UI changes in the running app,** on Windows and, when they touch the phone, on the phone,
  before the pull request.

### Android

- **The APK is built only in GitHub Actions** (workflow *Android*, on every push that touches the app).
  The Android SDK and NDK have no Windows-on-ARM64 builds, and the TLS library needs the NDK's clang, so
  Android compile errors show up in CI. Watch the run that the push started; don't dispatch another.
- The APK is a release build **signed with the personal release key** (repository secrets
  `ANDROID_KEYSTORE`, base64 of the `.jks` with alias `book-reader`, and `ANDROID_KEYSTORE_PASSWORD`). The
  workflow fails without them. **Never generate a new key:** an APK signed with another key won't install
  over the app, and uninstalling wipes the app's data on the phone.
- Install with `adb install -r <apk>` (the artifact `book-reader-apk`).
- On the phone the library is `Documents/Books`, reached through *All files access*. The minimum is
  Android 11, so `minSdk` is 30.

### reader-core

reader-core is consumed as `git+https://github.com/OlegG90/reader-core.git#vX.Y.Z`. To change it:

1. Open a PR in reader-core; the maintainer merges it.
2. Tag the merge commit, only after checking that the PR is `MERGED`.
3. Bump each app that uses it (this one and Simple Reader).

npm 12 (`allow-git=root` in `.npmrc`) won't move an installed git dependency to another tag, so bump it
with:

```
npm uninstall reader-core
npm install git+https://github.com/OlegG90/reader-core.git#vX.Y.Z
```

`tsconfig.json` sets `maxNodeModuleJsDepth: 3` so TypeScript reads the types of reader-core's
JavaScript (foliate-js).

## Versions and releases

- The version lives only in `src-tauri/Cargo.toml`. About and `breader --version` show it, and
  Android's version code follows it.
- *Windows* (`windows.yml`) runs the tests and builds `breader.exe` for x64 and ARM64 on every push that
  touches the app (artifact `book-reader-windows`).
- To release:
  1. Bump the version on a branch, open a PR, and wait for the maintainer to merge it.
  2. Tag `main` with `vX.Y.Z` (annotated) and push the tag.
  3. *Release* (`release.yml`) checks that the tag matches the version, then publishes
     `breader-x64.exe`, `breader-arm64.exe` and the signed `book-reader-android.apk` to a GitHub Release.
- Tags are cut from `main` only, after the merge.
