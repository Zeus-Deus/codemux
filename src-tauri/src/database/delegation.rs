//! Read only through the real database mutex, held through dispatch.
use super::*;
impl DatabaseStore {
    /// Called beneath the canonical state lease. The callback is synchronous;
    /// it must not reacquire DatabaseStore or AppStateStore locks.
    pub(crate) fn with_delegation_admission<T>(
        &self, thread: &str,
        f: impl FnOnce(AgentChatSessionRecord, Option<i64>, &dyn Fn(i64) -> Result<String, String>) -> Result<T, String>,
    ) -> Result<T, String> {
        let conn = self.conn.lock().unwrap();
        let record = agent_chat_session(&conn, thread).ok_or("Parent is missing from the canonical session database")?;
        let event = conn.query_row("SELECT MAX(id) FROM agent_chat_messages WHERE thread_id=?1", [thread], |r| r.get::<_,Option<i64>>(0)).map_err(|e|e.to_string())?;
        let host = |id| conn.query_row("SELECT ssh_target FROM hosts WHERE id=?1 AND user_id='local' AND deleted_at IS NULL", [id], |r| r.get::<_,String>(0)).map_err(|_|"Configured SSH host is missing or deleted".into());
        f(record,event,&host)
    }
}
