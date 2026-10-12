//! Provider-neutral workflow tools. Caller identity is captured from the
//! admitted dispatch, never accepted from model arguments or MCP metadata.

use super::{Dispatch, TaskSpec, TaskStatus, WorkflowService};
use crate::agent_provider::managed::{ManagedTool, ManagedToolHandler};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn graph_tools(dispatch: Dispatch, service: WorkflowService) -> Arc<dyn ManagedToolHandler> {
    Arc::new(GraphTools { dispatch, service })
}

struct GraphTools {
    dispatch: Dispatch,
    service: WorkflowService,
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> ManagedTool {
    ManagedTool {
        name: name.into(),
        description: description.into(),
        input_schema: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
    }
}

#[async_trait]
impl ManagedToolHandler for GraphTools {
    fn tools(&self) -> Vec<ManagedTool> {
        let task_schema = json!({"type":"object","properties":{
            "id":{"type":"string"},"title":{"type":"string"},"prompt":{"type":"string"},
            "dependencies":{"type":"array","items":{"type":"string"}},"route_id":{"type":["string","null"]},
            "access":{"type":"string","enum":["read_only","write"]},"scope":{"type":"array","items":{"type":"string"}},
            "output_schema":{"type":["object","boolean","null"]},"required":{"type":"boolean"}},
            "required":["id","title","prompt"],"additionalProperties":false});
        let id = json!({"type":"string"});
        vec![
            tool("workflow_add_tasks","Add bounded child tasks to this run. Reuse a stable idempotency_key when retrying the same request. Children inherit or narrow your permissions.",json!({"tasks":{"type":"array","items":task_schema.clone(),"maxItems":500},"idempotency_key":id.clone()}),&["tasks","idempotency_key"]),
            tool("workflow_list","List this task and its descendants; statuses are evidence, not inferred from agent prose.",json!({}),&[]),
            tool("workflow_wait","Read child results or yield until they finish. If state is waiting, end your turn immediately; CodeMux will resume your task with results. Do not poll or launch native subagents.",json!({"task_ids":{"type":"array","items":id.clone(),"minItems":1,"maxItems":500}}),&["task_ids"]),
            tool("workflow_message","Send bounded guidance to this task or a descendant. It is delivered on the next dispatched attempt, without interrupting unrelated work.",json!({"task_id":id.clone(),"text":{"type":"string"},"idempotency_key":id.clone()}),&["task_id","text","idempotency_key"]),
            tool("workflow_cancel","Cancel this task or a descendant. Authority is revoked immediately; write ownership is retained until execution is known to have stopped.",json!({"task_id":id.clone(),"idempotency_key":id.clone()}),&["task_id","idempotency_key"]),
            tool("workflow_replace","Replace a stopped child task. Creates a new generation and invalidates results that depend on its prior artifact.",json!({"task_id":id.clone(),"spec":task_schema,"idempotency_key":id.clone()}),&["task_id","spec","idempotency_key"]),
            tool("workflow_retire","Retire a child without deleting its evidence, conversation or files.",json!({"task_id":id.clone(),"idempotency_key":id}),&["task_id","idempotency_key"]),
        ]
    }

