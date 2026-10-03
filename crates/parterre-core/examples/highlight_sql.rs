//! PROTOTYPE (#208): times the SQL highlighter on a file.
//!
//! `cargo run --release -p parterre-core --features syntax --example highlight_sql -- FILE`

use parterre_core::highlight::{Language, highlight};
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).expect("a file to highlight");
    let text = std::fs::read_to_string(&path).expect("readable");
    let lines = text.lines().count();
    // The first run compiles the query; the rest show the steady state.
    for run in 0..4 {
        let t = Instant::now();
        let spans = highlight(Language::Sql, &text);
        let total: usize = spans.iter().map(Vec::len).sum();
        println!(
            "run {run}: {lines} lines, {total} spans, {:.1} ms",
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
}
