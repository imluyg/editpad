# Editpad

> Lightweight text editor · Rust + iced custom rendering · smooth on large files · Chinese IME capable

[简体中文](README.md) | **English**

A windowed notepad built from scratch in Rust: multi-tab, session snapshot restore, and a custom virtualized editor. The hard target is **opening a 50MB file instantly and never lagging**; in practice a 68MB / 600k-line log scrolls and edits just fine.

## Highlights

- **Large files** — rope storage + background streaming load + viewport-virtualized rendering, so a 68MB file costs the same per frame as a 5KB one;
- **Chinese-friendly** — system IME inline composition (underline, following text yields, cursor advances with the composition), CJK double-width alignment layered on real glyph metrics, so the cursor, clicks, and selection never drift;
- **Never lose work** — closing a dirty window asks nothing: snapshots are flushed write-ahead and it exits directly, and the last session is restored as-is on next launch; a runtime heartbeat does incremental backups, so a crash loses at most one interval;
- **Every key rebindable** — all 75 actions can be remapped in Settings; the `Ctrl+E` command palette reaches any command by fuzzy search;
- **Portable distribution** — a single exe; config and snapshots are isolated by the exe's location, so multiple copies don't interfere with one another.

## Quick Start

Environment: Windows 10/11 + Rust stable (toolchain locked by `rust-toolchain.toml`).

```powershell
cargo run --release          # run
cargo test --workspace       # full test run (core unit tests + app-layer tests + fuzz differential)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt                    # format (rustfmt.toml)
./package.ps1                # release packaging: build → stage → zip → SHA256
```

Large-file acceptance: open `dev-assets/bench-50mb.log` (68MB / 600k lines) — the UI stays draggable during loading and the progress bar advances in real time; once it opens, scrolling and editing don't lag. Open `.rs` / `.py` / `.md` files to see syntax highlighting.

## Features

- **Editing**: open / edit / save (atomic write: temp file + rename), undo / redo (rope structure shares snapshots), the full cursor-navigation set with Shift selection, word navigation and word deletion, mouse click positioning / drag select / scroll-wheel scrolling, bookmarks, column (rectangular) editing, smart indent on Enter and Tab/Shift+Tab block indent, Insert overwrite mode, line operations (move up/down, duplicate, delete, merge, split, reverse, sort, dedupe, delete empty lines), case and capitalize conversions, trim leading/trailing whitespace, line comment, Tab↔space conversion, insert date/time
- **Find & locate**: find / replace (debounced background scan), regex mode (capture groups + `$1` references), whole-word matching, highlighting of all in-viewport matches + live count, Find All results panel, go to line, bracket-pair jump, recent files (cursor position remembered)
- **Chinese input**: the system IME works; the pre-edit string is shown inline — composition is underlined at the cursor, text after the composition point yields, and with wrap on the underline continues segment by segment across wrapped rows, while the candidate box follows the cursor rectangle; a CJK double-width column map is layered on real glyph layout, the column model serving only as a fallback for unshaped lines
- **Large files**: ropey document model (O(log n) edits); background-thread streaming load + encoding detection (UTF-8/BOM, UTF-16LE/BE, GBK fallback); custom virtualized rendering — layout and highlight only for the visible lines; syntect line-by-line lazy highlighting: a checkpoint-style state machine supports random viewport access, edits invalidate only the state after the change, and large jumps first show an approximate coloring immediately before the precise pass replaces it
- **Viewing**: automatic syntax coloring by extension (unknown types take the plain-text fast path), embedded mini-syntax for Log/TOML, content sniffing for extensionless files, Markdown preview panel, one-click JSON validation and formatting, invisible-character marks, light/dark themes (highlight colors are rebuilt with the theme), body font-size scaling (10–48px), soft wrap (word-boundary-preferred wrapping)
- **Text tools**: Base64 / URL encode-decode on the selection, MD5 / SHA-256 digests (zero third-party deps, pinned by standard test vectors)
- **Editing safety**: read-only lock (all content-changing actions rejected at the master gate), dirty marker (tab ● prefix + title bar), auto-save (flushed after a debounce once you stop typing), backup before save (overwrite `.bak` / timestamped history directory), external-modification detection (reject write and offer a reload prompt), per-tab close confirmation, aggregate confirmation for batch close
- **Session restore**: closing a dirty window asks nothing — snapshots flush in write-ahead order, then it exits; a runtime heartbeat does incremental backups; on startup the last UI is silently restored (tabs / cursors / scroll / unnamed content); abnormal exits are detected with a one-time restore prompt
- **Windows & tabs**: multiple tabs (double-click blank to create, × close button, middle-click close, pin, context menu, restore last closed, `Ctrl+P` fuzzy quick-switch), command palette, menu bar consolidating common commands, fixed status-bar sections, globally remappable hotkeys, window position & size memory, fullscreen / always-on-top, automatic data isolation between copies, single-instance mutex and file forwarding, window title-bar icon, open files from the command line / double-click in Explorer / "Open with" ("打开方式")
- **Log scenarios**: file watching (tail follow), files whose first line is `.LOG` auto-append a timestamp when opened, Log syntax coloring, `F5` insert date/time
- **Navigation aids**: scrollbar mark strip (orange hit / amber bookmark ticks, click to jump), indent guides, right-edge ruler column, link detection (URL / `file:///` open externally, `path:line` open in-editor with line jump), drag-and-drop of the selection (hold Ctrl on release to copy, insertion-point indicator, single undo step)
- **Column editor**: `F6` opens a dialog (also in the Edit menu) that inserts repeated text or incrementing numbers into a column block line-by-line — including the zero-width insertion column from an Alt+Shift vertical drag — with number base (dec/hex/bin/oct), negative steps, and zero-padding; each confirm is a single undo step with per-field validation
- **Per-tab display**: per-tab word wrap three-state (follow global / on / off, View menu), per-tab font-size override (Ctrl+scroll affects only the current tab, reset via View menu), both saved with the session snapshot; with soft wrap on, `Home` / `End` move by visual row and `Alt+Home` / `Alt+End` go to the logical line edges

