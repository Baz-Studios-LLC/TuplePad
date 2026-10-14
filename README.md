# TuplePad

A fast, beautiful text editor for the Mac, with the feel of a native Mac app. Free and open source (MIT).

TuplePad is in early development.

## What works today

- **Large files.** A 500 MB, 3.8-million-line log opens in about a quarter of a second, and typing anywhere in it stays instant.
- **Nothing is lost when you quit.** Open documents, unsaved changes and untitled scratch notes all come back on the next launch, with the caret where you left it. TuplePad never asks to save on quit.
- **Syntax highlighting** for 25 languages, using the tree-sitter parser. It updates as you type, even in multi-megabyte files. Supported: C, C++, C#, CSS, Diff, Go, HTML, Java, JavaScript/JSX, JSON, Lua, Makefile, Markdown, PHP, Python, Ruby, Rust, Shell, SQL, Swift, TOML, TypeScript/TSX, XML and YAML. The language is detected from the file name or a `#!` line, and you can change it from the status bar.
- **Find and replace** (⌘F, ⌥⌘F): highlights every match as you type, with Match Case, Whole Words and Regular Expression options. Replace All with `$1`-style groups counts as one undo step. ⌘G / ⇧⌘G go to the next or previous match, and ⌘E searches for the selection.
- **Text encodings and line endings.** TuplePad detects and keeps each file's encoding (UTF-8, UTF-16, Windows and Mac legacy encodings, Japanese, Chinese, Korean) and line endings. Convert line endings from the status bar.
- **Multiple cursors:** ⌃⇧↑ / ⌃⇧↓ add a cursor above or below, ⌘-click adds one anywhere, and ⌥-drag selects a rectangular block.
- **Word wrap** (⌥⌘\\), per document. Wrapped lines keep their indentation, and carets, selection and find all work on the wrapped rows. It's off for files with more than 400,000 lines, to keep them fast.
- **The Text menu:** Go to Line (⌘L), Toggle Comment (⌘/), move lines (⌥⌘↑/↓), duplicate (⇧⌘D) and delete (⇧⌘K) lines. Also sort, reverse, remove duplicate or blank lines, join and split, change case (UPPER, lower, Title, Sentence, swap), trim whitespace, and convert tabs and indentation. Each is one undo step.
- **Helpful highlighting:** other occurrences of the selected word, the matching bracket pair, optional invisibles (spaces, tabs, line breaks) and indent guides. Brackets and quotes close automatically. Zoom with ⌘+ / ⌘− / ⌘0.
- **Code-editor basics:** auto-indent, Tab and Shift-Tab indenting, backspacing through indentation, smart Home (⌘←), copying the whole line when nothing is selected, and undo that groups typing into sensible steps.
- **Full Mac text input:** every standard keyboard shortcut, input methods for Japanese, Chinese and other languages, the accent menu (press and hold a letter), and dictation.
- **A Mail-style sidebar** of open documents, showing each one's location, a preview and a dot for unsaved changes.

## Running it

Double-click **`Launch TuplePad.command`**. It builds the app and opens it.

To work on it, open `TuplePad.xcodeproj` in Xcode and press ⌘R.

You need macOS 26 or later, Xcode 27, and Rust (install it from [rustup.rs](https://rustup.rs)).

## How it's built

| Part | Language | Where |
|---|---|---|
| Text engine: buffer (a rope), editing, undo, encodings, saving, syntax trees, search | Rust | `core/tuplepad-core` |
| Bridge between Rust and Swift (generated) | UniFFI | `scripts/build-core.sh` → `Generated/` |
| Editor surface: drawing, input, carets | Swift / AppKit | `TuplePad/Editor` |
| Window, sidebar, status bar, menus | SwiftUI | `TuplePad/Views`, `TuplePad/App` |
| App icon | Icon Composer | `TuplePad/AppIcon.icon` (artwork source in `design/`) |

The editor view draws only the lines on screen, so a document's size doesn't affect scrolling or typing speed.

## Roadmap

See [docs/GOAL.md](docs/GOAL.md) for the full plan. Next up: reloading files changed on disk, follow mode for logs, drag and drop, the rest of the File menu, an Encoding menu and a `tuplepad` terminal command. After that come the tools that usually come as plugins: compare, JSON and XML tools, encoding and hash tools, Markdown preview, and more. TuplePad will also include a built-in MCP server, so AI tools can read and edit your open documents while you watch.
