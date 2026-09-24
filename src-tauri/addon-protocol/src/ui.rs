//! Validate the entire Remote DOM batch before committing a normalized tree.
use crate::{limits, ErrorCode, ProtocolError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, ProtocolError>;
fn invalid() -> ProtocolError {
    ProtocolError::invalid("Invalid plugin UI")
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(default)]
    pub properties: BTreeMap<String, Value>,
    #[serde(default)]
    pub attributes: BTreeMap<String, Value>,
    #[serde(default)]
    pub event_listeners: BTreeMap<String, Value>,
    #[serde(default)]
    pub children: Vec<Node>,
}
#[derive(Debug, Clone, Serialize, Default)]
pub struct Tree {
    pub children: Vec<Node>,
}
pub const TAGS: &[&str] = &[
    "cmx-stack",
    "cmx-grid",
    "cmx-card",
    "cmx-text",
    "cmx-heading",
    "cmx-markdown",
    "cmx-button",
    "cmx-text-field",
    "cmx-text-area",
    "cmx-select",
    "cmx-checkbox",
    "cmx-switch",
    "cmx-tabs",
    "cmx-list",
    "cmx-table",
    "cmx-badge",
    "cmx-progress",
    "cmx-icon",
    "cmx-divider",
    "cmx-empty-state",
];
fn identifier(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
fn token(v: &Value, tokens: &[&str]) -> bool {
    v.as_str().is_some_and(|s| tokens.contains(&s))
}
fn string(v: &Value, max: usize) -> bool {
    v.as_str().is_some_and(|s| s.len() <= max)
}
fn property(key: &str, v: &Value) -> bool {
    if v.is_null() {
        return matches!(
            key,
            "spacing"
                | "direction"
                | "columns"
                | "align"
                | "width"
                | "height"
                | "size"
                | "color"
                | "label"
                | "title"
                | "placeholder"
                | "name"
                | "value"
                | "disabled"
                | "checked"
                | "level"
                | "max"
                | "headers"
                | "items"
                | "rows"
                | "options"
        );
    }
    match key {
        "spacing" => token(v, &["none", "xs", "sm", "md", "lg"]),
        "direction" => token(v, &["horizontal", "vertical"]),
        "columns" => v.as_u64().is_some_and(|n| (1..=12).contains(&n)),
        "align" => token(v, &["start", "center", "end", "stretch"]),
        "width" | "height" => token(v, &["auto", "full"]),
        "size" => token(v, &["xs", "sm", "md", "lg"]),
        "color" => token(
            v,
            &["default", "muted", "success", "warning", "danger", "accent"],
        ),
        "label" | "title" | "placeholder" => string(v, 4096),
        "name" => v
            .as_str()
            .is_some_and(|s| crate::manifest::ICONS.contains(&s)),
        "value" => {
            string(v, 32768)
                || v.is_boolean()
                || v.as_f64()
                    .is_some_and(|n| n.is_finite() && n.abs() <= 1_000_000_000.0)
        }
        "disabled" | "checked" => v.is_boolean(),
        "level" => v.as_u64().is_some_and(|n| (1..=6).contains(&n)),
        "max" => v.as_f64().is_some_and(|n| n > 0.0 && n <= 1_000_000_000.0),
        "headers" | "items" => v
            .as_array()
            .is_some_and(|a| a.len() <= 500 && a.iter().all(|x| string(x, 4096))),
        "rows" => v.as_array().is_some_and(|a| {
            a.len() <= 500
                && a.iter().all(|r| {
                    r.as_array()
                        .is_some_and(|r| r.len() <= 20 && r.iter().all(|x| string(x, 4096)))
                })
        }),
        "options" => v.as_array().is_some_and(|a| {
            a.len() <= 500
                && a.iter().all(|x| {
                    x.as_object().is_some_and(|o| {
                        o.len() == 2 && string(&x["label"], 4096) && string(&x["value"], 4096)
                    })
                })
        }),
        _ => false,
    }
}
impl Tree {
    pub fn apply(&mut self, records: &[Value]) -> Result<()> {
        self.apply_within(records, limits::CALLBACKS)
    }
    /// Applies a batch only if the committed tree holds at most `callbacks`
    /// live callbacks: the remainder of the plugin-wide budget for this view.
    pub fn apply_within(&mut self, records: &[Value], callbacks: usize) -> Result<()> {
        self.apply_started(records, Instant::now(), callbacks)
    }
    fn apply_started(
        &mut self,
        records: &[Value],
        started: Instant,
        allowed_callbacks: usize,
    ) -> Result<()> {
        let budget = || {
            if started.elapsed() > Duration::from_millis(50) {
                Err(ProtocolError::new(
                    ErrorCode::ResourceLimit,
                    "Plugin UI validation budget exceeded",
                ))
            } else {
                Ok(())
            }
        };
        budget()?;
        if records.len() > limits::MUTATIONS {
            return Err(invalid());
        }
        let mut candidate = self.clone();
        // Content-only updates cannot change callbacks; every structural or
        // listener change revalidates the candidate and recounts them.
        let mut callbacks = candidate.validate_counted()?;
        let mut bytes = serde_json::to_vec(&candidate).map_err(|_| invalid())?.len();
        for record in records {
            budget()?;
            let mut content_only = false;
            let r = record.as_array().ok_or_else(invalid)?;
            let kind = r.first().and_then(Value::as_u64).ok_or_else(invalid)?;
            let id = r.get(1).and_then(Value::as_str).ok_or_else(invalid)?;
            match kind {
                0 if r.len() == 4 => {
                    let node: Node = serde_json::from_value(r[2].clone()).map_err(|_| invalid())?;
                    // Moving an existing node is supported, but never under itself/descendants.
                    if contains(&node, id) || id == node.id {
                        return Err(invalid());
                    }
                    remove_id(&mut candidate.children, &node.id);
                    let children = children_mut(&mut candidate.children, id).ok_or_else(invalid)?;
                    let index = r[3].as_u64().ok_or_else(invalid)? as usize;
                    if index > children.len() {
                        return Err(invalid());
                    }
                    children.insert(index, node);
                }
                1 if r.len() == 3 => {
                    let children = children_mut(&mut candidate.children, id).ok_or_else(invalid)?;
                    let index = r[2].as_u64().ok_or_else(invalid)? as usize;
                    if index >= children.len() {
                        return Err(invalid());
                    }
                    children.remove(index);
                }
                2 if r.len() == 3 => {
                    let node = find_mut(&mut candidate.children, id).ok_or_else(invalid)?;
                    if ![3, 8].contains(&node.kind) {
                        return Err(invalid());
                    }
                    if !string(&r[2], 32768) {
                        return Err(invalid());
                    }
                    let next = r[2].as_str().ok_or_else(invalid)?;
                    bytes = bytes - json_len(&node.data)? + json_len(&next)?;
                    node.data = Some(next.into());
                    content_only = true;
                }
                3 if r.len() == 4 || r.len() == 5 => {
                    let node = find_mut(&mut candidate.children, id).ok_or_else(invalid)?;
                    if node.kind != 1 {
                        return Err(invalid());
                    }
                    let key = r[2].as_str().ok_or_else(invalid)?.to_string();
                    let mutation_kind = match r.get(4) {
                        None => 1,
                        Some(value) => value.as_u64().ok_or_else(invalid)?,
                    };
                    let target = match mutation_kind {
                        1 if property(&key, &r[3]) => &mut node.properties,
                        3 if matches!(key.as_str(), "press" | "change") => {
                            &mut node.event_listeners
                        }
                        _ => return Err(invalid()),
                    };
                    // Ordinary properties cannot change node/callback identity.
                    // Account for the exact JSON key/value and comma delta rather
                    // than serializing unrelated rows/text after every update.
                    if mutation_kind == 1 {
                        bytes = object_update_bytes(bytes, target, &key, &r[3])?;
                        content_only = true;
                    }
                    if r[3].is_null() {
                        target.remove(&key);
                    } else {
                        target.insert(key, r[3].clone());
                    }
                }
                _ => return Err(invalid()),
            }
            // Bound growth during the batch, as well as the final committed state.
            if content_only {
                if bytes > limits::TREE {
                    return Err(invalid());
                }
            } else {
                callbacks = candidate.validate_counted()?;
                bytes = serde_json::to_vec(&candidate).map_err(|_| invalid())?.len();
            }
            budget()?;
        }
        budget()?;
        if callbacks > allowed_callbacks {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "Plugin callback limit exceeded",
            ));
        }
        *self = candidate;
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        self.validate_counted().map(|_| ())
    }
    fn validate_counted(&self) -> Result<usize> {
        let mut ids = HashSet::new();
        let mut callbacks = HashSet::new();
        for node in &self.children {
            validate_node(node, 1, &mut ids, &mut callbacks)?
        }
        if serde_json::to_vec(self).map_err(|_| invalid())?.len() > limits::TREE {
            return Err(invalid());
        }
        Ok(callbacks.len())
    }
    /// Live callback IDs in this validated tree.
    pub fn callback_count(&self) -> usize {
        fn count(nodes: &[Node]) -> usize {
            nodes
                .iter()
                .map(|n| n.event_listeners.len() + count(&n.children))
                .sum()
        }
        count(&self.children)
    }
    pub fn callback(&self, node_id: &str, event: &str, callback_id: &str) -> bool {
        find(&self.children, node_id)
            .and_then(|n| n.event_listeners.get(event))
            .is_some_and(|v| v["callbackId"].as_str() == Some(callback_id))
    }
}
fn json_len(value: &impl Serialize) -> Result<usize> {
    Ok(serde_json::to_vec(value).map_err(|_| invalid())?.len())
}
fn object_update_bytes(
    total: usize,
    object: &BTreeMap<String, Value>,
    key: &str,
    value: &Value,
) -> Result<usize> {
    match (object.get(key), value.is_null()) {
        (Some(old), false) => Ok(total - json_len(old)? + json_len(value)?),
        (Some(old), true) => {
            Ok(total - json_len(&key)? - 1 - json_len(old)? - usize::from(object.len() > 1))
        }
        (None, false) => {
            Ok(total + json_len(&key)? + 1 + json_len(value)? + usize::from(!object.is_empty()))
        }
        (None, true) => Ok(total),
    }
}
#[cfg(test)]
mod budget_tests {
    use super::*;
    #[test]
    fn incremental_property_sizes_match_actual_json_for_insert_replace_and_remove() {
        for initial in [
            BTreeMap::new(),
            BTreeMap::from([("label".to_string(), serde_json::json!("old"))]),
            BTreeMap::from([
                ("label".to_string(), serde_json::json!("old")),
                ("title".into(), serde_json::json!("retained")),
            ]),
        ] {
            for key in ["label", "new\"key"] {
                for value in [
                    serde_json::json!("escaped\n雪\""),
                    serde_json::json!(["first", "second"]),
                    Value::Null,
                ] {
                    let expected = object_update_bytes(
                        100 + json_len(&initial).unwrap(),
                        &initial,
                        key,
                        &value,
                    )
                    .unwrap();
                    let mut changed = initial.clone();
                    if value.is_null() {
                        changed.remove(key);
                    } else {
                        changed.insert(key.into(), value);
                    }
                    assert_eq!(expected, 100 + json_len(&changed).unwrap());
                }
            }
        }
    }
    #[test]
    fn exhausted_budget_cannot_commit_any_mutation() {
        let mut tree = Tree::default();
        let expired = Instant::now() - Duration::from_millis(51);
        let error = tree
            .apply_started(&[], expired, limits::CALLBACKS)
            .unwrap_err();
        assert_eq!(error.data.code, ErrorCode::ResourceLimit);
        assert!(tree.children.is_empty());
    }
}
fn validate_node<'a>(
    node: &'a Node,
    depth: usize,
    ids: &mut HashSet<&'a str>,
    callbacks: &mut HashSet<&'a str>,
) -> Result<()> {
    if !identifier(&node.id)
        || !ids.insert(&node.id)
        || ids.len() > limits::NODES
        || depth > limits::DEPTH
        || !node.attributes.is_empty()
    {
        return Err(invalid());
    }
    match node.kind {
        1 => {
            let tag = node.element.as_deref().ok_or_else(invalid)?;
            if !TAGS.contains(&tag)
                || node.data.is_some()
                || node.properties.iter().any(|(k, v)| !property(k, v))
            {
                return Err(invalid());
            }
            for (event, value) in &node.event_listeners {
                let permitted = match event.as_str() {
                    "press" => tag == "cmx-button" || tag == "cmx-markdown",
                    "change" => [
                        "cmx-text-field",
                        "cmx-text-area",
                        "cmx-select",
                        "cmx-checkbox",
                        "cmx-switch",
                        "cmx-tabs",
                    ]
                    .contains(&tag),
                    _ => false,
                };
                let id = value["callbackId"].as_str().ok_or_else(invalid)?;
                if !permitted
                    || !value.as_object().is_some_and(|o| o.len() == 1)
                    || !identifier(id)
                    || !callbacks.insert(id)
                    || callbacks.len() > limits::CALLBACKS
                {
                    return Err(invalid());
                }
            }
        }
        3 | 8 => {
            if node.element.is_some()
                || node.data.as_ref().is_none_or(|s| s.len() > 32768)
                || !node.children.is_empty()
                || !node.properties.is_empty()
                || !node.event_listeners.is_empty()
            {
                return Err(invalid());
            }
        }
        _ => return Err(invalid()),
    }
    for child in &node.children {
        validate_node(child, depth + 1, ids, callbacks)?
    }
    Ok(())
}
fn contains(node: &Node, id: &str) -> bool {
    node.id == id || node.children.iter().any(|n| contains(n, id))
}
fn find<'a>(nodes: &'a [Node], id: &str) -> Option<&'a Node> {
    for node in nodes {
        if node.id == id {
            return Some(node);
        }
        if let Some(v) = find(&node.children, id) {
            return Some(v);
        }
    }
    None
}
fn find_mut<'a>(nodes: &'a mut [Node], id: &str) -> Option<&'a mut Node> {
    for node in nodes {
        if node.id == id {
            return Some(node);
        }
        if let Some(v) = find_mut(&mut node.children, id) {
            return Some(v);
        }
    }
    None
}
fn children_mut<'a>(nodes: &'a mut Vec<Node>, id: &str) -> Option<&'a mut Vec<Node>> {
    if id == "~" {
        Some(nodes)
    } else {
        find_mut(nodes, id)
            .filter(|n| n.kind == 1)
            .map(|n| &mut n.children)
    }
}
fn remove_id(nodes: &mut Vec<Node>, id: &str) -> bool {
    if let Some(i) = nodes.iter().position(|n| n.id == id) {
        nodes.remove(i);
        true
    } else {
        nodes.iter_mut().any(|n| remove_id(&mut n.children, id))
    }
}
