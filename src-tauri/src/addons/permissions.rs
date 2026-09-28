use super::{ErrorCode, Manifest, ProtocolError, Result};
use codemux_addon_protocol::manifest::Permission;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Source {
    Catalog {
        publisher: String,
        repository: String,
    },
    Local {
        identity: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Grant {
    pub installation_id: String,
    pub plugin_id: String,
    pub source: Source,
    pub digest: String,
    pub capabilities: String,
}
pub fn capability_digest(manifest: &Manifest) -> String {
    // Sort declarations so field/order-only differences cannot request authority.
    let mut permissions = manifest
        .permissions
        .iter()
        .map(|p| serde_json::to_string(p).unwrap())
        .collect::<Vec<_>>();
    permissions.sort();
    let mut http = manifest
        .http
        .iter()
        .map(|h| {
            let mut methods = h
                .methods
                .iter()
                .map(|m| serde_json::to_string(m).unwrap())
                .collect::<Vec<_>>();
            methods.sort();
            serde_json::json!({"origin":h.origin,"methods":methods,"credential":h.credential})
                .to_string()
        })
        .collect::<Vec<_>>();
    http.sort();
    let mut credentials = manifest
        .credentials
        .iter()
        .map(|c| serde_json::json!({"id":c.id,"origin":c.origin,"type":c.kind}).to_string())
        .collect::<Vec<_>>();
    credentials.sort();
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(permissions, http, credentials)).unwrap())
    )
}
impl Grant {
    pub fn check(
        &self,
        installation: &str,
        manifest: &Manifest,
        source: &Source,
        digest: &str,
    ) -> Result<()> {
        if self.installation_id != installation
            || self.plugin_id != manifest.id
            || &self.source != source
            || self.digest != digest
            || self.capabilities != capability_digest(manifest)
        {
            return Err(ProtocolError::new(
                ErrorCode::PermissionDenied,
                "This release needs permission review",
            ));
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct Context {
    pub generation: String,
    pub workspace: Option<super::workspace::Workspace>,
    pub composer: Option<String>,
    pub revision: u64,
    pub cancel: CancellationToken,
    interaction: Option<(Instant, bool)>,
}
impl Context {
    pub fn interaction_deadline(&self) -> Option<Instant> {
        self.interaction.map(|(time, _)| time + INTERACTION)
    }
}
/// Handles per plugin generation, so one plugin cannot exhaust another's.
const HANDLES: usize = 4096;
/// Interaction handles kept per generation. Using or expiring an interaction
/// never invalidates its context; only this bound evicts the oldest handles.
const INTERACTIONS: usize = 256;
const INTERACTION: Duration = Duration::from_secs(10);
#[derive(Default)]
pub struct Contexts {
    handles: HashMap<String, Context>,
    interactions: HashMap<String, VecDeque<String>>,
    revision: u64,
    composers: HashMap<String, String>,
}
impl Contexts {
    pub fn register_composer(&mut self, id: String, workspace: String) -> Result<()> {
        if uuid::Uuid::parse_str(&id).is_err() || self.composers.len() >= 64 {
            return Err(ProtocolError::invalid("Invalid composer registration"));
        }
        self.revoke_composer(&id);
        self.composers.insert(id, workspace);
        Ok(())
    }
    pub fn validate_composer(&self, id: &str, workspace: Option<&str>) -> Result<()> {
        if self.composers.get(id).map(String::as_str) != workspace || workspace.is_none() {
            return Err(ProtocolError::new(
                ErrorCode::NoComposer,
                "The chat composer is no longer available",
            ));
        }
        Ok(())
    }
    /// Registered composers in one workspace, to explain an ambiguous target.
    pub fn composers_in(&self, workspace: &str) -> usize {
        self.composers.values().filter(|w| *w == workspace).count()
    }
    pub fn issue(
        &mut self,
        generation: &str,
        workspace: Option<super::workspace::Workspace>,
        composer: Option<String>,
    ) -> Result<String> {
        if self
            .handles
            .values()
            .filter(|c| c.generation == generation)
            .count()
            >= HANDLES
        {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "Context handle limit",
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.handles.insert(
            id.clone(),
            Context {
                generation: generation.into(),
                workspace,
                composer,
                revision: self.revision,
                cancel: CancellationToken::new(),
                interaction: None,
            },
        );
        Ok(id)
    }
    pub fn get(&self, handle: &str, generation: &str) -> Result<&Context> {
        self.handles
            .get(handle)
            .filter(|c| {
                c.generation == generation
                    && c.revision == self.revision
                    && !c.cancel.is_cancelled()
            })
            .ok_or_else(|| {
                ProtocolError::new(ErrorCode::ContextStale, "The project or target changed")
            })
    }
    /// A clone was valid when read; it stays valid until its target changes,
    /// even if its handle was since evicted by newer interactions.
    pub fn live(&self, context: &Context) -> Result<()> {
        if context.revision == self.revision && !context.cancel.is_cancelled() {
            Ok(())
        } else {
            Err(ProtocolError::new(
                ErrorCode::ContextStale,
                "The project or target changed",
            ))
        }
    }
    /// Derives an interaction from a view context, sharing its cancellation.
    pub fn interact(&mut self, handle: &str, generation: &str, now: Instant) -> Result<String> {
        let mut context = self.get(handle, generation)?.clone();
        context.interaction = Some((now, false));
        let id = uuid::Uuid::new_v4().to_string();
        self.handles.insert(id.clone(), context);
        self.track(generation, id.clone());
        Ok(id)
    }
    /// Turns a handle issued for one command execution into its interaction,
    /// so executing a command leaves no separate base handle behind.
    pub fn promote(&mut self, handle: &str, generation: &str, now: Instant) -> Result<()> {
        self.get(handle, generation)?;
        self.handles.get_mut(handle).unwrap().interaction = Some((now, false));
        self.track(generation, handle.into());
        Ok(())
    }
    fn track(&mut self, generation: &str, handle: String) {
        let issued = self.interactions.entry(generation.into()).or_default();
        issued.push_back(handle);
        while issued.len() > INTERACTIONS {
            // Evict without cancelling: the token is shared with the parent
            // view, and an in-flight request rechecks its clone with live().
            if let Some(old) = issued.pop_front() {
                self.handles.remove(&old);
            }
        }
    }
    pub fn consume(&mut self, handle: &str, generation: &str, now: Instant) -> Result<Context> {
        self.get(handle, generation)?;
        let context = self.handles.get_mut(handle).unwrap();
        match &mut context.interaction {
            Some((issued, used))
                if !*used && now.saturating_duration_since(*issued) < INTERACTION =>
            {
                *used = true;
                Ok(context.clone())
            }
            _ => Err(ProtocolError::new(
                ErrorCode::InteractionRequired,
                "Use this action again; its interaction expired",
            )),
        }
    }
    pub fn revoke(&mut self, handle: &str) {
        if let Some(context) = self.handles.remove(handle) {
            context.cancel.cancel();
        }
        self.handles
            .retain(|_, context| !context.cancel.is_cancelled());
    }
    pub fn revoke_generation(&mut self, generation: &str) {
        self.interactions.remove(generation);
        self.handles.retain(|_, c| {
            if c.generation == generation {
                c.cancel.cancel();
                false
            } else {
                true
            }
        })
    }
    pub fn revoke_composer(&mut self, composer: &str) {
        self.composers.remove(composer);
        self.handles.retain(|_, c| {
            if c.composer.as_deref() == Some(composer) {
                c.cancel.cancel();
                false
            } else {
                true
            }
        })
    }
    pub fn change_workspace(&mut self) {
        for c in self.handles.values() {
            c.cancel.cancel()
        }
        self.handles.clear();
        self.interactions.clear();
        self.revision = self.revision.wrapping_add(1)
    }
    #[cfg(test)]
    pub fn handle_count(&self) -> usize {
        self.handles.len()
    }
}
pub fn require(manifest: &Manifest, permission: Permission) -> Result<()> {
    if manifest.permissions.contains(&permission) {
        Ok(())
    } else {
        Err(ProtocolError::new(
            ErrorCode::PermissionDenied,
            "This operation was not granted",
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interaction_is_single_use_and_bound_to_generation_and_revision() {
        let mut contexts = Contexts::default();
        let base = contexts.issue("a", None, Some("composer".into())).unwrap();
        let now = Instant::now();
        let click = contexts.interact(&base, "a", now).unwrap();
        assert!(contexts.consume(&click, "b", now).is_err());
        assert!(contexts.consume(&click, "a", now).is_ok());
        assert!(contexts.consume(&click, "a", now).is_err());
        let click = contexts.interact(&base, "a", now).unwrap();
        assert!(contexts
            .consume(&click, "a", now + Duration::from_secs(10))
            .is_err());
        let token = contexts.get(&base, "a").unwrap().cancel.clone();
        contexts.change_workspace();
        assert!(token.is_cancelled());
        assert!(contexts.get(&base, "a").is_err());
    }
    #[test]
    fn used_or_expired_interactions_keep_their_context_for_reads() {
        let mut contexts = Contexts::default();
        let base = contexts.issue("a", None, Some("composer".into())).unwrap();
        let now = Instant::now();
        let later = now + Duration::from_secs(11);
        let click = contexts.interact(&base, "a", now).unwrap();
        // Clicks elsewhere, including another plugin's, never prune it.
        let other = contexts.issue("b", None, None).unwrap();
        for _ in 0..10 {
            contexts.interact(&other, "b", later).unwrap();
            contexts.interact(&base, "a", later).unwrap();
        }
        assert!(contexts.get(&click, "a").is_ok());
        assert_eq!(
            contexts.consume(&click, "a", later).err().unwrap().data.code,
            ErrorCode::InteractionRequired
        );
        assert!(contexts.get(&click, "a").is_ok());
        let used = contexts.interact(&base, "a", now).unwrap();
        contexts.consume(&used, "a", now).unwrap();
        assert_eq!(
            contexts.consume(&used, "a", now).err().unwrap().data.code,
            ErrorCode::InteractionRequired
        );
        assert!(contexts.get(&used, "a").is_ok());
        assert_eq!(
            contexts.consume(&base, "a", now).err().unwrap().data.code,
            ErrorCode::InteractionRequired
        );
    }
    #[test]
    fn interactions_are_bounded_per_generation_without_cancelling_their_view() {
        let mut contexts = Contexts::default();
        let now = Instant::now();
        let view = contexts.issue("a", None, None).unwrap();
        let first = contexts.interact(&view, "a", now).unwrap();
        let held = contexts.get(&first, "a").unwrap().clone();
        let other_view = contexts.issue("b", None, None).unwrap();
        let other = contexts.interact(&other_view, "b", now).unwrap();
        for _ in 0..INTERACTIONS {
            contexts.interact(&view, "a", now).unwrap();
        }
        assert!(contexts.get(&first, "a").is_err(), "the oldest is evicted");
        assert!(contexts.live(&held).is_ok(), "an in-flight request continues");
        assert!(contexts.get(&view, "a").is_ok());
        assert!(contexts.get(&other, "b").is_ok());
        assert_eq!(contexts.handle_count(), 3 + INTERACTIONS);
        contexts.revoke(&view);
        assert!(contexts.live(&held).is_err());
        assert_eq!(contexts.handle_count(), 2);
    }
    #[test]
    fn execution_handles_become_the_interaction_and_limits_are_per_generation() {
        let mut contexts = Contexts::default();
        let now = Instant::now();
        let base = contexts.issue("a", None, None).unwrap();
        assert!(contexts.promote(&base, "b", now).is_err());
        contexts.promote(&base, "a", now).unwrap();
        assert_eq!(contexts.handle_count(), 1);
        contexts.consume(&base, "a", now).unwrap();
        for _ in 1..HANDLES {
            contexts.issue("a", None, None).unwrap();
        }
        assert_eq!(
            contexts.issue("a", None, None).err().unwrap().data.code,
            ErrorCode::ResourceLimit
        );
        assert!(contexts.issue("b", None, None).is_ok());
        contexts.revoke_generation("a");
        assert_eq!(contexts.handle_count(), 1);
    }
    #[test]
    fn source_and_release_are_part_of_the_grant() {
        let manifest = Manifest::parse(
            include_bytes!("../../addon-protocol/fixtures/hello.json"),
            None,
        )
        .unwrap();
        let source = Source::Local {
            identity: "one".into(),
        };
        let grant = Grant {
            installation_id: "install".into(),
            plugin_id: manifest.id.clone(),
            source: source.clone(),
            digest: "digest".into(),
            capabilities: capability_digest(&manifest),
        };
        assert!(grant.check("install", &manifest, &source, "digest").is_ok());
        assert!(grant.check("other", &manifest, &source, "digest").is_err());
        assert!(grant
            .check(
                "install",
                &manifest,
                &Source::Local {
                    identity: "two".into()
                },
                "digest"
            )
            .is_err());
        assert!(grant
            .check("install", &manifest, &source, "replacement")
            .is_err());
    }
}
