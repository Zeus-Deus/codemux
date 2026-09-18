use codemux_addon_protocol::ui::Tree;
use serde_json::json;
use std::time::Instant;
fn main() {
    let children = (0..1998)
        .map(|i| json!({"id":format!("text-{i}"),"type":3,"data":"x".repeat(8)}))
        .collect::<Vec<_>>();
    let mut tree = Tree::default();
    tree.apply(&[
        json!([0,"~",{"id":"root","type":1,"element":"cmx-stack","children":children},0]),
    ])
    .unwrap();
    let changes = (0..1000)
        .map(|i| json!([2, format!("text-{i}"), "changed"]))
        .collect::<Vec<_>>();
    let mut samples = Vec::new();
    for _ in 0..20 {
        let mut candidate = tree.clone();
        let start = Instant::now();
        candidate.apply(&changes).unwrap();
        samples.push(start.elapsed().as_secs_f64() * 1000.);
    }
    println!(
        "{}",
        json!({"nodes":1999,"mutations":1000,"treeBytes":serde_json::to_vec(&tree).unwrap().len(),"samplesMs":samples})
    );
}