## Keyboard Shortcuts

The following are the default combos, all remappable on the **Settings → Keyboard Shortcuts ("设置 → 快捷键")** page (click "Modify" ("修改") and press the new combo; Esc cancels; conflicts are rejected; "Restore All Defaults" ("全部恢复默认") resets everything in one click). `Enter` / `Tab` / `Insert` / `Esc` have fixed semantics and are not in the registry. If you forget a key, press `Ctrl+E` to open the command palette and fuzzy-search any command to run directly.

### Files & Tabs

| Default combo | Action |
|------|------|
| Ctrl+O / Ctrl+S | Open / Save |
| Ctrl+T / Ctrl+W / Ctrl+Tab / Ctrl+Shift+Tab | New / Close / cycle forward / cycle backward through tabs |
| Ctrl+Shift+W | Restore the last closed tab (remembers the most recent 10 named tabs in-session) |
| Ctrl+P / Ctrl+E | Quick tab switch (fuzzy) / Command palette (fuzzy-search all commands) |
| Ctrl+Shift+G / Ctrl+Shift+Q | Copy full path / Copy file name (current tab) |
| Ctrl+Shift+V | Open containing folder (locate the current file in Explorer) |
| Ctrl+R | Toggle read-only lock (edits and undo are all rejected) |
| F8 | Toggle file watching for the current tab (tail follow) |

### Editing

