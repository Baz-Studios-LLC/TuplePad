//! Times opening a file and reading/editing in the middle of it:
//! `cargo run --release --example open_timing -- path/to/big.log`
use std::time::Instant;
use tuplepad_core::{EditKind, TextBuffer, TextPos, TextSelection};

fn main() {
    let path = std::env::args().nth(1).expect("usage: open_timing <file>");
    let start = Instant::now();
    let buffer = TextBuffer::open(path).expect("open");
    let lines = buffer.line_count();
    println!("open: {:?} for {lines} lines (language: {:?})", start.elapsed(), buffer.language());

    let start = Instant::now();
    let visible = buffer.lines(lines / 2, 60);
    let spans = buffer.highlights(lines / 2, 60);
    println!("60 lines + highlights from the middle: {:?} ({} chars, {} spans)", start.elapsed(),
             visible.iter().map(|l| l.len()).sum::<usize>(), spans.len());

    let start = Instant::now();
    let caret = TextSelection::caret(TextPos { line: lines / 2, column: 0 });
    let mut selections = vec![caret];
    for c in "hello".chars() {
        selections = buffer.replace(selections, vec![c.to_string()], EditKind::Typing);
        buffer.highlights(lines / 2, 60);
    }
    println!("5 keystrokes mid-file, re-highlighting after each: {:?}", start.elapsed() / 5);

    let start = Instant::now();
    buffer.undo();
    println!("undo: {:?}", start.elapsed());
}
