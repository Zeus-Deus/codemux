import type { MessageProvenance } from "@/tauri/events";

/** Only structured metadata identifies a sender; prompt text is never evidence. */
export function messageProvenance(value: MessageProvenance): MessageProvenance {
  const string = (field: unknown) => typeof field === "string" && field ? field : undefined;
  const notification = value.notification;
  const validNotification = notification && typeof notification.summary === "string"
    && notification.source && ["delegated_task","remote_task","subagent","automation","system"].includes(notification.source.kind)
    && Array.isArray(notification.source.task_ids) && notification.source.task_ids.every((id) => typeof id === "string")
    && ["completed","failed","cancelled","updated","unknown"].includes(notification.outcome);
  return {
    message_id: string(value.message_id),
    created_by: value.created_by && ["user","agent","system"].includes(value.created_by) ? value.created_by : "user",
    creation_source: value.creation_source && ["unknown","desktop","web","phone","mcp","provider","codemux"].includes(value.creation_source) ? value.creation_source : "unknown",
    sender_thread_id: string(value.sender_thread_id),
    sender_title: string(value.sender_title),
    input_intent: value.input_intent && ["turn_start","queued_turn","steer","promoted_queued_to_steer"].includes(value.input_intent) ? value.input_intent : "turn_start",
    notification: validNotification ? {source:{kind:notification.source.kind,task_ids:notification.source.task_ids,child_thread_id:string(notification.source.child_thread_id)},outcome:notification.outcome,summary:notification.summary,tasks:Array.isArray(notification.tasks) ? notification.tasks.filter((task) => task && typeof task.provider === "string" && typeof task.title === "string") : undefined} : undefined,
    automation_id: string(value.automation_id),
    automation_run_id: string(value.automation_run_id),
    delivery_id: string(value.delivery_id),
  };
}

export function messageAttribution(item: MessageProvenance): "agent" | "codemux" | "automation" | null {
  if (item.automation_id) return "automation";
  if (item.created_by === "system" || (item.created_by === "agent" && item.creation_source === "codemux")) return "codemux";
  if (item.created_by === "agent") return "agent";
  return null;
}

/** Result presentation requires persisted notification metadata. */
export function isDelegationResultsMessage(item: MessageProvenance & {text: string}): boolean {
  return item.notification?.source.kind === "delegated_task";
}