| Default combo | Action |
|------|------|
| Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z | Undo / Redo (two default key sets coexist) |
| Ctrl+C / Ctrl+X / Ctrl+V / Ctrl+A | Copy / Cut / Paste / Select All (copies or cuts the whole line when there's no selection) |
| Ctrl+← / Ctrl+→ | Word navigation (Shift passes through = extend selection to the word boundary) |
| Ctrl+Backspace / Ctrl+Delete | Delete to start / end of word |
| Enter / Tab / Shift+Tab | Smart-indent newline / block indent (inserts a tab when there's no selection) / outdent |
| Insert | Toggle overwrite mode (typing replaces character-by-character; paste and IME commit always insert) |
| Ctrl+D / Ctrl+L | Duplicate current line below / Delete current line |
| Ctrl+Shift+↑ / Ctrl+Shift+↓ | Move current line up / down (a multi-line selection moves as a block) |
| Alt+Shift+drag | Column (rectangular) selection: typing/backspace replaces or deletes line-by-line, Ctrl+C/X copy or cut the block, Esc cancels; a vertical drag creates a zero-width insertion column (2px indicator) = insert before that column |
| F6 | Column editor dialog: insert repeated text or incrementing numbers into the block line-by-line (base / step / zero-padding adjustable) |
| F5 | Insert current date/time at the cursor (`YYYY-MM-DD HH:MM`, local timezone) |
| Ctrl+Q | Toggle line comment (`//` `#` `--` `::` prefix auto-selected by syntax) |

### Line Operations & Text Conversion

With a selection only the touched lines are processed; without one the operation applies to the whole document. Each operation is a single undo snapshot, and bookmarks move along by the line mapping.

| Default combo | Action |
|------|------|
| Ctrl+Shift+S / Ctrl+Shift+D | Sort lines ascending / descending (code-point order, case-sensitive) |
| Ctrl+1 / Ctrl+2 | Sort by number ascending / descending (the first signed integer in the line is the key; lines with no number always sort last) |
| Ctrl+3 / Ctrl+4 | Sort by line length ascending / descending (character count is the key) |
| Ctrl+Shift+E | Reverse line order (palindromic blocks are a no-op) |
| Ctrl+Shift+K / Ctrl+Shift+Y | Remove duplicate lines (keep first occurrence) / remove consecutive duplicate lines (keep the first of each run) |
| Ctrl+Shift+N / Ctrl+Shift+R | Delete empty lines / delete blank lines (including lines containing only whitespace) |
| Ctrl+Shift+J / Ctrl+Shift+H | Merge lines / Split lines |
| Ctrl+Shift+U / Ctrl+U / Ctrl+5 | Uppercase / Lowercase / Capitalize each word |
| Ctrl+Shift+T / L / B | Trim trailing / leading / both leading & trailing whitespace (full-width spaces and NBSP count too) |
| Ctrl+Shift+I / O / P | Leading tabs to spaces / all tabs to spaces / leading spaces to tabs |
| Ctrl+6 / Ctrl+7 | Selection Base64 encode / decode |
| Ctrl+8 / Ctrl+9 | Selection URL percent-encode / decode |
| Ctrl+0 / F7 | Selection MD5 / SHA-256 digest (replaces the selection with the result) |

### Find, Bookmarks & Navigation

| Default combo | Action |
|------|------|
| Ctrl+F / F3 / Shift+F3 | Find/replace bar (auto-fills the selection if any) / Find next / Find previous |
| Ctrl+Shift+A | "Find All" ("查找全部") results panel (line:col + line excerpt, click to jump; huge result sets show only the first 500) |
| Ctrl+G | Go to line |
| Ctrl+Home / Ctrl+End | Go to start / end of document |
| Home / End | Start / end of line (with soft wrap on: start / end of the current **visual** row) |
| Alt+Home / Alt+End | Start / end of the **logical** line (crosses wrapped segments; fixed semantics, not remappable) |
| Ctrl+Shift+M | Jump to the other side of a matching bracket (works when the cursor is adjacent to `()` `[]` `{}`; both sides show an underline simultaneously) |
| Ctrl+F2 / F2 / Shift+F2 | Toggle bookmark on current line / Next / Previous (wraps around at the edges; amber dot on the left of the line-number gutter) |
| Ctrl+Shift+F2 / Ctrl+Shift+C / Ctrl+Shift+X | Clear all bookmarks / Copy all marked lines / Delete all marked lines |

### View & Window

| Default combo | Action |
|------|------|
| Ctrl+scroll / Shift+scroll | Zoom **current tab** font size (10–48px, per-tab override; the global default lives in Settings → Font) / horizontal scroll |
| F11 / F9 | Toggle fullscreen / always-on-top |
| Ctrl+Shift+F | Format JSON (JSON files only; errors point to line:col) |

The View menu also offers a per-tab word-wrap three-state switch (follow global / on / off)
and "reset tab font size"; both overrides are saved with the session snapshot.

## Interface

**Menu bar ("菜单栏")**: The top "File / Edit / View / Settings" hosts the common command entries (File: open / save / save as / recent files / restore last closed; Edit: clipboard and edit suite / find & go / timestamp / line comment; View: zoom / theme / invisible-character marks / Markdown preview / recent files panel; Settings: popup entry and backup mode). The expanded overlay keeps a stable position; clicking another menu-bar item slides straight across to it, and clicking blank space or pressing Esc collapses it. Actions that didn't make it onto the menus (read-only, watch, text tools, etc.) go through the command palette or hotkeys.

**Command palette (Ctrl+E) / quick switch (Ctrl+P)**: The data source is the full hotkey registry plus the current session's tabs, so a newly registered action automatically shows up in the palette; fuzzy matching is case-insensitive (contiguous hits and word starts are weighted); commands can be looked up by id fragment (e.g. `readonly`) and tabs by path fragment; ↑↓ selects, Enter runs, Esc closes.

**Tab context menu ("标签页右键菜单")**: 📌 Pin/Unpin (pinned tabs are exempt from single and batch close), Save, Save As / Rename, Copy Full Path, Copy File Name, Close, Close Other Tabs, Close Tabs to the Right; a batch close with unsaved changes pops one aggregate confirmation. Double-click the blank area of the tab strip to create a new tab, and middle-click closes a tab.

**Status bar ("状态栏", fixed sections)**: The far left is the file path (truncated to a fixed width); the vertical bar is followed by the persistent statistics group "Length · Lines · Line · Column · Position" ("长度 · 行数 · 行 · 列 · 位置"; Position = the cursor's character offset in the whole document, 1-based), then elastic blank space in the middle; the right vertical bar separates the fixed "Selection · Line endings · Encoding" ("选区 · 行尾 · 编码") — the two bar positions stay constant, so a selection appearing or disappearing only updates the numbers. The line-ending / encoding labels are clickable and pop a menu.

## Find & Replace

Default is literal matching; the find bar's ".* Regex" (".* 正则") toggle switches regex mode — capture groups are supported and replacement can reference `$1` (`^$` per-line anchoring requires `(?m)` on first); the "Match Case" ("区分大小写") and "Whole Word" ("整词") toggles narrow the hits as needed (whole word is not applied in regex mode, where boundary semantics are expressed by the pattern itself). The scan runs in the background debounced (200ms); all in-viewport matches get an amber background and the find bar shows a live hit count ("Scanning…" ("扫描中…") while scanning). `Ctrl+F` with a selection auto-fills its text as the query (up to 10,000 characters). An invalid regex reports an error in the status bar; regex replacement in very large documents has a character cap, beyond which it prompts you to switch to literal mode.

## Soft Wrap & Display

**Soft wrapping (word wrap)**: Settings → Appearance ("设置 → 外观"), the "Word Wrap" ("自动换行") toggle (default off; the "View" menu keeps it in sync). Lines break at real glyph pixel width with word boundaries preferred (spaces, hyphens, and CJK boundaries offer break opportunities; forbidden line-start characters are avoided), so non-monospace / CJK fonts stay accurate; the cursor, selection, bookmarks, bracket hints, line numbers, and line-end markers are all positioned by visual line; vertical motion keeps the target column; while on, the horizontal scrollbar is hidden and column editing is refused; the line end and right edge always reserve one CJK character width.

**Invisible-character marks (Settings → Appearance)**: "Show Whitespace" ("显示空白字符") draws a faint dot at space positions and a short dash for tabs; "Show Line-End Symbols" ("显示行尾符") draws a short vertical mark at each line's end. Purely a render-layer overlay; document content and cursor behavior are unchanged.

**Body font selection**: In the settings popup you can filter the installed system fonts by keyword and pick the glyph family for body and UI; "Restore Default" ("回退默认") restores the built-in monospace font. If the selected font isn't installed, this launch auto-falls-back and warns once in the status bar, without discarding the config. A monospace font that includes CJK glyphs is recommended (e.g. 更纱黑体 (Sarasa Gothic), Noto Sans Mono CJK SC, 新宋体 (NSimSun)); a non-monospace font makes column alignment look uneven, but the cursor, clicks, and selection still snap to real glyph positions.

## File Watching & Read-only

**File watching (F8)**: When on, it patrols the current tab's (mtime, size) every 2 seconds — a clean tab changed externally is silently reloaded; a dirty tab is never silently reloaded (it's still left to the prompt bar for you to decide). **Tail follow**: if the view was at the bottom before a reload, after it the cursor lands at the end and scrolls to the bottom; otherwise the pre-reload view is restored. Great for watching logs. Watch state is session-only and isn't saved into the snapshot.

**`.LOG` auto-timestamp**: A file whose first line is exactly `.LOG` gets the current date/time appended to the end when opened normally (the classic `.LOG` convention); the session-restore path doesn't append, and after appending the file is truthfully marked dirty.

**Read-only lock (Ctrl+R)**: Every content-changing action (including undo/redo) is rejected at the master gate; unrecognized new actions fail-safe to a default deny. Navigation, bookmarks, copy, and other read-only actions still work; a rejected action only warns and never marks dirty.

## Session Snapshots & Startup Restore

Closing the window with unsaved changes **no longer asks for confirmation**: all dirty tabs (including unnamed ones) are automatically written to the snapshot area (`%APPDATA%\editpad\instances\<instance key>\snapshot\`) and then it exits directly — asking "save or not?" is deferred to the next launch. Writes use a write-ahead order of page files first and the manifest second, so a power cut or crash at any moment never leaves a half-written session; when snapshots total more than 64MB the oldest are evicted first. The confirmation bar's "Discard Changes" ("放弃更改") means discarding the snapshots too.

**Reopening = the last interface**: On startup it silently rebuilds all tabs from the session manifest — unsaved content returns as-is from the snapshot (staying dirty), clean files automatically reload, and the tab order, active tab, and each tab's cursor and scroll position are all restored; unnamed-tab numbering continues. If the last run was an abnormal exit (the manifest lacks a proper shutdown marker), it pops a one-time "Detected an unsaved workspace" ("检测到未保存的工作区") prompt for you to choose restore or discard. Restore volume is bounded by a memory guardrail (256MB); when exceeded it keeps the active tab and summarizes in the status bar. If files were passed on the command line (double-click / "Open with"), session restore is skipped this launch and each file is opened directly; the snapshot is left untouched, so the next parameterless launch can still restore.

**Runtime heartbeat fallback**: While editing, every `snapshot_interval_secs` seconds (default 10, allowed 5–120) it patrols dirty tabs and incrementally writes only tabs whose content changed into the snapshot area — crash or kill mid-typing, and after restart you lose at most one interval of input. Documents over 16MB don't take part in heartbeat rewrites (written only on exit). The intermediate manifest the heartbeat writes lacks the "normal shutdown" marker, which is precisely the criterion the abnormal-exit detection uses.

Privacy switches (configured in `config.toml` in the instance directory):

- `enable_snapshots = false` — turn it off entirely (clears the existing snapshot area on startup);
- `remember_session = false` — don't restore the UI and don't write a manifest on exit (clears existing sessions on startup);
- `exit_mode = "ask"` — restore the old "ask every time" confirmation-bar behavior.

These switches, together with theme / font size / auto-write-after-edit (default off: edits stay in the window and only `Ctrl+S` writes the original file; when on, an externally changed file refuses the write first and prompts; the delay seconds are adjustable), are all collected into the "Settings" ("设置") popup, and changes are written back to `config.toml` immediately — no manual editing of the file needed; the popup's tail includes a read-only hotkey quick-reference table.

## Save, Backup & Encoding

**Backup on save (Settings → Save)**: Before overwriting an existing file, a copy of the on-disk old version is made — "Overwrite `name.bak`" ("覆盖式 name.bak") writes a single backup in the same directory each time; "Timestamped history" ("时间戳历史") writes into the `name.bak.d/` directory, archived per second. Files over 64MB are automatically skipped; a failed backup only warns in the status bar and never blocks the save itself.

**Encoding & line endings (clickable in the status bar)**: The encoding menu lets you choose "Save as UTF-8 / UTF-8(BOM) / GBK / Big5 / Shift_JIS / EUC-JP / EUC-KR" — after choosing, the tab remembers that preference (used on every subsequent save, reset on reload or save-as to a new path); characters that a legacy encoding can't represent (such as emoji) are written as `&#number;` numeric entities with an explicit notice. Where transcoding is irreversible (e.g. GBK→UTF-8, removing the BOM) the status bar explains it. The line-ending menu shows the current dominant line ending and can convert to CRLF (Windows) or LF (Unix) in one click, normalizing mixed endings; conversion is an ordinary document edit — undoable, and it participates in auto-save and snapshots. An unnamed tab needs a "Save As" first to get a path before it can choose a save encoding.

**Recent-files cursor memory**: When you reopen a file from "Recent Files", the cursor and vertical scroll return automatically to where they were when that tab closed (each memory travels with its file; with "Remember Recent Files" ("记住最近文件") off the cursor records are cleared too, leaving no privacy trace). Before displaying, the list pre-checks whether the files still exist.

## Multiple Copies & Single Instance

Config and session snapshots live in `%APPDATA%\editpad\instances\<instance key>\` — the instance key is derived automatically from **the exe's location** (FNV-1a 64-bit hash), so **each copy keeps its own data**: settings, recent files, and unsaved sessions are mutually invisible (updating / reinstalling to the same location keeps the data). On first run, a legacy-layout `%APPDATA%\editpad\` is moved wholesale into the first instance directory.

The same copy runs **only one instance at a time** (the mutex name derives from the instance key; copies at different locations coexist): a second launch with a file argument writes the path into a handshake file and forwards it to the running instance, opening it in a new tab; without an argument it pops a one-time native prompt and exits.

**Window memory**: The window position and size are recorded to `config.toml` just before closing, and the next launch restores them (drag/resize is throttled and flushed live, with a fallback write on close; geometry isn't remembered while fullscreen).

## Format Support

- Common code languages are colored automatically by extension (built into syntect plus a maintained alias table: `log` / `md` / `markdown` / `toml` / `ini` / `cfg` / `conf` / `mk` / `yml`, etc.);
- Embedded mini-syntaxes: **Log** (ERROR/WARN/INFO/DEBUG levels colored + timestamps) and **TOML** (section names / keys / strings / comments), shipped with the single binary;
- Extensionless files are sniffed by content: shebang, `<?xml`, JSON/YAML heuristics, plus the conventional filenames `Dockerfile` / `Makefile`;
- Markdown files offer a "Markdown preview" ("MD 预览") panel (headings / lists / quotes / code blocks / horizontal rules / bold & italic inline styles, rendered read-only);
- JSON files support `Ctrl+Shift+F` to validate and format in one click;
- Switching between light and dark themes rebuilds the highlight colors wholesale, so no light-theme residual colors remain in dark mode.

## Technical Approach

Three things together make a large file as smooth as a 5KB one:

| Stage | Approach | Corresponding crate |
|------|------|-----------|
| Storage | Rope tree structure, O(log n) edits | `ropey` |
| Loading | Background-thread streaming read + encoding detection (UTF-8 / GBK / UTF-16) | `encoding_rs` |
| Rendering | Viewport virtualization — layout & highlight only for the visible few dozen lines | Custom-drawn component |
| Coloring | Checkpoint-style per-line lazy highlighting, supporting random viewport access | `syntect` |

Architecturally, **core and shell are separated**: `crates/core` is the pure logic layer with zero GUI dependencies (document model, encoding loading, atomic save, search, highlighting, snapshots, text tools, etc.), and `crates/app` is the iced (0.14, tiny-skia software rendering) UI shell. If a terminal version is ever wanted, core is reused as-is. `vendor/iced_tiny_skia` is the in-place-maintained render-layer patch (selection residue lines and layer-stack growth caused by partial-invalid rendering are fixed here).

## Directory Structure

```
editpad/
├── Cargo.toml                  # workspace: core + app
├── rust-toolchain.toml         # toolchain locked; rustfmt.toml is the format config
├── crates/
│   ├── core/                   # pure logic layer (zero GUI deps, testable/reusable standalone)
│   │   ├── src/                # document / search / highlight / loader / saver /
│   │   │                       # settings / snapshot / json / markdown / brackets /
│   │   │                       # toolkit / paths / syntaxes / error
│   │   ├── tests/              # edge-case input batch + random-edit differential fuzz
│   │   └── examples/           # load / find / replace / highlight / memory benchmarks
│   └── app/                    # iced UI shell
│       ├── src/
│       │   ├── main.rs         # iced entry + Message enum + module registration
│       │   ├── update.rs       # update() dispatch + domain methods (editor/file/find/tabs/…)
│       │   ├── view.rs         # main view assembly (menu bar / tab strip / command palette overlay)
│       │   ├── settings_ui.rs  # settings popup UI + neutral styling
│       │   ├── state.rs        # Editpad state struct + Default
│       │   ├── hotkeys.rs      # hotkey registry (75 actions, same data source as the command palette)
│       │   ├── load / find_scan / highlight_pave / md_preview / fonts / session /
│       │   │   tab / autosave / heartbeat / chrome / single_instance / icon.rs
│       │   ├── editor/         # custom virtualized editor
│       │   │   ├── core.rs     # EditorCore struct + geometry/layout accessors
│       │   │   ├── undo / motion / edit / block / highlight.rs   # impl split by domain
│       │   │   ├── view.rs / wrap.rs / metrics.rs / scrollbars.rs
│       │   │   └── *_tests.rs  # tests accompanying the impl files (#[path]-mounted submodules)
│       │   └── tests/          # app-layer tests split by domain (tabs/file/find/session/…)
│       └── assets/             # assets such as app.ico (embedded into the exe by build.rs)
├── vendor/iced_tiny_skia/      # in-place-maintained render-layer patch
├── dev-assets/                 # large-file acceptance sample (bench-50mb.log, gitignored)
└── package.ps1                 # release packaging (build → stage → zip → SHA256)
```

## Testing & Quality

- Every core-layer pure-logic function is covered by unit tests;
- Random edit sequences (interleaved insert / delete / replace-all / undo-redo, mixing three kinds of line endings, CJK, and emoji) are differentially compared step-by-step against a `String` reference implementation (`core/tests/edit_sequence_fuzz.rs`, reproducible with a fixed seed);
- The editor layer separately checks structural invariants under random mixed operations and differentially verifies "undo all the way back to the initial, redo all the way up to a consistent final state";
- Rendering regressions use headless pixel-level assertions (composition, wrapping, selection band, scrollbar stability, etc.);
- 50MB performance and memory use script-generated log files as regression benchmarks (`core/examples/*_bench.rs`);
- Zero clippy warnings is the discipline line (`workspace.lints`).

## Development History

| Milestone | Content | Status |
|--------|------|------|
| M0 | Skeleton + open / edit / save / dirty marker / atomic save | ✅ Done |
| M1 | Everyday usability: background streaming load + progress, find & replace, go to line, settings & recent files, hotkeys | ✅ Done |
| M2a | Large-file push: iced 0.14 (IME works) + custom virtualized editor + rope data source + undo/redo | ✅ Done |
| M2b | syntect per-line lazy highlighting, IME pre-edit inline, CJK column mapping | ✅ Done |
| M3 | Close confirmation, drag-open, dark theme + font-size settings, dirty replace protection, open/save-as deadlock fixes | ✅ Done |
| M4 | Multi-tab, instant save, JSON validate & format, Log/TOML coloring + extensionless sniffing + Markdown preview | ✅ Done |
| M5 | Session snapshots: close a dirty window with no prompt + restore the last interface on startup, recoverable after an abnormal exit | ✅ Done |

After M5, iteration continues from a candidate pool and user requests: soft wrap v2 (word-boundary wrapping), regex & whole-word search, bookmarks, column-block editing v2, menu-bar/status-bar refactor, multi-instance isolation and single-instance mutex, command-line open, word navigation and smart indent, extended line operations, overwrite mode, read-only lock, text tools, encoding expansion, command palette, file-watch tail follow — plus a round of structural refactoring (update/view/core domain splits, clippy zeroed) and repeated root-cause fixes for IME / wrapping / rendering.

## License

Licensed under `Apache-2.0` (see `LICENSE-APACHE`), consistent with Rust ecosystem convention.