    async fn call(&self, name: &str, arguments: Value) -> Result<Value, String> {
        self.service.authorize_attempt(&self.dispatch)?;
        if arguments.to_string().len() > 256 * 1024 {
            return Err("Workflow tool request is too large".into());
        }
        let run_id = &self.dispatch.run_id;
        match name {
            "workflow_list" => {
                let run = self.service.snapshot(run_id)?;
                Ok(json!(run.tasks.iter().filter(|task|is_descendant(&run,&self.dispatch.task_id,&task.spec.id)).map(|task|json!({"id":task.spec.id,"title":task.spec.title,"status":task.status,"generation":task.generation,"dependencies":task.spec.dependencies})).collect::<Vec<_>>()))
            }
            "workflow_wait" => {
                let ids: Vec<String> = serde_json::from_value(
                    arguments
                        .get("task_ids")
                        .cloned()
                        .ok_or("task_ids is required")?,
                )
                .map_err(|e| e.to_string())?;
                if ids.is_empty() || ids.len() > 500 {
                    return Err("Wait requires 1–500 task IDs".into());
                }
                let run = self.service.snapshot(run_id)?;
                let mut pending = vec![];
                let mut results = vec![];
                let mut observations = vec![];
                for id in ids {
                    if id == self.dispatch.task_id
                        || !is_descendant(&run, &self.dispatch.task_id, &id)
                    {
                        return Err("A task can wait only for its descendants".into());
                    }
                    let task = run
                        .tasks
                        .iter()
                        .find(|task| task.spec.id == id)
                        .ok_or("Unknown task")?;
                    if matches!(
                        task.status,
                        TaskStatus::Succeeded
                            | TaskStatus::Failed
                            | TaskStatus::Cancelled
                            | TaskStatus::Blocked
                            | TaskStatus::Unknown
                    ) {
                        if task.status == TaskStatus::Succeeded {
                            observations.push((task.spec.id.clone(), task.generation));
                        }
                        results.push(super::scripts::task_result(task));
                    } else {
                        pending.push(id);
                    }
                }
                self.service
                    .observe_successful_dependencies(&self.dispatch, &observations)?;
                Ok(if pending.is_empty() {
                    json!({"state":"ready","results":results})
                } else {
                    json!({"state":"waiting","task_ids":pending,"results":results})
                })
            }
            "workflow_add_tasks" => {
                let tasks: Vec<TaskSpec> = serde_json::from_value(
                    arguments.get("tasks").cloned().ok_or("tasks is required")?,
                )
                .map_err(|e| e.to_string())?;
                let ids = tasks.iter().map(|task| task.id.clone()).collect::<Vec<_>>();
                self.service.add_tasks_as(
                    &self.dispatch,
                    tasks,
                    &scoped_key(&self.dispatch, name, string(&arguments, "idempotency_key")?)?,
                )?;
                Ok(json!({"task_ids":ids}))
            }
            "workflow_message" | "workflow_cancel" | "workflow_replace" | "workflow_retire" => {
                let key = scoped_key(&self.dispatch, name, string(&arguments, "idempotency_key")?)?;
                let payload = json!({"name":name,"arguments":arguments});
                self.service.read_or_execute(run_id, &key, &payload, || {
                    let target = string(&arguments, "task_id")?;
                    let run = match name {
                        "workflow_message" => self.service.message_as(
                            &self.dispatch,
                            target,
                            string(&arguments, "text")?,
                        )?,
                        "workflow_cancel" => self.service.cancel_task_as(&self.dispatch, target)?,
                        "workflow_replace" => self.service.replace_as(
                            &self.dispatch,
                            target,
                            serde_json::from_value(
                                arguments.get("spec").cloned().ok_or("spec is required")?,
                            )
                            .map_err(|e| e.to_string())?,
                        )?,
                        "workflow_retire" => self.service.retire_as(&self.dispatch, target)?,
                        _ => return Err("Unsupported workflow mutation".into()),
                    };
                    Ok(json!({"revision":run.revision}))
                })
            }
            _ => Err(format!("Unavailable workflow tool: {name}")),
        }
    }
}

fn scoped_key(dispatch: &Dispatch, name: &str, key: &str) -> Result<String, String> {
    if key.is_empty() || key.len() > 128 {
        return Err("Idempotency key must contain 1–128 bytes".into());
    }
    use sha2::{Digest, Sha256};
    // Parent generation remains stable across yield/continuation attempts.
    // Distinct parents can independently use ordinary keys such as `spawn`.
    Ok(format!(
        "tool-{:x}",
        Sha256::digest(
            json!([dispatch.task_id, dispatch.generation, name, key])
                .to_string()
                .as_bytes()
        )
    ))
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn is_descendant(run: &super::RunSnapshot, parent: &str, id: &str) -> bool {
    let mut current = Some(id);
    for _ in 0..=run.resolved_limits.max_depth {
        let Some(id) = current else { return false };
        if id == parent {
            return true;
        }
        current = run
            .tasks
            .iter()
            .find(|task| task.spec.id == id)
            .and_then(|task| task.parent_task_id.as_deref());
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn graph_idempotency_is_scoped_to_parent_generation() {
        let service = WorkflowService::open(":memory:", 2).unwrap();
        let spec=serde_json::from_value(json!({"workspace_id":"demo","title":"Delegation","goal":"Test without models",
            "routes":[{"id":"claude","provider":"claude"}],
            "tasks":[{"id":"a","title":"A","prompt":"Coordinate"},{"id":"b","title":"B","prompt":"Coordinate"}]})).unwrap();
        let run = service.create(spec, "graph-test").unwrap();
        let a = service.claim_next().unwrap().unwrap();
        service.mark_running(&a).unwrap();
        let b = service.claim_next().unwrap().unwrap();
        service.mark_running(&b).unwrap();
        let a_tools = graph_tools(a.clone(), service.clone());
        let b_tools = graph_tools(b.clone(), service.clone());
        let request = json!({"idempotency_key":"spawn","tasks":[{"id":"a-child","title":"Child A","prompt":"Inspect"}]});
        let first = a_tools
            .call("workflow_add_tasks", request.clone())
            .await
            .unwrap();
        assert_eq!(
            a_tools.call("workflow_add_tasks", request).await.unwrap(),
            first
        );
        b_tools
            .call(
                "workflow_add_tasks",
                json!({"idempotency_key":"spawn","run_id":"forged",
            "tasks":[{"id":"b-child","title":"Child B","prompt":"Inspect"}]}),
            )
            .await
            .unwrap();
        let snapshot = service.snapshot(&run.id).unwrap();
        assert_eq!(snapshot.tasks.len(), 4);
        assert_eq!(
            snapshot
                .tasks
                .iter()
                .find(|task| task.spec.id == "b-child")
                .unwrap()
                .parent_task_id
                .as_deref(),
            Some("b")
        );
        let mut continuation = a.clone();
        continuation.attempt_id = "next-attempt".into();
        assert_eq!(
            scoped_key(&a, "workflow_add_tasks", "spawn").unwrap(),
            scoped_key(&continuation, "workflow_add_tasks", "spawn").unwrap()
        );
        continuation.generation += 1;
        assert_ne!(
            scoped_key(&a, "workflow_add_tasks", "spawn").unwrap(),
            scoped_key(&continuation, "workflow_add_tasks", "spawn").unwrap()
        );
        service.cancel_task(&run.id, "a").unwrap();
        assert!(a_tools
            .call(
                "workflow_message",
                json!({"task_id":"a-child","text":"Late","idempotency_key":"late"})
            )
            .await
            .is_err());
    }
}
