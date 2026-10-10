//! Device-local custom ACP definitions and durable thread identity. Never synced.
use super::*;
use crate::agent_provider::custom_acp::{
    config::{AcpAgent, AcpAgentInput, AcpLaunchConfig}, AcpBinding,
};
use std::collections::BTreeMap;

pub(super) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS custom_acp_agents (
            id TEXT PRIMARY KEY, definition TEXT NOT NULL, environment BLOB NOT NULL
        );
        CREATE TABLE IF NOT EXISTS custom_acp_bindings (
            thread_id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, binding TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS custom_acp_bindings_agent ON custom_acp_bindings(agent_id);",
    )
}

fn decode_definition(raw: &str) -> Result<AcpAgent, String> {
    let mut agent: AcpAgent = serde_json::from_str(raw)
        .map_err(|_| "The saved ACP definition is invalid; repair it in Agent settings.".to_string())?;
    // Even a damaged/legacy definition must not expose a value in a listing.
    for value in agent.environment.values_mut() { *value = None; }
    Ok(agent)
}

fn decode_environment(raw: &[u8]) -> Result<HashMap<String, String>, String> {
    let bytes = crate::auth::decrypt_data(raw)
        .map_err(|_| "The saved ACP environment cannot be decrypted on this device.".to_string())?;
    serde_json::from_slice(&bytes)
        .map_err(|_| "The saved ACP environment is invalid; repair it in Agent settings.".to_string())
}

fn project_binding(tx: &rusqlite::Transaction<'_>, binding: &AcpBinding) -> Result<(), String> {
    let effort = crate::agent_provider::custom_acp::catalog::semantic_option(&binding.catalog.config_options, "thought_level")
        .and_then(|option| option.current_value.as_str());
    let sdk = binding.session_id.as_deref().map(|native|
        serde_json::json!(["acp", binding.agent_id, binding.revision, native]).to_string());
    // One transaction for both authorities; never project delayed bridge state.
    tx.execute("UPDATE agent_chat_sessions SET model=?2, effort=?3
        WHERE thread_id=?1 AND provider='acp' AND cwd=?4
        AND (sdk_session_id IS NULL OR sdk_session_id=?5)",
        params![binding.thread_id, binding.catalog.current_model, effort, binding.cwd, sdk])
        .map_err(|e| e.to_string())?;
    Ok(())
}

impl DatabaseStore {
    pub fn acp_agents(&self) -> Result<Vec<AcpAgent>, String> {
        let conn = self.conn.lock().map_err(|_| "ACP storage is unavailable".to_string())?;
        let mut statement = conn.prepare("SELECT definition FROM custom_acp_agents ORDER BY rowid")
            .map_err(|e| e.to_string())?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?;
        rows.map(|row| row.map_err(|e| e.to_string()).and_then(|raw| decode_definition(&raw))).collect()
    }

