//! The control plane's tool registry: one declaration per tool and one
//! authorized dispatch path that every route uses.
//!
//! Transports authenticate a caller and pick a [`Route`]. Visibility, the access
//! gate, argument validation and handler selection all happen here, once.
//! Adding a tool is a [`ToolId`] variant plus two exhaustive matches: its
//! declaration in `ToolId::spec` (`catalog.rs`) and its handler arm in [`run`].
//! Neither compiles without the other.
use super::{
    native, service, wait, Access, Audience, CallerKind, CallerView, ControlAccess, ControlCaller,
    ControlError, ErrorCode,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::borrow::Cow;
use tauri::{AppHandle, Runtime};

macro_rules! tool_ids {
    ($($id:ident),* $(,)?) => {
        /// Every tool the registry serves.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum ToolId { $($id),* }

        impl ToolId {
            /// Listing order on every route.
            pub const ALL: &'static [ToolId] = &[$(ToolId::$id),*];
        }
    };
}

tool_ids! {
    WorkspaceList, AgentCapabilities, ThreadList, ThreadLaunch, ThreadSend, ThreadRead,
    ThreadStatus, ThreadWait, ThreadInterrupt, ThreadStop, ThreadRespond, OperationStatus,
    OrchestratorCapabilities, CreateThreads, ThreadSearch, PluginList, PluginInvoke,
}

/// What a call targets, read from its arguments. Declared so project-scoped
/// grants can become one check in [`dispatch`]; nothing enforces it yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Instance,
    Workspace,
    Thread,
    Operation,
    Plugin,
}

/// MCP behaviour hints for a tool that is not read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effect {
    /// It may end or release work in a way that cannot be undone.
    pub destructive: bool,
    /// Repeating the identical call changes nothing more (receipts and keys).
    pub idempotent: bool,
}

/// How a successful result becomes MCP content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presentation {
    /// `structuredContent` is the value; the text is its compact JSON.
    Json,
    /// Plugin results: `images` become image content blocks.
    PluginResult,
}

/// A tool's advertised shape for one caller. The registry adds the name and
/// the annotations.
pub struct Definition {
    pub description: Cow<'static, str>,
    pub input_schema: Value,
}

/// One tool's declaration. Its fields are private: a constructor named after
/// the access class is the only way to build one, so no tool skips the gate.
#[derive(Clone, Copy)]
pub struct ToolSpec {
    name: &'static str,
    access: Access,
    audience: Audience,
    scope: Scope,
    effect: Effect,
    waits: bool,
    collaboration: bool,
    presentation: Presentation,
    def: fn(&CallerView) -> Option<Definition>,
}

impl ToolSpec {
    const fn new(
        name: &'static str,
        access: Access,
        audience: Audience,
        scope: Scope,
        effect: Effect,
        def: fn(&CallerView) -> Option<Definition>,
    ) -> Self {
        Self { name, access, audience, scope, effect, waits: false, collaboration: true, presentation: Presentation::Json, def }
    }

    pub const fn read(name: &'static str, audience: Audience, scope: Scope, def: fn(&CallerView) -> Option<Definition>) -> Self {
        Self::new(name, Access::Read, audience, scope, Effect { destructive: false, idempotent: true }, def)
    }

    pub const fn write(name: &'static str, audience: Audience, scope: Scope, effect: Effect, def: fn(&CallerView) -> Option<Definition>) -> Self {
        Self::new(name, Access::Write, audience, scope, effect, def)
    }

    pub const fn starts_threads(name: &'static str, audience: Audience, scope: Scope, effect: Effect, def: fn(&CallerView) -> Option<Definition>) -> Self {
        Self::new(name, Access::StartsThreads, audience, scope, effect, def)
    }

    pub const fn machine(name: &'static str, audience: Audience, scope: Scope, effect: Effect, def: fn(&CallerView) -> Option<Definition>) -> Self {
        Self::new(name, Access::Machine, audience, scope, effect, def)
    }

    pub const fn admin(name: &'static str, audience: Audience, scope: Scope, effect: Effect, def: fn(&CallerView) -> Option<Definition>) -> Self {
        Self::new(name, Access::Admin, audience, scope, effect, def)
    }

    /// The call may park for a time the client chooses. Transports budget such
    /// calls apart and end them when the client goes away.
    pub const fn waits(mut self) -> Self {
        self.waits = true;
        self
    }

    /// Agents may call it while Agent collaboration is off (plugin tools).
    pub const fn without_collaboration(mut self) -> Self {
        self.collaboration = false;
        self
    }

