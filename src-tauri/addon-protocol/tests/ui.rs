use codemux_addon_protocol::ui::Tree;
use serde_json::json;
#[test]
fn invalid_batch_never_partially_commits() {
    let mut tree = Tree::default();
    let initial = json!([0,"~",{"id":"a","type":1,"element":"cmx-text","children":[{"id":"b","type":3,"data":"original"}]},0]);
    tree.apply(&[initial]).unwrap();
    assert!(tree
        .apply(&[
            json!([2, "b", "partial"]),
            json!([3,"a","dangerouslySetInnerHTML",{"__html":"<script/>"}])
        ])
        .is_err());
    assert_eq!(
        tree.children[0].children[0].data.as_deref(),
        Some("original")
    );
    for record in [
        json!([3, "a", "style", "background:url(https://attacker)"]),
        json!([0,"a",{"id":"a","type":1,"element":"cmx-card"},0]),
        json!([0,"~",{"id":"c","type":1,"element":"script"},0]),
        json!([3, "a", "href", "https://attacker", 2]),
        json!([3, "a", "dangerouslySetInnerHTML", null, 1]),
        json!([0,"~",{"id":"z","type":1,"element":"cmx-text","properties":{"style":null}},0]),
    ] {
        assert!(tree.apply(&[record]).is_err());
    }
}
#[test]
fn callback_authority_expires_on_replacement_and_unmount() {
    let mut tree = Tree::default();
    tree.apply(&[json!([0,"~",{"id":"a","type":1,"element":"cmx-button","eventListeners":{"press":{"callbackId":"old"}},"children":[]},0])]).unwrap();
    assert!(tree.callback("a", "press", "old"));
    assert!(!tree.callback("other", "press", "old"));
    tree.apply(&[json!([3,"a","press",{"callbackId":"new"},3])])
        .unwrap();
    assert!(!tree.callback("a", "press", "old"));
    assert!(tree.callback("a", "press", "new"));
    tree.apply(&[json!([1, "~", 0])]).unwrap();
    assert!(!tree.callback("a", "press", "new"));
}
#[test]
fn nested_and_oversized_trees_fail_before_render() {
    let mut node = json!({"id":"0","type":3,"data":"leaf"});
    for i in 1..34 {
        node = json!({"id":i.to_string(),"type":1,"element":"cmx-card","children":[node]});
    }
    assert!(Tree::default().apply(&[json!([0, "~", node, 0])]).is_err());
    let children: Vec<_> = (0..2001)
        .map(|i| json!({"id":i.to_string(),"type":3,"data":"x"}))
        .collect();
    assert!(Tree::default()
        .apply(&[json!([0,"~",{"id":"root","type":1,"element":"cmx-stack","children":children},0])])
        .is_err());
}

#[test]
fn mutation_and_serialized_tree_limits_reject_atomically() {
    let mut tree = Tree::default();
    tree.apply(&[json!([0,"~",{"id":"text","type":3,"data":"original"},0])])
        .unwrap();
    let flood = vec![json!([2, "text", "changed"]); 1001];
    assert!(tree.apply(&flood).is_err());
    assert_eq!(tree.children[0].data.as_deref(), Some("original"));
    let large = (0..9)
        .map(|i| json!({"id":format!("child-{i}"),"type":3,"data":"x".repeat(32768)}))
        .collect::<Vec<_>>();
    assert!(tree
        .apply(&[json!([0,"~",{"id":"large","type":1,"element":"cmx-stack","children":large},1])])
        .is_err());
    assert_eq!(tree.children.len(), 1);
    assert_eq!(tree.children[0].data.as_deref(), Some("original"));
}
