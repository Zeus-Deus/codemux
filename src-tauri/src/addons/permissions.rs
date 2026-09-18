use super::{ErrorCode, Manifest, ProtocolError, Result};
use codemux_addon_protocol::manifest::Permission;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
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
        self.interaction
            .map(|(time, _)| time + Duration::from_secs(10))
    }
}
#[derive(Default)]
pub struct Contexts {
    handles: HashMap<String, Context>,
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
    pub fn issue(
        &mut self,
        generation: &str,
        workspace: Option<super::workspace::Workspace>,
        composer: Option<String>,
    ) -> Result<String> {
        if self.handles.len() >= 4096 {
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
    pub fn interact(&mut self, handle: &str, generation: &str, now: Instant) -> Result<String> {
        let mut context = self.get(handle, generation)?.clone();
        context.interaction = Some((now, false));
        self.handles.retain(|_, c| {
            !c.interaction.as_ref().is_some_and(|(issued, used)| {
                *used || now.saturating_duration_since(*issued) >= Duration::from_secs(10)
            })
        });
        if self.handles.len() >= 4096 {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "Context handle limit",
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.handles.insert(id.clone(), context);
        Ok(id)
    }
    pub fn consume(&mut self, handle: &str, generation: &str, now: Instant) -> Result<Context> {
        self.get(handle, generation)?;
        let context = self.handles.get_mut(handle).unwrap();
        match &mut context.interaction {
            Some((issued, used))
                if !*used && now.saturating_duration_since(*issued) < Duration::from_secs(10) =>
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
        self.revision = self.revision.wrapping_add(1)
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
