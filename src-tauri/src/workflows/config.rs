//! Host policy limits execution across runs. Repository policy can only narrow
//! a requested run; neither a script nor a repository can grant file access.
use super::{RunSpec, WorkflowService};
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostPolicy {
    pub global_concurrency: usize,
    pub provider_concurrency: BTreeMap<String, usize>,
    pub run_caps: RunCaps,
}
impl Default for HostPolicy {
    fn default() -> Self {
        Self {
            global_concurrency: 8,
            provider_concurrency: BTreeMap::new(),
            run_caps: RunCaps::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunCaps {
    pub concurrency: Option<usize>,
    pub max_tasks: Option<usize>,
    pub max_attempts: Option<usize>,
    pub max_depth: Option<usize>,
    pub token_budget: Option<u64>,
    pub max_output_bytes: Option<usize>,
    pub wall_time_ms: Option<u64>,
    pub deny_writes: bool,
}

fn read_optional<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_file() {
        return Err("Workflow policy must be a regular file".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(error) => return Err(format!("Cannot read {}: {error}", path.display())),
    };
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Workflow policy must be a regular file".into());
    }
    // Read a bounded prefix instead of trusting metadata (pipes and races).
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65_536 {
        return Err("Workflow policy exceeds 64 KiB".into());
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| format!("Invalid workflow policy {}: {e}", path.display()))
}

impl HostPolicy {
    pub fn load(path: &Path) -> Result<Self, String> {
        let policy: Self = read_optional(path)?;
        if !(1..=256).contains(&policy.global_concurrency) {
            return Err("Host workflow concurrency must be 1–256".into());
        }
        for (provider, capacity) in &policy.provider_concurrency {
            super::capabilities::provider_kind(provider)?;
            if !(1..=256).contains(capacity) {
                return Err("Provider workflow concurrency must be 1–256".into());
            }
        }
        policy.run_caps.validate()?;
        Ok(policy)
    }
    pub fn configure(&self, service: &WorkflowService) -> Result<(), String> {
        for (provider, capacity) in &self.provider_concurrency {
            service.set_provider_capacity(provider, *capacity)?;
        }
        Ok(())
    }
    pub fn constrain(&self, spec: &mut RunSpec, workspace: &Path) -> Result<(), String> {
        self.run_caps.apply(spec)?;
        let project: RunCaps = read_optional(&workspace.join(".codemux/workflows.json"))?;
        project.apply(spec)
    }
}

impl RunCaps {
    fn validate(&self) -> Result<(), String> {
        if self.concurrency.is_some_and(|n| !(1..=256).contains(&n))
            || self.max_tasks.is_some_and(|n| !(1..=10_000).contains(&n))
            || self
                .max_attempts
                .is_some_and(|n| !(1..=50_000).contains(&n))
            || self.max_depth.is_some_and(|n| n > 32)
            || self.token_budget == Some(0)
            || self
                .max_output_bytes
                .is_some_and(|n| !(1..=16_777_216).contains(&n))
            || self
                .wall_time_ms
                .is_some_and(|n| !(1..=86_400_000).contains(&n))
        {
            return Err("Workflow policy has an invalid resource cap".into());
        }
        Ok(())
    }
    fn apply(&self, spec: &mut RunSpec) -> Result<(), String> {
        self.validate()?;
        let limits = &mut spec.limits;
        if let Some(cap) = self.concurrency {
            limits.concurrency = if limits.concurrency == 0 {
                cap.min(4)
            } else {
                limits.concurrency.min(cap)
            };
        }
        if let Some(cap) = self.max_tasks {
            limits.max_tasks = limits.max_tasks.min(cap);
        }
        if let Some(cap) = self.max_attempts {
            limits.max_attempts = limits.max_attempts.min(cap);
        }
        if let Some(cap) = self.max_depth {
            limits.max_depth = limits.max_depth.min(cap);
        }
        if let Some(cap) = self.max_output_bytes {
            limits.max_output_bytes = limits.max_output_bytes.min(cap);
        }
        if let Some(cap) = self.wall_time_ms {
            limits.wall_time_ms = limits.wall_time_ms.min(cap);
        }
        if let Some(cap) = self.token_budget {
            limits.token_budget = Some(
                limits
                    .token_budget
                    .map_or(cap, |requested| requested.min(cap)),
            );
        }
        if self.deny_writes && spec.allow_writes {
            return Err("File changes are disabled by workflow policy".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_caps_cannot_widen_requested_limits() {
        let mut spec: RunSpec = serde_json::from_value(
            serde_json::json!({"workspace_id":"demo","title":"Audit","goal":"Review","routes":[]}),
        )
        .unwrap();
        spec.limits.concurrency = 2;
        spec.limits.token_budget = Some(100);
        let caps: RunCaps = serde_json::from_value(
            serde_json::json!({"concurrency":32,"max_tasks":3,"token_budget":500,"max_depth":0}),
        )
        .unwrap();
        caps.apply(&mut spec).unwrap();
        assert_eq!(spec.limits.concurrency, 2);
        assert_eq!(spec.limits.token_budget, Some(100));
        assert_eq!(spec.limits.max_tasks, 3);
        assert_eq!(spec.limits.max_depth, 0);
        spec.allow_writes = true;
        assert!(RunCaps {
            deny_writes: true,
            ..Default::default()
        }
        .apply(&mut spec)
        .is_err());
    }
    #[test]
    fn invalid_or_unknown_policy_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("workflows.json");
        assert_eq!(HostPolicy::load(&file).unwrap().global_concurrency, 8);
        for json in [
            r#"{"global_concurrency":0}"#,
            r#"{"provider_concurrency":{"other":2}}"#,
            r#"{"run_caps":{"max_tasks":0}}"#,
            r#"{"unlimited":true}"#,
        ] {
            std::fs::write(&file, json).unwrap();
            assert!(HostPolicy::load(&file).is_err(), "{json}");
        }
        std::fs::write(&file, vec![b' '; 65_537]).unwrap();
        assert!(HostPolicy::load(&file).is_err());
    }
}
