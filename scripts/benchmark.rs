//! Bounded backend/layout measurement. Not a renderer/FPS benchmark.
#[path = "../src/git.rs"] mod git;
#[path = "../src/graph.rs"] mod graph;
#[path = "../src/platform.rs"] mod platform;
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "fixtures/large".into());
    let repo = git::Git::open(&path).expect("open benchmark repository");
    let total = repo.run(&["rev-list", "--all", "--count"]).unwrap();
    let branch_total = repo.run(&["for-each-ref", "--format=%(refname)", "refs/heads"]).unwrap().lines().count();
    println!("{{");
    println!("  \"repository\": {:?},", repo.path.to_string_lossy());
    println!("  \"total_commits\": {}, \"local_branches\": {},", total.trim(), branch_total);
    println!("  \"conditions\": \"Windows native release-optimized std-only harness; Git subprocess snapshot; one priming run then three measured runs per limit. OS filesystem cache was not flushed; no cold-cache claim. Linear history with offset refs; no merges, large diffs, remote traffic, UI frames, or FPS measured.\",");
    println!("  \"cases\": [");
    for (case, limit) in [500, 5000, 10000].into_iter().enumerate() {
        let priming = repo.snapshot(limit).expect("warm-up snapshot");
        let seed = (!priming.head_id.is_empty()).then_some(priming.head_id.as_str());
        let _ = graph::layout_with_head(&priming.commits, seed);
        println!("    {{\"limit\": {limit}, \"priming_commits\": {}, \"samples\": [", priming.commits.len());
        for sample in 0..3 {
            let start = Instant::now();
            let state = repo.snapshot(limit).expect("measured snapshot");
            let snapshot_ms = start.elapsed().as_secs_f64()*1000.0;
            let start = Instant::now();
            let seed = (!state.head_id.is_empty()).then_some(state.head_id.as_str());
            let layout = graph::layout_with_head(&state.commits,seed);
            let graph_ms = start.elapsed().as_secs_f64()*1000.0;
            // Consume observable output to retain the complete layout work.
            let edges: usize = layout.rows.iter().map(|row|row.edges.len()).sum();
            println!("      {{\"loaded_commits\": {}, \"changed_files\": {}, \"lanes\": {}, \"edge_segments\": {}, \"snapshot_ms\": {:.4}, \"graph_ms\": {:.4}}}{}", state.commits.len(),state.status.len(),layout.lanes,edges,snapshot_ms,graph_ms,if sample<2{","}else{""});
        }
        println!("    ]}}{}",if case<2{","}else{""});
    }
    println!("  ]\n}}");
}
