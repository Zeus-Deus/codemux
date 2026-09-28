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
        json!([3, "a", "label", "invalid mode", "property"]),
        json!([3, "a", "label", "invalid mode", null]),
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

#[test]
fn escaped_content_updates_cannot_exceed_the_intermediate_tree_limit() {
    let mut children = (0..7)
        .map(|i| json!({"id":format!("large-{i}"),"type":3,"data":"x".repeat(32768)}))
        .collect::<Vec<_>>();
    children.push(json!({"id":"target","type":3,"data":"original"}));
    let mut tree = Tree::default();
    tree.apply(&[
        json!([0,"~",{"id":"root","type":1,"element":"cmx-stack","children":children},0]),
    ])
    .unwrap();
    for update in [
        vec![
            json!([2, "target", "\n".repeat(32768)]),
            json!([2, "target", "small again"]),
        ],
        vec![
            json!([3, "root", "value", "\n".repeat(32768)]),
            json!([3, "root", "value", null]),
        ],
    ] {
        assert!(tree.apply(&update).is_err());
        assert_eq!(
            tree.children[0].children[7].data.as_deref(),
            Some("original")
        );
        assert!(tree.children[0].properties.is_empty());
    }
}

#[test]
fn malformed_and_duplicate_ids_and_disallowed_events_are_rejected() {
    let mut tree = Tree::default();
    tree.apply(&[json!([0,"~",{"id":"root","type":1,"element":"cmx-stack"},0])])
        .unwrap();
    let long = "a".repeat(65);
    for id in ["", long.as_str(), "a/b", "a_b", "a b", "é"] {
        assert!(
            tree.apply(&[json!([0,"root",{"id":id,"type":3,"data":"x"},0])])
                .is_err(),
            "{id:?}"
        );
    }
    tree.apply(&[json!([0,"root",{"id":"a".repeat(64),"type":3,"data":"x"},0])])
        .unwrap();
    for record in [
        // Duplicate node IDs inside one inserted subtree, and across the tree.
        json!([0,"root",{"id":"card","type":1,"element":"cmx-card","children":[{"id":"twin","type":3,"data":"x"},{"id":"twin","type":3,"data":"y"}]},0]),
        json!([0,"root",{"id":"card","type":1,"element":"cmx-card","children":[{"id":"root","type":3,"data":"x"}]},0]),
        // A callback ID is unique within its view.
        json!([0,"root",{"id":"pair","type":1,"element":"cmx-stack","children":[
            {"id":"first","type":1,"element":"cmx-button","eventListeners":{"press":{"callbackId":"same"}}},
            {"id":"second","type":1,"element":"cmx-button","eventListeners":{"press":{"callbackId":"same"}}}
        ]},0]),
        json!([0,"root",{"id":"bad","type":1,"element":"cmx-button","eventListeners":{"press":{"callbackId":"a_b"}}},0]),
        json!([0,"root",{"id":"extra","type":1,"element":"cmx-button","eventListeners":{"press":{"callbackId":"a","capture":true}}},0]),
        // Events belong only to tags that can raise them.
        json!([0,"root",{"id":"label","type":1,"element":"cmx-text","eventListeners":{"press":{"callbackId":"a"}}},0]),
        json!([0,"root",{"id":"field","type":1,"element":"cmx-text-field","eventListeners":{"press":{"callbackId":"a"}}},0]),
        json!([0,"root",{"id":"hover","type":1,"element":"cmx-button","eventListeners":{"hover":{"callbackId":"a"}}},0]),
    ] {
        assert!(tree.apply(std::slice::from_ref(&record)).is_err(), "{record}");
    }
    assert_eq!(tree.children[0].children.len(), 1);
    assert_eq!(tree.callback_count(), 0);
}

#[test]
fn plugin_callback_budget_rejects_the_whole_batch() {
    let buttons = (0..10)
        .map(|i| json!({"id":format!("b{i}"),"type":1,"element":"cmx-button","eventListeners":{"press":{"callbackId":format!("c{i}")}}}))
        .collect::<Vec<_>>();
    let mut tree = Tree::default();
    tree.apply_within(
        &[json!([0,"~",{"id":"root","type":1,"element":"cmx-stack","children":buttons},0])],
        10,
    )
    .unwrap();
    assert_eq!(tree.callback_count(), 10);
    let error = tree
        .apply_within(
            &[
                json!([3, "b0", "label", "not committed"]),
                json!([0,"root",{"id":"more","type":1,"element":"cmx-button","eventListeners":{"press":{"callbackId":"extra"}}},0]),
            ],
            10,
        )
        .unwrap_err();
    assert_eq!(
        error.data.code,
        codemux_addon_protocol::ErrorCode::ResourceLimit
    );
    assert_eq!(tree.children[0].children.len(), 10);
    assert!(tree.children[0].children[0].properties.is_empty());
    // Replacing a listener keeps the count; removing a node frees budget.
    tree.apply_within(&[json!([3,"b0","press",{"callbackId":"new"},3])], 10)
        .unwrap();
    tree.apply_within(&[json!([1, "root", 0])], 9).unwrap();
    assert_eq!(tree.callback_count(), 9);
}
