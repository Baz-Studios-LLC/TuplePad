# TuplePad

A fast, beautiful text editor for Mac, Windows and Linux. Free and open source (MIT).

TuplePad is in early development. It's being rebuilt on [Slint](https://slint.dev), a cross-platform UI toolkit, so one app runs on all three systems. The first version, a native Mac app in SwiftUI, stays in the repository as a reference until the new one matches it.

## What works today

- **Large files.** A 500 MB, 3.8-million-line log opens in about a quarter of a second and scrolls at your display's full frame rate. Typing shows on screen within about 8 ms. Minified files with multi-megabyte lines draw only the part on screen.
- **Nothing is lost when you quit.** Open documents, unsaved changes and untitled notes come back on the next launch, with the caret, scroll position and word wrap where you left them.
- **Syntax highlighting** for 25 languages, using the tree-sitter parser. Markdown shows emphasis, bold, code, links and headings, with fenced code blocks colored in their own language.
- **Multiple carets:** ⌃⇧↑ / ⌃⇧↓ add one above or below (Ctrl+Alt+↑/↓ on Windows and Linux), ⌘-click (Ctrl-click) adds one anywhere, ⌥-drag (Alt-drag) selects a column, and ⌘D (Ctrl+D) selects the next occurrence.
- **Code-editor basics:** brackets and quotes close automatically, Return keeps the indentation (one level deeper after an opening bracket, with the closer moved to its own line), Tab and Shift-Tab indent, Backspace steps back through indentation, smart Home, and Copy with nothing selected copies the whole line. Pasting as many lines as there are carets gives each caret one.
- **Helpful highlighting:** other occurrences of the word at the caret, the matching bracket pair, optional invisibles (spaces, tabs, line breaks) and indent guides.
- **Word wrap** (⌥Z) per document. Wrapped lines keep their indentation, and carets, selection and ↑/↓ work on the wrapped rows.
- **Input methods** for Japanese, Chinese and other languages, with the text you're composing shown in place.
- **A Mail-style sidebar** of open documents, showing each one's location, a preview line and a dot for unsaved changes. Drag its edge to resize it, or hide it (⌃⌘S).
- **Themes:** TuplePad (neutral, with midnight blue), Midnight (ivory and navy), Graphite, Sepia and High Contrast, each in light and dark, following the system or pinned. Zoom with ⌘+ / ⌘− / ⌘0.
- **Settings** (⌘,): font, size and line height with a live preview; indentation; typing helpers; display options; the encoding and line endings for new documents; appearance and theme. Changes apply as you make them.
- **Command palette** (⇧⌘P): fuzzy search over every command, open document, theme and recent file, with keyboard shortcuts shown. Type a few letters (`wwr` finds Word Wrap) and press Return.
- **Files:** New, Open, Open Recent, Save, Save As, Save All (⌥⌘S), Revert to Saved, Rename or Move, Show in Finder, and Close, Close Others and Close All, asking before closing unsaved changes. Drop files on the window to open them, and drag selected text to move it (hold ⌥ to copy).
- **Keeps up with your files.** When another app changes a file, TuplePad reloads it (Undo brings your text back), or shows a quiet banner offering Reload or Keep Mine if you have unsaved changes. Moved and deleted files get a banner too. **Follow End of File** (View menu) tails a growing log.
- **Encodings and line endings:** detected and kept. Reopen with another encoding, convert to one (with or without a BOM; TuplePad warns about characters it can't hold), and switch line endings, from the File menu.
- **The `tuplepad` terminal command** (File ▸ Install Command Line Tool…): `tuplepad notes.txt`, `tuplepad +42 main.rs` or `main.rs:42:7`, and `git log | tuplepad -`. Files that don't exist yet open as new documents. TuplePad opens from Finder and Open With.
- **Print** (⌘P) with syntax colors and line numbers (each optional), through the Mac's print panel (with its preview and Save as PDF).

The Mac app in `TuplePad/` does much more, and those features are coming to the new app milestone by milestone (see the roadmap below):

<details>
<summary>What the Mac app does</summary>

- **Large files.** A 500 MB, 3.8-million-line log opens in about a quarter of a second, and typing anywhere in it stays instant.
- **Nothing is lost when you quit.** Open documents, unsaved changes and untitled scratch notes all come back on the next launch, with the caret where you left it. TuplePad never asks to save on quit.
- **Syntax highlighting** for 25 languages, using the tree-sitter parser. It updates as you type, even in multi-megabyte files. Supported: C, C++, C#, CSS, Diff, Go, HTML, Java, JavaScript/JSX, JSON, Lua, Makefile, Markdown, PHP, Python, Ruby, Rust, Shell, SQL, Swift, TOML, TypeScript/TSX, XML and YAML. The language is detected from the file name or a `#!` line, and you can change it from the status bar.
- **Find and replace** (⌘F, ⌥⌘F): highlights every match as you type, with Match Case, Whole Words and Regular Expression options. Replace All with `$1`-style groups counts as one undo step. ⌘G / ⇧⌘G go to the next or previous match, and ⌘E searches for the selection. **Count Matches** gives an exact total without selecting anything, and **Mark All** (⌃⌘M) keeps every match highlighted until you clear it.
- **Find in Files** (⇧⌘F) and **Find in Open Documents:** a results panel under the editor, grouped by file with each match highlighted; click one to jump there. Folder searches respect `.gitignore`, skip binary and very large files, take include and exclude patterns (`*.swift, *.rs`, `build`), run in the background with progress and a Stop button, and get through tens of thousands of files in about a second.
- **Replace in Files** (⌥⇧⌘F): preview every change, file by file, before anything is touched, and leave out any file. Each file can be undone afterward, from the panel. Files keep their encoding, line endings and permissions.
- **Bookmarks:** ⌘F2 bookmarks a line (or click left of the line numbers), F2 and ⇧F2 jump between them, and bookmarks show as tags in the gutter. Bookmark every line that matches a search, then copy or delete the bookmarked lines. Bookmarks follow their lines as you edit and come back after a relaunch.
- **Text encodings and line endings.** TuplePad detects and keeps each file's encoding (UTF-8, UTF-16, Windows and Mac legacy encodings, Japanese, Chinese, Korean) and line endings. Convert line endings from the status bar.
- **Multiple cursors:** ⌃⇧↑ / ⌃⇧↓ add a cursor above or below, ⌘-click adds one anywhere, and ⌥-drag selects a rectangular block.
- **Word wrap** (⌥⌘\\), per document. Wrapped lines keep their indentation, and carets, selection and find all work on the wrapped rows. It's off for files with more than 400,000 lines, to keep them fast.
- **The Text menu:** Go to Line (⌘L), Toggle Comment (⌘/), move lines (⌥⌘↑/↓), duplicate (⇧⌘D) and delete (⇧⌘K) lines. Also sort, reverse, remove duplicate or blank lines, join and split, change case (UPPER, lower, Title, Sentence, swap), trim whitespace, and convert tabs and indentation. Each is one undo step.
- **Keeps up with your files.** When another app changes a file, TuplePad reloads it (or asks, if you have unsaved changes; a reload can be undone). It notices files that were moved or deleted. **Follow End of File** (View menu) tails a growing log.
- **File menu:** Open Recent, Save All (⌥⌘S), Revert to Saved, Rename and Move (also from the title bar), Close Others (⌥⌘W) and Close All, Show in Finder, Print with syntax colors and line numbers, Reopen with Encoding and Convert to Encoding. Drag files onto the window to open them, or drag text in and out of the editor.
- **The `tuplepad` terminal command** (TuplePad ▸ Install Command Line Tool…): `tuplepad notes.txt`, `tuplepad +42 main.rs` or `main.rs:42:7`, and `git log | tuplepad -`. Files that don't exist yet open as new documents.
- **Helpful highlighting:** other occurrences of the selected word, the matching bracket pair, optional invisibles (spaces, tabs, line breaks) and indent guides. Brackets and quotes close automatically. Zoom with ⌘+ / ⌘− / ⌘0.
- **Code-editor basics:** auto-indent (one level deeper after an opening bracket, with the closer moved to its own line), Tab and Shift-Tab indenting, backspacing through indentation, smart Home (⌘←), copying the whole line when nothing is selected, and undo that groups typing into sensible steps.
- **Full Mac text input:** every standard keyboard shortcut, input methods for Japanese, Chinese and other languages, the accent menu (press and hold a letter), and dictation.
- **A Mail-style sidebar** of open documents, showing each one's location, a preview and a dot for unsaved changes.
- **Power editing:**
  - The **column editor** (⌥⌘C) inserts text or numbers (start, step, repeat, padding, decimal/hex/octal/binary) down a column or at every caret.
  - **Code folding** from the syntax tree (by indentation for Python, YAML and plain text): click the chevrons by the line numbers, or use ⌥⌘← / ⌥⌘→ and Fold All / Unfold All.
  - **Split view** (⌘\\ side by side, ⌃⌘\\ top and bottom), with the same document or two different ones.
  - A **minimap** (View ▸ Show Minimap) to see and scroll the whole document.
  - An **outline** (⌃⌘O) of functions, types and headings, in an inspector on the right; click to jump.
  - **Word completion** from every open document as you type, accepted with Tab or Return.
- **Markdown highlighting** of emphasis, bold, code, links and headings, with fenced code blocks colored in their own language. HTML colors the JavaScript and CSS inside it.
- **Command palette** (⇧⌘P): fuzzy-search every menu command, open document and recent file, with keyboard shortcuts shown. Type a few letters (`dl` finds Delete Line) and press Return.
- **Themes:** TuplePad, Midnight (ivory on midnight blue), Graphite, Sepia and High Contrast. Each has a light and a dark version that follows your Mac, or pin TuplePad to Light or Dark in Settings.
- **Settings** (⌘,): editor font, size and line height; tab width, tabs or spaces, and indentation detection; wrap, invisibles, indent guides and current-line highlight; and the encoding and line endings for new documents. The status bar also sets each document's indentation.

</details>

## Running it

Double-click **`Launch TuplePad.command`**. It builds TuplePad (into `build/TuplePad.app`) and opens it.

To work on it: `cargo run -p tuplepad-app --manifest-path core/Cargo.toml -- [files…]`. You need Rust (install it from [rustup.rs](https://rustup.rs)) and, on a Mac, Xcode's command-line tools. Windows and Linux builds are next on the list to test.

The Mac app opens from `TuplePad.xcodeproj` in Xcode (macOS 26 or later, Xcode 27).

## How it's built

| Part | Language | Where |
|---|---|---|
| Text engine: buffer (a rope), editing, undo, encodings, saving, syntax trees, search, session | Rust | `core/tuplepad-core` |
| The app: window, editor, sidebar, menus | Slint and Rust | `core/tuplepad-app` (`ui/` and `src/`) |
| `tuplepad` terminal command | Rust | `core/tuplepad-cli` |
| The first Mac app (reference) | Swift / SwiftUI | `TuplePad/` |
| App icon | Icon Composer | `TuplePad/AppIcon.icon` (artwork source in `design/`) |

The editor draws only the lines on screen, so a document's size doesn't affect scrolling or typing speed.

## Roadmap

See [docs/GOAL.md](docs/GOAL.md) for the plan: files, settings and themes, search, power editing and the built-in tools (compare, JSON and XML, hashes, Markdown preview and more) come next. TuplePad will also include a built-in MCP server, so AI tools can read and edit your open documents while you watch.