    pub const fn presented(mut self, presentation: Presentation) -> Self {
        self.presentation = presentation;
        self
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn access(&self) -> Access {
        self.access
    }

    pub fn audience(&self) -> Audience {
        self.audience
    }

    pub fn scope(&self) -> Scope {
        self.scope
    }

    pub fn effect(&self) -> Effect {
        self.effect
    }

    pub fn may_wait(&self) -> bool {
        self.waits
    }

    /// An Agent collaboration tool: agents reach it only while collaboration is on.
    pub fn collaboration(&self) -> bool {
        self.collaboration
    }

    pub fn presentation(&self) -> Presentation {
        self.presentation
    }
}

/// The route a call arrived on. The transport chooses it, never the arguments.
#[derive(Clone, Copy)]
pub(crate) enum Route<'a> {
    /// The explicit-argument contract: socket, stdio, HTTP, in-process callers.
    Outside,
    /// An in-app chat's tool call. Arguments may omit `workspace_id`, which
    /// then comes from the calling chat or the target thread. Plugin host calls
    /// have no actor.
    InApp { actor: Option<&'a crate::delegation::app::NativeActor> },
}

impl Route<'_> {
    fn audience(self) -> Audience {
        match self {
            Route::Outside => Audience::Outside,
            Route::InApp { .. } => Audience::InApp,
        }
    }
}

/// A validated call on its way to a handler.
pub(crate) struct Call<'a, R: Runtime> {
    pub app: &'a AppHandle<R>,
    pub caller: &'a ControlCaller,
    pub route: Route<'a>,
    pub args: Value,
}

/// What `tools/list` serves.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: Cow<'static, str>,
    pub input_schema: Value,
    pub annotations: Value,
}

pub struct Listed {
    pub spec: ToolSpec,
    pub definition: ToolDefinition,
}

/// Bare socket commands that predate `tool_call`. Frozen: new tools are
/// reachable only through `tool_call`, so a tool name never collides with a
/// socket command such as `automation_list`.
const SOCKET_ALIASES: [&str; 13] = [
    "agent_capabilities", "thread_list", "thread_launch", "thread_send", "thread_read", "thread_status",
    "thread_wait", "thread_interrupt", "thread_stop", "thread_respond", "operation_status", "plugin_list",
    "plugin_invoke",
];

pub fn lookup(name: &str) -> Option<(ToolId, ToolSpec)> {
    ToolId::ALL.iter().map(|id| (*id, id.spec())).find(|(_, spec)| spec.name == name)
}

/// The tool, if `audience`'s route serves it.
pub fn served(name: &str, audience: Audience) -> Option<(ToolId, ToolSpec)> {
    lookup(name).filter(|(_, spec)| reaches(spec.audience, audience))
}

fn reaches(declared: Audience, route: Audience) -> bool {
    declared == Audience::Both || declared == route
}

/// The tools `view` may run, in [`ToolId::ALL`] order. Callers are offered only
/// what they can run; a hidden tool's call still fails with the gate's code.
pub fn list(view: &CallerView) -> Vec<Listed> {
    let agent = matches!(view.kind, CallerKind::Agent { .. });
    ToolId::ALL
        .iter()
        .map(|id| id.spec())
        .filter(|spec| reaches(spec.audience, view.audience))
        .filter(|spec| view.collaboration || !spec.collaboration || !agent)
        .filter(|spec| spec.access.listed(view))
        .filter_map(|spec| {
            let Definition { description, input_schema } = (spec.def)(view)?;
            let definition = ToolDefinition { name: spec.name, description, input_schema, annotations: annotations(&spec) };
            Some(Listed { spec, definition })
        })
        .collect()
}

fn annotations(spec: &ToolSpec) -> Value {
    if spec.access == Access::Read {
        json!({"readOnlyHint": true, "openWorldHint": false})
    } else {
        json!({
            "readOnlyHint": false,
            "destructiveHint": spec.effect.destructive,
            "idempotentHint": spec.effect.idempotent,
            "openWorldHint": false,
        })
    }
}

/// The outside catalog for `caller`: HTTP `tools/list` and socket `tools_list`.
pub fn tools_for(caller: &ControlCaller) -> Vec<ToolDefinition> {
    list(&CallerView::of(caller, Audience::Outside)).into_iter().map(|tool| tool.definition).collect()
}

