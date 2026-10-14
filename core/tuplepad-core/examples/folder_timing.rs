//! Times a folder search: `cargo run --release --example folder_timing -- <folder> <pattern> [everything]`
use std::time::Instant;
use tuplepad_core::{FolderSearch, FolderSearchOptions, SearchQuery};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let options = FolderSearchOptions {
        folder: args[1].clone(),
        query: SearchQuery { pattern: args[2].clone(), case_sensitive: false, whole_word: false, regex: std::env::var("REGEX").is_ok() },
        include: String::new(),
        exclude: String::new(),
        search_everything: args.get(3).is_some(),
    };
    let start = Instant::now();
    let search = FolderSearch::start(options).unwrap();
    let mut files = 0;
    loop {
        files += search.take_results().len();
        let progress = search.progress();
        if progress.finished {
            println!("{:?} in {:.3}s; {files} files with matches", progress, start.elapsed().as_secs_f64());
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
