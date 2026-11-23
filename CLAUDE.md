@AGENTS.md

## Claude Code

- Talk to the maintainer in Ukrainian; everything written into the repository stays in English.
- Step 4 of the workflow is `/code-review` against `main` (Standards and Spec, with the spec in
  `docs/spec.md`), and step 5 is `/simplify`. Tell their sub-agents that the project uses no formatters.
- Sub-agents and parallel sessions never switch branches in a shared checkout; work that needs its own
  branch gets its own worktree.
- Installing tools or packages needs the maintainer's approval. The maintainer types passwords into the
  app themselves; never handle them in chat.
- When an APK is waiting to be built, watch the run the push started (`gh run watch`) rather than
  dispatching another one.
- Edit files with the Edit / Write tools, or with Python scripts written to a file and then run. Inline
  heredocs break on quotes in the code.
- Checking the desktop app: computer use knows the dev build as **Breader** (grant it by the exe path
  `src-tauri/target/debug/breader.exe`). Windows' Click to Do overlay can take focus; bring the app
  forward again with `open_application`. The keyboard layout may be Ukrainian.

## Project notes (Obsidian)

The maintainer keeps the project's notes in Ukrainian in the Obsidian vault `myVault`
(`C:\Workspace\Library\Obsidian\myVault`), in the folder `40 Проєкти/Book Reader/`:

- `Book Reader.md` is the overview and describes the current state only. Its «Стан» section lists the
  stages and pull requests.
- `Board/Milestones/M-NN/` mirrors the stages in `docs/roadmap.md`. Tickets are `NNN - Назва.md` with
  `ticket_status` (`todo` / `progress` / `done`), `start_date` and `end_date`.
- `Doc/` is a junction to this repository's `docs/`. Never edit the spec through it.
- The folder's own rules are in `40 Проєкти/Book Reader/AGENTS.md`.

After a stage or a notable change (a PR opened or merged, a release), update the ticket statuses and the
overview's «Стан» section.
