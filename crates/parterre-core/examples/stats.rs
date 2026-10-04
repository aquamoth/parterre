//! Loads a repository and reports graph sizes and timings for every simplification mode.
//!
//! Usage: `cargo run --release -p parterre-core --example stats -- <repo> [--dump-nodes <mode>]`

use std::time::Instant;

use parterre_core::layout::{self, LayoutEdge, LayoutInput, LayoutOptions, Point, Ranking};
use parterre_core::revgraph::{self, GraphOptions, Simplification};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).map(String::as_str).unwrap_or(".");
    let t = Instant::now();
    let repo = parterre_core::git::load_repo(path.as_ref()).expect("load");
    eprintln!(
        "loaded {} commits, {} refs in {:?}",
        repo.commits.len(),
        repo.refs.len(),
        t.elapsed()
    );
    eprintln!("git's hash length: {}", repo.abbrev_len);
    if let Some(head) = repo.head_commit() {
        let t = Instant::now();
        let log = parterre_core::log::LogQuery::commit(head).run(&repo);
        eprintln!("log of HEAD: {} commits in {:?}", log.len(), t.elapsed());
    }

    if let Some(i) = args.iter().position(|a| a == "--dump-nodes") {
        let mode = match args.get(i + 1).map(String::as_str) {
            Some("branches") => Simplification::BranchesAndMerges,
            Some("all") => Simplification::AllCommits,
            _ => Simplification::Decorated,
        };
        let g = revgraph::build(
            &repo,
            &GraphOptions {
                simplification: mode,
                ..Default::default()
            },
        );
        for n in &g.nodes {
            println!("{}", repo.commit(n.commit).oid);
        }
        return;
    }

    for mode in Simplification::ALL {
        let t = Instant::now();
        let g = revgraph::build(
            &repo,
            &GraphOptions {
                simplification: mode,
                ..Default::default()
            },
        );
        let build = t.elapsed();
        let input = LayoutInput {
            sizes: g
                .nodes
                .iter()
                // Roughly the app's boxes: 12 px monospace (7.2 px per char), 20 px margins.
                .map(|n| {
                    let chars = n
                        .refs
                        .iter()
                        .map(|&r| repo.refs[r].name.chars().count())
                        .max()
                        .unwrap_or(0)
                        .max(8);
                    Point::new(40.0 + 7.2 * chars as f32, 24.0 * n.refs.len().max(1) as f32)
                })
                .collect(),
            times: g
                .nodes
                .iter()
                .map(|n| repo.commit(n.commit).commit_time)
                .collect(),
            edges: g
                .edges
                .iter()
                .map(|e| LayoutEdge {
                    child: e.child,
                    parent: e.parent,
                    first_parent: e.first_parent,
                })
                .collect(),
            priority: g.layout_anchor(&repo).into_iter().collect(),
        };
        for ranking in Ranking::ALL {
            let t = Instant::now();
            let l = layout::layout(
                &input,
                &LayoutOptions {
                    ranking,
                    concentrate_edges: args.iter().any(|a| a == "--bundle"),
                    // PROTOTYPE (centred trunk).
                    trunk: std::env::var("PARTERRE_TRUNK")
                        .ok()
                        .and_then(|v| layout::Trunk::from_letter(&v))
                        .unwrap_or_default(),
                    ..Default::default()
                },
            );
            let layers = l.layers.iter().max().map_or(0, |m| m + 1);
            let bends: usize = l.edges.iter().map(|e| e.len() - 2).sum();
            println!(
                "{:<22} nodes {:>6} edges {:>6} build {:>9.2?} | {:<20} layers {:>6} dummies {:>7} crossings {:>6} size {:>7.0}x{:<7.0} layout {:>9.2?}",
                mode.label(),
                g.nodes.len(),
                g.edges.len(),
                build,
                ranking.label(),
                layers,
                bends,
                l.crossings,
                l.max.x - l.min.x,
                l.max.y - l.min.y,
                t.elapsed()
            );
        }
    }
}