/// Every outside tool with the least access that runs it and the first
/// sentence of its description, for generated docs and capabilities.
pub fn outside_tool_index() -> Vec<(&'static str, ControlAccess, &'static str)> {
    list(&CallerView::outside(ControlAccess::FullAccess))
        .into_iter()
        .filter_map(|tool| {
            let summary = first_sentence(static_text(&tool.definition.description)?);
            Some((tool.spec.name, tool.spec.access.minimum(), summary))
        })
        .collect()
}

/// Outside definitions are built from static text (`catalog.rs`), so their
/// descriptions borrow it; in-app variants own theirs.
pub(super) fn static_text(text: &Cow<'static, str>) -> Option<&'static str> {
    match text {
        Cow::Borrowed(text) => Some(*text),
        Cow::Owned(_) => None,
    }
}

fn first_sentence(text: &'static str) -> &'static str {
    text.find(". ").map_or(text, |end| &text[..=end])
}

/// The unauthenticated provider relay (`codemux-self`, `.mcp.json` stdio) must
/// refuse registry tools it has no legacy implementation for.
pub fn relay_refuses(name: &str) -> bool {
    lookup(name).is_some() && !crate::mcp_server::is_legacy_tool(name)
}

pub fn is_socket_alias(name: &str) -> bool {
    SOCKET_ALIASES.contains(&name)
}

/// The access gate, for argument-dependent escalation inside a handler (a
/// launch into a new worktree needs Machine) and for admin socket commands.
/// The gate does not depend on the route.
pub fn require(caller: &ControlCaller, access: Access) -> Result<(), ControlError> {
    access.check(&CallerView::of(caller, Audience::Outside))
}

/// The one pipeline: lookup, authorize, the caller's definition, the access
/// gate, argument validation, then the handler.
///
/// Authorization comes first, so a revoked or stale caller gets that answer
/// whatever it calls. Access comes before arguments, so a caller learns
/// nothing about a tool's arguments it may not run.
pub(crate) async fn dispatch<R: Runtime>(
    app: &AppHandle<R>,
    caller: &ControlCaller,
    route: Route<'_>,
    name: &str,
    args: Value,
) -> Result<Value, ControlError> {
    let audience = route.audience();
    let (id, spec) = served(name, audience).ok_or_else(|| unknown(name))?;
    service::authorize_native(app, caller, spec.collaboration)?;
    let view = CallerView::of(caller, audience);
    let definition = (spec.def)(&view).ok_or_else(|| unknown(name))?;
    spec.access.check(&view)?;
    if !args.is_object() {
        return Err(ControlError::new(ErrorCode::InvalidArguments, "Arguments must be an object"));
    }
    super::schema::check(&definition.input_schema, &args)
        .map_err(|violation| ControlError::new(ErrorCode::InvalidArguments, violation.to_string()))?;
    run(id, Call { app, caller, route, args }).await
}

fn unknown(name: &str) -> ControlError {
    ControlError::new(ErrorCode::UnknownTool, format!("Unknown tool: {name}"))
}

/// The outside route, MCP-shaped. `Ok` for every handled call, tool errors
/// included; `Err` only for `unknown_tool`, which doors report as JSON-RPC -32602.
pub async fn call<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, name: &str, args: Value) -> Result<Value, ControlError> {
    match dispatch(app, caller, Route::Outside, name, args).await {
        Err(error) if error.code == ErrorCode::UnknownTool => Err(error),
        result => Ok(mcp_result(name, result)),
    }
}

async fn run<R: Runtime>(id: ToolId, call: Call<'_, R>) -> Result<Value, ControlError> {
    use ToolId::*;
    let Call { app, caller, route, args } = call;
    let actor = || match route {
        Route::InApp { actor: Some(actor) } => Ok(actor),
        _ => Err(ControlError::new(ErrorCode::StaleCaller, "This tool needs the calling chat")),
    };
    match (id, route) {
        (WorkspaceList, _) => service::workspace_list(app, args),
        (AgentCapabilities, _) => service::agent_capabilities(app, caller, args).await,
        (ThreadList, Route::Outside) => service::thread_list(app, args),
        (ThreadList, Route::InApp { .. }) => native::in_app_inventory(app, actor()?, "thread_list", args),
        (ThreadSearch, _) => native::in_app_inventory(app, actor()?, "thread_search", args),
        (OrchestratorCapabilities, _) => native::in_app_capabilities(app, caller, actor()?, args).await,
        (ThreadLaunch, Route::Outside) => service::launch(app, caller, args).await,
        (ThreadLaunch, Route::InApp { .. }) => native::in_app_launch(app, caller, actor()?, args).await,
        (CreateThreads, _) => native::in_app_create_threads(app, caller, actor()?, args).await,
        (ThreadSend, route) => service::thread_send(app, caller, native::target_workspace(app, route, args)?).await,
        (ThreadRead, Route::Outside) => service::thread_read(app, caller, args),
        (ThreadRead, route) => native::read_bounded(app, caller, native::target_workspace(app, route, args)?),
        (ThreadStatus, _) => wait::thread_status(app, args).await,
        (ThreadWait, route) => wait::thread_wait(app, caller, native::target_workspace(app, route, args)?).await,
        (ThreadInterrupt, route) => service::thread_interrupt(app, caller, native::target_workspace(app, route, args)?).await,
        (ThreadStop, _) => service::thread_stop(app, caller, args).await,
        (ThreadRespond, _) => service::thread_respond(app, caller, args).await,
        (OperationStatus, _) => service::operation_status(app, caller, args),
        (PluginList | PluginInvoke, _) => crate::plugins::platform_transport::agent_request(app, caller.clone(), id.spec().name, args)
            .await
            .map_err(|error| ControlError::new(ErrorCode::PluginOperationFailed, error)),
    }
}

/// The one CallToolResult shape for registry tools on every outside door.
pub fn mcp_result(name: &str, result: Result<Value, ControlError>) -> Value {
    match result {
        Ok(value) if lookup(name).is_some_and(|(_, spec)| spec.presentation == Presentation::PluginResult) => {
            crate::plugins::platform_transport::mcp_result(value)
        }
        Ok(Value::String(text)) => {
            json!({"isError": false, "content": [{"type": "text", "text": text}], "structuredContent": {"result": text}})
        }
        Ok(value) => {
            let text = value.to_string();
            let structured = if value.is_object() { value } else { json!({"result": value}) };
            json!({"isError": false, "content": [{"type": "text", "text": text}], "structuredContent": structured})
        }
        Err(error) => mcp_error(&error),
    }
}

pub fn mcp_error(error: &ControlError) -> Value {
    json!({"isError": true, "content": [{"type": "text", "text": error.to_string()}], "structuredContent": {"error": error}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_provider::ProviderKind;
    use ControlAccess::{FullAccess, ReadOnly, Supervised};

    /// The views every catalog is served to.
    fn views() -> Vec<CallerView> {
        let mut views: Vec<CallerView> = [ReadOnly, Supervised, FullAccess].into_iter().map(CallerView::outside).collect();
        for kind in [CallerKind::Client, CallerKind::Agent { full_access: true }] {
            views.push(CallerView { kind, ..CallerView::outside(FullAccess) });
        }
        views.push(CallerView::in_app(ProviderKind::Claude, Some("bypassPermissions"), true));
        views.push(CallerView::in_app(ProviderKind::Codex, Some("plan"), true));
        views.push(CallerView::in_app(ProviderKind::Claude, Some("default"), false));
        views
    }

    fn names(view: &CallerView) -> Vec<&'static str> {
        list(view).iter().map(|tool| tool.spec.name).collect()
    }

    #[test]
    fn every_tool_id_has_one_unique_name() {
        let mut seen = std::collections::HashSet::new();
        for id in ToolId::ALL {
            let spec = id.spec();
            assert!(seen.insert(spec.name), "{} is declared twice", spec.name);
            assert_eq!(lookup(spec.name).map(|(found, _)| found), Some(*id));
        }
        assert!(lookup("nope").is_none());
    }

    #[test]
    fn registry_schemas_use_only_the_validated_vocabulary() {
        for view in views() {
            for tool in list(&view) {
                let schema = &tool.definition.input_schema;
                let errors = super::super::schema::vocabulary_errors(schema);
                assert!(errors.is_empty(), "{} ({:?}): {errors:?}", tool.spec.name, view.audience);
                assert_eq!(schema["type"], "object", "{}", tool.spec.name);
                assert_eq!(schema["additionalProperties"], false, "{}", tool.spec.name);
            }
        }
    }

    #[test]
    fn legacy_twins_are_outside_only() {
        // An in-app chat would otherwise receive two tools under one prefixed name.
        for id in ToolId::ALL {
            let spec = id.spec();
            if crate::mcp_server::is_legacy_tool(spec.name) {
                assert_eq!(spec.audience, Audience::Outside, "{}", spec.name);
            }
        }
    }

    #[test]
    fn relay_refuses_registry_tools_without_legacy_twins() {
        for name in ["thread_launch", "thread_status", "create_threads"] {
            assert!(relay_refuses(name), "{name}");
        }
        for name in ["workspace_list", "browser_click", "nope"] {
            assert!(!relay_refuses(name), "{name}");
        }
    }

    #[test]
    fn relay_allowlist_names_only_legacy_tools() {
        // The allowlist governs legacy self tools; a registry-only tool is never
        // reachable through it.
        for name in crate::mcp::registry::RELAY_READ_TOOLS {
            assert!(crate::mcp_server::is_legacy_tool(name), "{name}");
            assert!(!relay_refuses(name), "{name}");
        }
    }

    #[test]
    fn socket_aliases_are_the_frozen_native_names() {
        assert_eq!(SOCKET_ALIASES.len(), 13);
        for name in SOCKET_ALIASES {
            assert!(served(name, Audience::Outside).is_some(), "{name}");
        }
        for name in ["workspace_list", "create_threads", "tool_call", "automation_list"] {
            assert!(!is_socket_alias(name), "{name}");
        }
    }

    #[test]
    fn listing_offers_only_what_the_caller_can_run() {
        for view in views() {
            let listed = names(&view);
            for id in ToolId::ALL {
                let spec = id.spec();
                let runnable = reaches(spec.audience, view.audience)
                    && (view.collaboration || !spec.collaboration || !matches!(view.kind, CallerKind::Agent { .. }))
                    && spec.access.check(&view).is_ok();
                assert_eq!(listed.contains(&spec.name), runnable, "{} for {view:?}", spec.name);
            }
        }
        assert_eq!(names(&CallerView::outside(ReadOnly)), [
            "workspace_list", "agent_capabilities", "thread_list", "thread_read", "thread_status", "thread_wait",
            "operation_status", "plugin_list",
        ], "a read-only connection is not offered tools that change anything");
        assert_eq!(names(&CallerView::outside(Supervised)).len(), 14);
        assert_eq!(names(&CallerView::in_app(ProviderKind::Claude, Some("default"), false)), ["plugin_list", "plugin_invoke"],
            "with Agent collaboration off, chats keep only plugin tools");
    }

    #[test]
    fn outside_tool_index_lists_outside_tools_with_minimum_access() {
        let index = outside_tool_index();
        assert_eq!(index.len(), list(&CallerView::outside(FullAccess)).len(), "every outside tool has static text");
        let entry = |name: &str| index.iter().find(|(tool, _, _)| *tool == name).copied();
        assert_eq!(entry("thread_launch").map(|(_, access, _)| access), Some(Supervised));
        assert_eq!(entry("workspace_list"), Some(("workspace_list", ReadOnly, "List workspaces on this CodeMux instance.")));
        assert!(entry("create_threads").is_none(), "in-app tools are not outside tools");
    }

    #[test]
    fn annotations_follow_access() {
        for tool in list(&CallerView::outside(FullAccess)) {
            let (spec, annotations) = (tool.spec, &tool.definition.annotations);
            if spec.access == Access::Read {
                assert_eq!(*annotations, json!({"readOnlyHint": true, "openWorldHint": false}), "{}", spec.name);
            } else {
                assert_eq!(*annotations, json!({"readOnlyHint": false, "destructiveHint": spec.effect.destructive,
                    "idempotentHint": spec.effect.idempotent, "openWorldHint": false}), "{}", spec.name);
            }
        }
        let destructive: Vec<&str> = list(&CallerView::outside(FullAccess)).into_iter()
            .filter(|tool| tool.definition.annotations["destructiveHint"] == true).map(|tool| tool.spec.name).collect();
        assert_eq!(destructive, ["thread_interrupt", "thread_stop", "thread_respond", "plugin_invoke"]);
        assert!(list(&CallerView::outside(FullAccess)).iter().all(|tool| tool.definition.annotations["idempotentHint"] != false),
            "every outside mutation is keyed, so a retry is safe");
    }

    #[test]
    fn mcp_results_have_one_shape() {
        let object = mcp_result("thread_list", Ok(json!({"threads": []})));
        assert_eq!(object, json!({"isError": false, "content": [{"type": "text", "text": "{\"threads\":[]}"}], "structuredContent": {"threads": []}}));
        let text = mcp_result("thread_list", Ok(json!("done")));
        assert_eq!(text["content"][0]["text"], "done");
        assert_eq!(text["structuredContent"], json!({"result": "done"}));
        assert_eq!(mcp_result("thread_list", Ok(json!([1])))["structuredContent"], json!({"result": [1]}));
        let plugin = mcp_result("plugin_invoke", Ok(json!({"ok": true, "images": [{"mimeType": "image/png", "data": "AA=="}]})));
        assert_eq!(plugin["content"][1], json!({"type": "image", "mimeType": "image/png", "data": "AA=="}));
        assert_eq!(plugin["structuredContent"], json!({"ok": true}));
        let error = mcp_result("thread_list", Err(ControlError::new(ErrorCode::WorkspaceNotFound, "gone")));
        assert_eq!(error, json!({"isError": true, "content": [{"type": "text", "text": "workspace_not_found: gone"}],
            "structuredContent": {"error": {"code": "workspace_not_found", "message": "gone", "retryable": false}}}));
        assert_eq!(mcp_error(&ControlError::new(ErrorCode::AppUnavailable, "down"))["structuredContent"]["error"]["retryable"], true);
    }

    /// An argument object that fills every property (`required_only`: only the
    /// required ones), with the first enum value, the shortest allowed string,
    /// the minimum number and one array item.
    fn example(schema: &Value, required_only: bool) -> Value {
        if let Some(first) = schema["enum"].get(0) {
            return first.clone();
        }
        let kind = match &schema["type"] {
            Value::Array(kinds) => kinds[0].as_str().unwrap_or_default(),
            kind => kind.as_str().unwrap_or_default(),
        };
        match kind {
            "object" => {
                let required: Vec<&str> = schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                let properties = schema["properties"].as_object().into_iter().flatten()
                    .filter(|(name, _)| !required_only || required.contains(&name.as_str()))
                    .map(|(name, property)| (name.clone(), example(property, required_only)));
                Value::Object(properties.collect())
            }
            "string" => {
                let length = schema["minLength"].as_u64().unwrap_or(1).max(1).min(schema["maxLength"].as_u64().unwrap_or(u64::MAX));
                json!("x".repeat(length as usize))
            }
            "integer" | "number" => schema["minimum"].clone().as_i64().map_or(json!(0), |minimum| json!(minimum)),
            "boolean" => json!(true),
            "array" => {
                let count = schema["minItems"].as_u64().unwrap_or(1).max(1);
                Value::Array((0..count).map(|_| example(&schema["items"], required_only)).collect())
            }
            "null" => Value::Null,
            _ => json!({}),
        }
    }

    #[test]
    fn outside_schema_examples_parse_into_typed_args() {
        use serde::de::DeserializeOwned;
        fn typed<T: DeserializeOwned>(args: Value) -> Result<(), ControlError> {
            service::parse::<T>(args).map(drop)
        }
        let parser = |name: &str| -> Option<fn(Value) -> Result<(), ControlError>> {
            Some(match name {
                "workspace_list" => typed::<service::WorkspaceListArgs>,
                "agent_capabilities" => typed::<service::CapabilitiesArgs>,
                "thread_list" => typed::<service::ThreadListArgs>,
                "thread_launch" => typed::<service::Launch>,
                "thread_send" => typed::<service::ThreadSendArgs>,
                "thread_read" => typed::<service::ThreadReadArgs>,
                "thread_status" => typed::<service::Target>,
                "thread_wait" => typed::<super::super::wait::Args>,
                "thread_interrupt" => typed::<service::InterruptArgs>,
                "thread_stop" => typed::<service::StopArgs>,
                "thread_respond" => typed::<service::RespondArgs>,
                "operation_status" => typed::<service::OperationStatusArgs>,
                _ => return None,
            })
        };
        // The plugin layer checks its own fields by hand.
        const UNTYPED: &[&str] = &["plugin_list", "plugin_invoke"];
        for tool in list(&CallerView::outside(FullAccess)) {
            let name = tool.spec.name;
            if UNTYPED.contains(&name) {
                continue;
            }
            let parse = parser(name).unwrap_or_else(|| panic!("{name} needs an entry here: its typed arguments"));
            for required_only in [false, true] {
                let mut args = example(&tool.definition.input_schema, required_only);
                if args.get("provider").is_some() {
                    args["provider"] = json!("claude");
                }
                super::super::schema::check(&tool.definition.input_schema, &args).unwrap_or_else(|v| panic!("{name}: {v}: {args}"));
                parse(args.clone()).unwrap_or_else(|error| panic!("{name} rejects its own schema's example {args}: {error}"));
            }
        }
    }
}