    pub fn save_acp_agent(&self, input: AcpAgentInput) -> Result<AcpAgent, String> {
        input.validate()?;
        if input.id.as_ref().is_some_and(|id| uuid::Uuid::parse_str(id).is_err()) {
            return Err("Invalid ACP instance ID.".into());
        }
        let mut conn = self.conn.lock().map_err(|_| "ACP storage is unavailable".to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let previous: Option<(String, Vec<u8>)> = match &input.id {
            Some(id) => tx.query_row("SELECT definition, environment FROM custom_acp_agents WHERE id=?1", [id],
                |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(|e| e.to_string())?,
            None => None,
        };
        if input.id.is_some() && previous.is_none() {
            return Err("This ACP agent was removed. Refresh Agent settings before editing it.".into());
        }
        if previous.is_none() {
            let count: i64 = tx.query_row("SELECT COUNT(*) FROM custom_acp_agents", [], |row| row.get(0)).map_err(|e| e.to_string())?;
            if count >= 64 { return Err("At most 64 custom ACP agents may be configured.".into()); }
        }
        let old_agent = previous.as_ref().map(|(raw, _)| decode_definition(raw)).transpose()?;
        let old_environment = match previous.as_ref().map(|(_, raw)| decode_environment(raw)).transpose() {
            Ok(value) => value,
            Err(error) if input.environment.values().any(Option::is_none) => return Err(error),
            // Complete replacement/removal needs no old values. Unknown launch
            // equivalence deliberately rotates the revision below.
            Err(_) => None,
        };
        let mut environment = HashMap::new();
        for (name, value) in &input.environment {
            let value = match value {
                Some(value) => value.clone(),
                None => old_environment.as_ref().and_then(|old| old.get(name)).cloned().ok_or_else(|| "Cannot retain an environment variable that was not previously saved.".to_string())?,
            };
            environment.insert(name.clone(), value);
        }
        // Revalidate the resolved environment too: redacted retained values still count toward the size limit.
        AcpAgentInput {
            id: input.id.clone(), name: input.name.clone(), executable: input.executable.clone(), args: input.args.clone(),
            environment: environment.iter().map(|(name, value)| (name.clone(), Some(value.clone()))).collect(),
            enabled: input.enabled, auth_method: input.auth_method.clone(),
        }.validate()?;
        let launch_unchanged = old_agent.as_ref().is_some_and(|old| {
            old.executable == input.executable && old.args == input.args && old.auth_method == input.auth_method && old_environment.as_ref() == Some(&environment)
        });
        let agent = AcpAgent {
            id: input.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            name: input.name, executable: input.executable, args: input.args,
            environment: environment.keys().map(|name| (name.clone(), None)).collect::<BTreeMap<_, _>>(),
            enabled: input.enabled, auth_method: input.auth_method,
            revision: if launch_unchanged { old_agent.unwrap().revision } else { uuid::Uuid::new_v4().to_string() },
        };
        let definition = serde_json::to_string(&agent).map_err(|_| "Cannot encode the ACP definition".to_string())?;
        let environment = crate::auth::encrypt_data(&serde_json::to_vec(&environment).map_err(|_| "Cannot encode the ACP environment".to_string())?)?;
        tx.execute("INSERT INTO custom_acp_agents(id,definition,environment) VALUES (?1,?2,?3)
            ON CONFLICT(id) DO UPDATE SET definition=excluded.definition, environment=excluded.environment",
            params![agent.id, definition, environment]).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(agent)
    }

    pub fn acp_launch_config(&self, id: &str) -> Result<AcpLaunchConfig, String> {
        let conn = self.conn.lock().map_err(|_| "ACP storage is unavailable".to_string())?;
        let row: Option<(String, Vec<u8>)> = conn.query_row("SELECT definition,environment FROM custom_acp_agents WHERE id=?1", [id],
            |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(|e| e.to_string())?;
        let (raw, environment) = row.ok_or_else(|| "This ACP agent is missing. Restore its configuration or start a new chat.".to_string())?;
        let agent = decode_definition(&raw)?;
        if !agent.enabled { return Err("This ACP agent is disabled. Enable it in Agent settings to continue.".into()); }
        Ok(AcpLaunchConfig { agent, environment: decode_environment(&environment)? })
    }

    pub fn delete_acp_agent(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|_| "ACP storage is unavailable".to_string())?;
        conn.execute("DELETE FROM custom_acp_agents WHERE id=?1", [id]).map_err(|e| e.to_string())?;
        // Keep bindings and transcript history. They must never fall through to another definition.
        Ok(())
    }

    pub fn acp_binding(&self, thread: &str) -> Result<Option<AcpBinding>, String> {
        let conn = self.conn.lock().map_err(|_| "ACP storage is unavailable".to_string())?;
        let raw: Option<String> = conn.query_row("SELECT binding FROM custom_acp_bindings WHERE thread_id=?1", [thread], |row| row.get(0))
            .optional().map_err(|e| e.to_string())?;
        raw.map(|raw| serde_json::from_str(&raw).map_err(|_| "The saved ACP thread binding is invalid; it will not be replaced automatically.".to_string())).transpose()
    }

    pub fn update_acp_binding(&self, binding: &AcpBinding) -> Result<(), String> {
        // Updates belong only to the already admitted process. Definition edits
        // cannot rebind its identity, and this path cannot admit a new launch.
        let mut conn = self.conn.lock().map_err(|_| "ACP storage is unavailable".to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let previous: Option<String> = tx.query_row("SELECT binding FROM custom_acp_bindings WHERE thread_id=?1 AND agent_id=?2",
            params![binding.thread_id, binding.agent_id], |row| row.get(0)).optional().map_err(|e| e.to_string())?;
        let old: AcpBinding = serde_json::from_str(&previous.ok_or("ACP live binding is missing")?)
            .map_err(|_| "Invalid saved ACP binding".to_string())?;
        if old.thread_id != binding.thread_id || old.agent_id != binding.agent_id || old.revision != binding.revision
            || old.cwd != binding.cwd || old.session_id != binding.session_id || binding.session_id.is_none() {
            return Err("ACP live binding identity changed".into());
        }
        let serialized = serde_json::to_string(binding).map_err(|_| "Cannot encode the ACP binding".to_string())?;
        tx.execute("UPDATE custom_acp_bindings SET binding=?3 WHERE thread_id=?1 AND agent_id=?2",
            params![binding.thread_id, binding.agent_id, serialized]).map_err(|e| e.to_string())?;
        project_binding(&tx, binding)?;
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn save_acp_binding(&self, binding: &AcpBinding) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|_| "ACP storage is unavailable".to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let raw_agent: Option<String> = tx.query_row("SELECT definition FROM custom_acp_agents WHERE id=?1", [&binding.agent_id], |row| row.get(0))
            .optional().map_err(|e| e.to_string())?;
        let agent = decode_definition(&raw_agent.ok_or_else(|| "The ACP agent was removed while this session was starting.".to_string())?)?;
        if !agent.enabled || agent.revision != binding.revision {
            return Err("The ACP launch configuration changed or was disabled. This chat will not switch configurations automatically.".into());
        }
        let previous: Option<String> = tx.query_row("SELECT binding FROM custom_acp_bindings WHERE thread_id=?1", [&binding.thread_id], |row| row.get(0))
            .optional().map_err(|e| e.to_string())?;
        if let Some(raw) = previous {
            let old: AcpBinding = serde_json::from_str(&raw).map_err(|_| "Invalid saved ACP binding".to_string())?;
            if old.agent_id != binding.agent_id || old.revision != binding.revision || old.cwd != binding.cwd {
                return Err("This chat is already bound to a different ACP launch configuration or workspace.".into());
            }
        }
        let serialized = serde_json::to_string(binding).map_err(|_| "Cannot encode the ACP binding".to_string())?;
        tx.execute("INSERT INTO custom_acp_bindings(thread_id,agent_id,binding) VALUES (?1,?2,?3)
            ON CONFLICT(thread_id) DO UPDATE SET binding=excluded.binding",
            params![binding.thread_id, binding.agent_id, serialized]).map_err(|e| e.to_string())?;
        project_binding(&tx, binding)?;
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn project_acp_binding(&self, thread: &str) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|_| "ACP storage is unavailable".to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let raw: String = tx.query_row("SELECT binding FROM custom_acp_bindings WHERE thread_id=?1", [thread], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let binding: AcpBinding = serde_json::from_str(&raw).map_err(|_| "Invalid saved ACP binding".to_string())?;
        project_binding(&tx, &binding)?;
        tx.commit().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> AcpAgentInput {
        AcpAgentInput { id: None, name: "Local harness".into(), executable: "dsh".into(), args: vec!["--profile".into(), "acp".into()], environment: [("TOKEN".into(), Some("synthetic-secret-for-test".into()))].into(), enabled: true, auth_method: None }
    }
    #[test]
    fn acp_definitions_encrypt_environment_and_list_only_redacted_values() {
        let db = init_test_database();
        let agent = db.save_acp_agent(input()).unwrap();
        assert_eq!(agent.environment["TOKEN"], None);
        let listed = db.acp_agents().unwrap();
        assert_eq!(listed.len(), 1);
        assert!(!serde_json::to_string(&listed).unwrap().contains("synthetic-secret-for-test"));
        assert_eq!(db.acp_launch_config(&agent.id).unwrap().environment["TOKEN"], "synthetic-secret-for-test");
        let conn = db.conn.lock().unwrap();
        let (definition, ciphertext): (String, Vec<u8>) = conn.query_row("SELECT definition,environment FROM custom_acp_agents", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        assert!(!definition.contains("synthetic-secret-for-test"));
        assert!(!ciphertext.windows(b"synthetic-secret-for-test".len()).any(|window| window == b"synthetic-secret-for-test"));
    }
    #[test]
    fn acp_cosmetic_edits_retain_launch_identity_and_redacted_environment() {
        let db = init_test_database(); let old = db.save_acp_agent(input()).unwrap();
        let mut edit = input(); edit.id = Some(old.id.clone()); edit.name = "Renamed harness".into(); edit.environment.insert("TOKEN".into(), None);
        let changed = db.save_acp_agent(edit).unwrap();
        assert_eq!(old.revision, changed.revision);
        assert_eq!(db.acp_launch_config(&old.id).unwrap().environment["TOKEN"], "synthetic-secret-for-test");
        let mut edit = input(); edit.id = Some(old.id.clone()); edit.args.push("literal value".into());
        assert_ne!(db.save_acp_agent(edit).unwrap().revision, old.revision);
    }
    #[test]
    fn acp_missing_retained_variables_and_removed_instances_are_not_resurrected() {
        let db = init_test_database(); let mut invalid = input(); invalid.environment.insert("MISSING".into(), None);
        assert!(db.save_acp_agent(invalid).is_err());
        assert!(db.acp_agents().unwrap().is_empty());
        let old = db.save_acp_agent(input()).unwrap(); db.delete_acp_agent(&old.id).unwrap();
        let mut edit = input(); edit.id = Some(old.id.clone());
        assert!(db.save_acp_agent(edit).is_err());
        assert!(db.acp_launch_config(&old.id).is_err());
    }
    #[test]
    fn review7_full_replacement_repairs_unreadable_environment_without_rebinding() {
        for invalid_json in [false, true] {
            for clear in [false, true] {
                let db = init_test_database();
                let old = db.save_acp_agent(input()).unwrap();
                let binding = AcpBinding { thread_id: "repair-thread".into(), agent_id: old.id.clone(), revision: old.revision.clone(), cwd: "unused".into(), session_id: Some("native".into()), catalog: crate::agent_provider::custom_acp::catalog::catalog_from(&old.id, "repair", &Default::default(), &serde_json::json!({})).unwrap(), config_values: HashMap::new() };
                db.save_acp_binding(&binding).unwrap();
                let corrupt = if invalid_json { crate::auth::encrypt_data(b"not JSON").unwrap() } else { b"unreadable ciphertext".to_vec() };
                db.conn.lock().unwrap().execute("UPDATE custom_acp_agents SET environment=?2 WHERE id=?1", params![old.id, corrupt]).unwrap();
                let mut edit = input(); edit.id = Some(old.id.clone());
                edit.environment = if clear { BTreeMap::new() } else { [("PUBLIC_SETTING".into(), Some("replacement".into()))].into() };
                let repaired = db.save_acp_agent(edit);
                assert!(repaired.is_ok(), "complete replacement needs no old secret: {:?}", repaired.as_ref().err());
                let repaired = repaired.unwrap();
                assert_eq!(repaired.id, old.id);
                assert_ne!(repaired.revision, old.revision, "unknown launch equivalence must rotate revision");
                let launch = db.acp_launch_config(&old.id).unwrap();
                assert_eq!(launch.environment.len(), usize::from(!clear));
                if !clear { assert_eq!(launch.environment["PUBLIC_SETTING"], "replacement"); }
                assert_eq!(serde_json::to_value(db.acp_binding("repair-thread").unwrap().unwrap()).unwrap(), serde_json::to_value(binding).unwrap());
                assert!(!serde_json::to_string(&db.acp_agents().unwrap()).unwrap().contains("replacement"));
            }
        }
    }
    #[test]
    fn review7_retaining_unreadable_secret_rejects_without_mutation() {
        let db = init_test_database(); let old = db.save_acp_agent(input()).unwrap();
        db.conn.lock().unwrap().execute("UPDATE custom_acp_agents SET environment=?2 WHERE id=?1", params![old.id, b"unreadable ciphertext".to_vec()]).unwrap();
        let before: (String, Vec<u8>) = db.conn.lock().unwrap().query_row("SELECT definition,environment FROM custom_acp_agents WHERE id=?1", [&old.id], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        let mut edit = input(); edit.id = Some(old.id.clone()); edit.environment.insert("TOKEN".into(), None);
        let error = db.save_acp_agent(edit).err().expect("retaining unreadable secret must reject");
        assert!(error.contains("cannot be decrypted"));
        let after: (String, Vec<u8>) = db.conn.lock().unwrap().query_row("SELECT definition,environment FROM custom_acp_agents WHERE id=?1", [&old.id], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(before, after);
        assert!(!error.contains("synthetic-secret"));
    }
    #[test]
    fn acp_disabled_instances_cannot_launch() {
        let db = init_test_database(); let mut value = input(); value.enabled = false;
        let agent = db.save_acp_agent(value).unwrap();
        assert!(db.acp_launch_config(&agent.id).is_err());
        assert_eq!(db.acp_agents().unwrap().len(), 1);
    }
}
