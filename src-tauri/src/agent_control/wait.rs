//! Waiting on a native thread's run: `thread_wait` and `thread_status`, the
//! entry points every route calls. They forward to the service's polling
//! implementation until the event-driven wait lands here.
use super::{service, ControlCaller, ControlError};
use serde_json::Value;
use tauri::{AppHandle, Runtime};

pub(crate) async fn thread_wait<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    service::thread_wait(app, caller, args).await
}

pub(crate) async fn thread_status<R: Runtime>(app: &AppHandle<R>, args: Value) -> Result<Value, ControlError> {
    service::thread_status(app, args).await
}
