//! Provider-aware skill inventory and exact turn-time resolution.
//!
//! Provider catalogs own native discovery while readable filesystem entries
//! provide the portable body/base-directory capability used across providers.
//! Catalog failures are isolated and returned with the successful inventory.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::agent_provider::codex::protocol::{Capabilities, ClientInfo, InitializeParams};
use crate::agent_provider::opencode::{
    OpenCodeClient, OpenCodeClientConfig, OpenCodeServerManager,
};
use crate::json_rpc_child::{JsonRpcChild, SpawnConfig};

use super::compatibility::classify_compatibility;
use super::parser::parse_skill_file;
use super::paths::enumerate_scan_paths;
use super::scanner::scan_directory;
use super::{
    skill_id_for_path, ResolvedSkillInvocation, Skill, SkillAdapterError, SkillAvailability,
    SkillInventory, SkillInvocationKind, SkillProjection, SkillProvenance, SkillProvider,
    SkillScope,
};

const CACHE_TTL: Duration = Duration::from_secs(60);
const CODEX_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
struct CacheEntry {
    loaded_at: Instant,
    inventory: SkillInventory,
}

#[derive(Default)]
pub struct SkillInventoryService {
    cache: Mutex<HashMap<(Option<PathBuf>, bool), CacheEntry>>,
}

impl SkillInventoryService {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn list(
        &self,
        cwd: Option<&Path>,
        include_plugins: bool,
        force: bool,
        opencode_manager: Option<&Arc<OpenCodeServerManager>>,
    ) -> Result<SkillInventory, String> {
        let canonical_cwd =
            cwd.map(|path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));
        let key = (canonical_cwd.clone(), include_plugins);
        if !force {
            if let Some(hit) = self.cache.lock().await.get(&key) {
                if hit.loaded_at.elapsed() < CACHE_TTL {
                    return Ok(hit.inventory.clone());
                }
            }
        }

        let mut inventory = filesystem_inventory(canonical_cwd.as_deref(), include_plugins);

        if let Some(cwd) = canonical_cwd.as_deref() {
            let codex = harvest_codex_catalog(cwd);
            let opencode = async {
                match opencode_manager {
                    Some(manager) => harvest_opencode_catalog(manager, cwd).await.map(Some),
                    None => Ok::<Option<Vec<Skill>>, String>(None),
                }
            };
            let (codex_result, opencode_result) = tokio::join!(codex, opencode);
            match codex_result {
                Ok((entries, errors)) => {
                    merge_catalog(&mut inventory.skills, entries);
                    inventory
                        .errors
                        .extend(errors.into_iter().map(|message| SkillAdapterError {
                            provider: SkillProvider::Codex,
                            message,
                        }));
                }
                Err(message) => inventory.errors.push(SkillAdapterError {
                    provider: SkillProvider::Codex,
                    message,
                }),
            }
            match opencode_result {
                Ok(Some(entries)) => merge_catalog(&mut inventory.skills, entries),
                Ok(None) => {}
                Err(message) => inventory.errors.push(SkillAdapterError {
                    provider: SkillProvider::Opencode,
                    message,
                }),
            }
        }

        stabilize_preference_ids(&mut inventory.skills, canonical_cwd.as_deref());
        sort_skills(&mut inventory.skills);
        self.cache.lock().await.insert(
            key,
            CacheEntry {
                loaded_at: Instant::now(),
                inventory: inventory.clone(),
            },
        );
        Ok(inventory)
    }

    pub async fn invalidate(&self) {
        self.cache.lock().await.clear();
    }

    pub async fn resolve(
        &self,
        cwd: &Path,
        include_plugins: bool,
        provider: SkillProvider,
        ids: &[String],
        opencode_manager: Option<&Arc<OpenCodeServerManager>>,
    ) -> Result<Vec<ResolvedSkillInvocation>, String> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let requested = ids.iter().cloned().collect::<HashSet<_>>();
        if requested.len() != ids.len() {
            return Err("duplicate_skill_reference".into());
        }

        let canonical_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        let cache_key = (Some(canonical_cwd.clone()), include_plugins);
        let mut hint = self
            .cache
            .lock()
            .await
            .get(&cache_key)
            .map(|entry| entry.inventory.clone());
        if hint.as_ref().is_none_or(|inventory| {
            !requested
                .iter()
                .all(|id| inventory.skills.iter().any(|skill| &skill.id == id))
        }) {
            // A catalog-only selection needs one authoritative discovery when
            // no prior popup inventory exists. The normal path reuses the
            // cached identity hints and avoids starting unrelated providers.
            hint = Some(
                self.list(
                    Some(&canonical_cwd),
                    include_plugins,
                    true,
                    opencode_manager,
                )
                .await?,
            );
        }
        let hint = hint.expect("inventory hint is populated above");
        let source_providers = hint
            .skills
            .iter()
            .filter(|skill| requested.contains(&skill.id))
            .map(|skill| skill.provider)
            .collect::<HashSet<_>>();

        let mut inventory = filesystem_inventory(Some(&canonical_cwd), include_plugins);
        if source_providers.contains(&SkillProvider::Codex) {
            if let Ok((entries, _)) = harvest_codex_catalog(&canonical_cwd).await {
                merge_catalog(&mut inventory.skills, entries);
            }
        }
        if source_providers.contains(&SkillProvider::Opencode) {
            if let Some(manager) = opencode_manager {
                if let Ok(entries) = harvest_opencode_catalog(manager, &canonical_cwd).await {
                    merge_catalog(&mut inventory.skills, entries);
                }
            }
        }
        stabilize_preference_ids(&mut inventory.skills, Some(&canonical_cwd));
        sort_skills(&mut inventory.skills);

        let by_id = inventory
            .skills
            .iter()
            .map(|skill| (skill.id.as_str(), skill))
            .collect::<HashMap<_, _>>();
        let mut resolved = Vec::with_capacity(ids.len());
        for id in ids {
            let skill = by_id
                .get(id.as_str())
                .ok_or_else(|| format!("skill_not_found: {id}"))?;
            let projection = skill
                .projections
                .iter()
                .find(|projection| projection.target_provider == provider)
                .ok_or_else(|| format!("skill_unavailable: {}", skill.name))?;
            if matches!(
                projection.availability,
                SkillAvailability::Unavailable | SkillAvailability::NativeOnly
            ) {
                return Err(format!("skill_unavailable: {}", skill.name));
            }
            let invocation = exact_invocation_kind(skill, projection, &inventory.skills)?;
            resolved.push(ResolvedSkillInvocation {
                skill_id: skill.id.clone(),
                name: skill.name.clone(),
                source_provider: skill.provider,
                source_scope: skill.scope,
                base_dir: (!skill.skill_dir.is_empty()).then(|| skill.skill_dir.clone()),
                path: (!skill.skill_dir.is_empty()).then(|| skill.file_path.clone()),
                body: skill.readable.then(|| skill.body.clone()),
                invocation,
            });
        }
        Ok(resolved)
    }
}

fn exact_invocation_kind(
    skill: &Skill,
    projection: &SkillProjection,
    inventory: &[Skill],
) -> Result<SkillInvocationKind, String> {
    if !matches!(projection.invocation, SkillInvocationKind::NativeCommand) {
        return Ok(projection.invocation);
    }
    let native_name_count = inventory
        .iter()
        .filter(|candidate| {
            candidate.provider == skill.provider
                && candidate.name == skill.name
                && candidate.projections.iter().any(|candidate_projection| {
                    candidate_projection.target_provider == projection.target_provider
                        && !matches!(
                            candidate_projection.availability,
                            SkillAvailability::Unavailable | SkillAvailability::NativeOnly
                        )
                })
        })
        .count();
    if native_name_count <= 1 {
        return Ok(projection.invocation);
    }
    if skill.readable {
        // A plain provider command would run that provider's precedence
        // winner, not necessarily the exact definition selected in Codemux.
        return Ok(SkillInvocationKind::PromptPrefix);
    }
    Err(format!(
        "skill_native_collision_unresolvable: {}",
        skill.name
    ))
}

fn filesystem_inventory(cwd: Option<&Path>, include_plugins: bool) -> SkillInventory {
    let mut skills = Vec::new();
    let base_paths = enumerate_scan_paths(cwd, include_plugins);
    for (path, provider) in base_paths.user_paths {
        skills.extend(scan_directory(&path, provider, SkillScope::User, None));
    }
    for (path, provider) in base_paths.project_paths {
        skills.extend(scan_directory(&path, provider, SkillScope::Project, None));
    }
    for (path, plugin_slug) in base_paths.plugin_paths {
        skills.extend(scan_directory(
            &path,
            SkillProvider::Claude,
            SkillScope::Plugin,
            Some(plugin_slug),
        ));
    }

    // Claude, Codex and OpenCode all have ancestor-scoped project discovery.
    // The normal enumerator intentionally scans only the supplied root for the
    // sync watcher, so add ancestor roots here for the cwd-scoped inventory.
    if let Some(cwd) = cwd {
        let mut seen = skills
            .iter()
            .map(|skill| skill.file_path.clone())
            .collect::<HashSet<_>>();
        for ancestor in cwd.ancestors().skip(1) {
            for (relative, provider) in [
                (".claude/skills", SkillProvider::Claude),
                (".codex/skills", SkillProvider::Codex),
                (".agents/skills", SkillProvider::Codex),
                (".opencode/skills", SkillProvider::Opencode),
                (".codemux/skills", SkillProvider::Codemux),
            ] {
                for skill in scan_directory(
                    &ancestor.join(relative),
                    provider,
                    SkillScope::Project,
                    None,
                ) {
                    if seen.insert(skill.file_path.clone()) {
                        skills.push(skill);
                    }
                }
            }
        }
    }
    sort_skills(&mut skills);
    SkillInventory {
        skills,
        errors: Vec::new(),
    }
}

fn stabilize_preference_ids(skills: &mut [Skill], cwd: Option<&Path>) {
    let Some(repo) = cwd.and_then(crate::git::git_canonical_root) else {
        return;
    };
    let Some(worktree_root) = repo.toplevel.as_deref() else {
        return;
    };
    let canonical_root = repo.canonical_root_path();
    for skill in skills
        .iter_mut()
        .filter(|skill| skill.scope == SkillScope::Project)
    {
        if let Some(preference_id) =
            project_preference_id(Path::new(&skill.file_path), worktree_root, &canonical_root)
        {
            skill.preference_id = preference_id;
        }
    }
}

fn project_preference_id(
    skill_path: &Path,
    worktree_root: &Path,
    canonical_root: &Path,
) -> Option<String> {
    let relative = skill_path.strip_prefix(worktree_root).ok()?;
    Some(skill_id_for_path(&format!(
        "preference:{}:{}",
        canonical_root.to_string_lossy(),
        relative.to_string_lossy()
    )))
}

async fn harvest_opencode_catalog(
    manager: &Arc<OpenCodeServerManager>,
    cwd: &Path,
) -> Result<Vec<Skill>, String> {
    let handle = manager.ensure_running().await?;
    let mut config = OpenCodeClientConfig::new(handle.base_url);
    config.server_password = Some(handle.server_password);
    let client = OpenCodeClient::new(config)?;
    let entries = client.list_skills(cwd).await?;
    Ok(entries
        .into_iter()
        .map(|entry| {
            catalog_skill(
                SkillProvider::Opencode,
                entry.name,
                entry.description,
                entry.location,
                Some(entry.content),
                true,
                SkillScope::Configured,
            )
        })
        .collect())
}

async fn harvest_codex_catalog(cwd: &Path) -> Result<(Vec<Skill>, Vec<String>), String> {
    let binary = which::which("codex").map_err(|_| "codex_not_installed".to_string())?;
    let child = JsonRpcChild::spawn(SpawnConfig {
        program: binary,
        args: vec!["app-server".into()],
        env: HashMap::new(),
        cwd: Some(cwd.to_path_buf()),
        default_timeout: CODEX_TIMEOUT,
    })
    .await
    .map_err(|error| format!("codex_skills_spawn_failed: {error}"))?;
    let child = Arc::new(child);
    let init = serde_json::to_value(InitializeParams {
        client_info: ClientInfo {
            name: "codemux-skill-inventory".into(),
            title: "Codemux".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        capabilities: Capabilities {
            experimental_api: true,
        },
    })
    .map_err(|error| format!("codex_skills_initialize_serialize: {error}"))?;
    child
        .request("initialize", init)
        .await
        .map_err(|error| format!("codex_skills_initialize_failed: {error}"))?;
    child
        .notify("initialized", json!({}))
        .await
        .map_err(|error| format!("codex_skills_initialized_failed: {error}"))?;
    let response = child
        .request(
            "skills/list",
            json!({ "cwds": [cwd.to_string_lossy()], "forceReload": false }),
        )
        .await
        .map_err(|error| format!("codex_skills_list_failed: {error}"))?;
    let _ = child.shutdown().await;
    Ok(parse_codex_catalog(response))
}

fn parse_codex_catalog(value: Value) -> (Vec<Skill>, Vec<String>) {
    let groups = value
        .get("data")
        .or_else(|| value.get("skills"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let errors = groups
        .iter()
        .flat_map(|group| {
            group
                .get("errors")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .map(|error| {
            error
                .as_str()
                .map(ToString::to_string)
                .or_else(|| {
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
                .unwrap_or_else(|| error.to_string())
        })
        .collect::<Vec<_>>();
    let entries = groups
        .into_iter()
        .flat_map(|entry| {
            entry
                .get("skills")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| vec![entry])
        })
        .collect::<Vec<_>>();
    let skills = entries
        .into_iter()
        .filter_map(|entry| {
            let name = entry.get("name")?.as_str()?.to_string();
            let path = entry
                .get("path")
                .or_else(|| entry.get("location"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let description = entry
                .get("description")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let enabled = entry
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let scope = match entry.get("scope").and_then(Value::as_str) {
                Some("repo") | Some("project") => SkillScope::Project,
                Some("system") => SkillScope::System,
                Some("admin") => SkillScope::Admin,
                Some("user") => SkillScope::User,
                _ => SkillScope::Configured,
            };
            Some(catalog_skill(
                SkillProvider::Codex,
                name,
                description,
                path.unwrap_or_else(|| "codex://native".into()),
                None,
                enabled,
                scope,
            ))
        })
        .collect();
    (skills, errors)
}

fn catalog_skill(
    provider: SkillProvider,
    name: String,
    description: Option<String>,
    location: String,
    body: Option<String>,
    enabled: bool,
    scope: SkillScope,
) -> Skill {
    let location_path = PathBuf::from(&location);
    let skill_path = if location_path.is_dir() {
        Some(location_path.join("SKILL.md"))
    } else if location_path.is_file() {
        Some(location_path)
    } else {
        None
    };
    let canonical_path = skill_path
        .as_ref()
        .map(|path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));
    let file_path = canonical_path
        .as_ref()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| location.clone());
    let skill_dir = if let Some(path) = canonical_path.as_deref() {
        path.parent()
            .unwrap_or(Path::new(""))
            .to_string_lossy()
            .to_string()
    } else {
        String::new()
    };
    let parsed_file = canonical_path
        .as_deref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|contents| parse_skill_file(&contents).ok());
    let raw_frontmatter = parsed_file
        .as_ref()
        .map(|parsed| parsed.frontmatter.clone())
        .unwrap_or(Value::Null);
    let body = body
        .filter(|body| !body.trim().is_empty())
        .or_else(|| parsed_file.map(|parsed| parsed.body))
        .unwrap_or_default();
    let readable = !body.trim().is_empty();
    let id = if canonical_path.is_some() {
        skill_id_for_path(&file_path)
    } else {
        skill_id_for_path(&format!("{provider:?}:{scope:?}:{name}:{location}"))
    };
    let projections = [
        SkillProvider::Claude,
        SkillProvider::Codex,
        SkillProvider::Opencode,
    ]
    .into_iter()
    .map(|target| {
        let native = target == provider;
        let (compatibility, reasons) =
            classify_compatibility(&body, &raw_frontmatter, provider, target);
        SkillProjection {
            target_provider: target,
            availability: if native && !enabled {
                SkillAvailability::Unavailable
            } else if native {
                if readable {
                    SkillAvailability::Native
                } else {
                    SkillAvailability::NativeOnly
                }
            } else if readable {
                SkillAvailability::ExplicitPortable
            } else {
                SkillAvailability::Unavailable
            },
            compatibility,
            reasons: if native && !enabled {
                vec!["The source provider reports this skill as disabled.".into()]
            } else if !native && !readable {
                vec!["The provider catalog did not expose readable skill content.".into()]
            } else {
                reasons
            },
            invocation: if native && !enabled {
                SkillInvocationKind::None
            } else if !readable {
                SkillInvocationKind::None
            } else if target == SkillProvider::Codex && canonical_path.is_some() {
                SkillInvocationKind::CodexSkillItem
            } else if native && target == SkillProvider::Claude {
                SkillInvocationKind::NativeCommand
            } else if native || readable {
                SkillInvocationKind::PromptPrefix
            } else {
                SkillInvocationKind::None
            },
        }
    })
    .collect::<Vec<_>>();
    let claude_projection = projections
        .iter()
        .find(|projection| projection.target_provider == SkillProvider::Claude)
        .expect("Claude projection is always computed");
    let compatibility = claude_projection.compatibility;
    let compatibility_signals = claude_projection.reasons.clone();
    Skill {
        preference_id: id.clone(),
        id,
        name,
        description,
        provider,
        scope,
        skill_dir,
        file_path,
        body,
        raw_frontmatter,
        bundled_files: Vec::new(),
        compatibility,
        compatibility_signals,
        symlinked: false,
        plugin_slug: None,
        provenance: SkillProvenance::ProviderCatalog,
        readable,
        source_enabled: enabled,
        projections,
    }
}

fn merge_catalog(existing: &mut Vec<Skill>, catalog: Vec<Skill>) {
    for catalog_skill in catalog {
        if let Some(readable) = existing.iter_mut().find(|skill| {
            !catalog_skill.file_path.is_empty() && skill.file_path == catalog_skill.file_path
        }) {
            readable.provenance = SkillProvenance::ProviderCatalog;
            if let Some(catalog_projection) = catalog_skill
                .projections
                .into_iter()
                .find(|projection| projection.target_provider == catalog_skill.provider)
            {
                if let Some(existing_projection) =
                    readable.projections.iter_mut().find(|projection| {
                        projection.target_provider == catalog_projection.target_provider
                    })
                {
                    *existing_projection = catalog_projection;
                } else {
                    readable.projections.push(catalog_projection);
                }
            }
            if readable.description.is_none() {
                readable.description = catalog_skill.description;
            }
            continue;
        }
        existing.push(catalog_skill);
    }
}

fn sort_skills(skills: &mut [Skill]) {
    skills.sort_by(|a, b| {
        provider_rank(a.provider)
            .cmp(&provider_rank(b.provider))
            .then(scope_rank(a.scope).cmp(&scope_rank(b.scope)))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then(a.id.cmp(&b.id))
    });
}

fn provider_rank(provider: SkillProvider) -> u8 {
    match provider {
        SkillProvider::Claude => 0,
        SkillProvider::Codex => 1,
        SkillProvider::Opencode => 2,
        SkillProvider::Codemux => 3,
    }
}

fn scope_rank(scope: SkillScope) -> u8 {
    match scope {
        SkillScope::Project => 0,
        SkillScope::User => 1,
        SkillScope::Plugin => 2,
        SkillScope::Configured => 3,
        SkillScope::Managed => 4,
        SkillScope::Admin => 5,
        SkillScope::System => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn codex_fixture_is_tolerantly_decoded() {
        let (skills, errors) = parse_codex_catalog(json!({
            "data": [{
                "name": "deploy",
                "description": "Ship it",
                "path": "codex://built-in/deploy",
                "scope": "system",
                "enabled": true
            }]
        }));
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "deploy");
        assert_eq!(skills[0].scope, SkillScope::System);
        assert!(!skills[0].readable);
        assert!(errors.is_empty());
        assert!(skills[0].projections.iter().any(|projection| {
            projection.target_provider == SkillProvider::Codex
                && projection.availability == SkillAvailability::NativeOnly
                && projection.invocation == SkillInvocationKind::None
        }));
    }

    #[test]
    fn codex_cwd_group_fixture_is_decoded() {
        let (skills, errors) = parse_codex_catalog(json!({
            "data": [{
                "cwd": "/repo",
                "skills": [{
                    "name": "review",
                    "description": "Review changes",
                    "path": "/repo/.agents/skills/review/SKILL.md",
                    "scope": "repo",
                    "enabled": true
                }],
                "errors": [{"message": "broken configured root"}]
            }]
        }));
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "review");
        assert_eq!(skills[0].scope, SkillScope::Project);
        assert_eq!(errors, vec!["broken configured root"]);
    }

    #[test]
    fn catalog_path_is_portable_only_when_body_can_be_read() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join("review");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: review\ndescription: Review\nallowed-tools: [Bash]\n---\nRead this body.",
        )
        .unwrap();

        let skill = catalog_skill(
            SkillProvider::Codex,
            "review".into(),
            None,
            skill_dir.to_string_lossy().to_string(),
            None,
            true,
            SkillScope::User,
        );
        assert!(skill.readable);
        assert_eq!(skill.body, "Read this body.");
        assert!(skill.file_path.ends_with("review/SKILL.md"));
        assert!(skill.projections.iter().any(|projection| {
            projection.target_provider == SkillProvider::Claude
                && projection.availability == SkillAvailability::ExplicitPortable
                && projection.compatibility == crate::skills::SkillCompatibility::HardWarn
        }));

        let missing = catalog_skill(
            SkillProvider::Codex,
            "missing".into(),
            None,
            temp.path().join("missing").to_string_lossy().to_string(),
            None,
            true,
            SkillScope::User,
        );
        assert!(!missing.readable);
        assert!(missing.projections.iter().any(|projection| {
            projection.target_provider == SkillProvider::Claude
                && projection.availability == SkillAvailability::Unavailable
        }));
    }

    #[test]
    fn filesystem_inventory_walks_cwd_ancestors() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join(".claude/skills/ancestor-only");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: ancestor-only\ndescription: ancestor\n---\nUse it.",
        )
        .unwrap();
        let cwd = temp.path().join("packages/app/src");
        fs::create_dir_all(&cwd).unwrap();

        let inventory = filesystem_inventory(Some(&cwd), false);
        assert!(inventory
            .skills
            .iter()
            .any(|skill| skill.name == "ancestor-only"));
    }

    #[test]
    fn duplicate_native_names_fall_back_to_exact_readable_content() {
        let temp = tempfile::tempdir().unwrap();
        let first_dir = temp.path().join("user/review");
        let second_dir = temp.path().join("project/review");
        for directory in [&first_dir, &second_dir] {
            fs::create_dir_all(directory).unwrap();
            fs::write(
                directory.join("SKILL.md"),
                "---\nname: review\ndescription: Review\n---\nUse this exact definition.",
            )
            .unwrap();
        }
        let first = catalog_skill(
            SkillProvider::Claude,
            "review".into(),
            None,
            first_dir.to_string_lossy().to_string(),
            None,
            true,
            SkillScope::User,
        );
        let second = catalog_skill(
            SkillProvider::Claude,
            "review".into(),
            None,
            second_dir.to_string_lossy().to_string(),
            None,
            true,
            SkillScope::Project,
        );
        let inventory = vec![first.clone(), second];
        let projection = first
            .projections
            .iter()
            .find(|projection| projection.target_provider == SkillProvider::Claude)
            .unwrap();

        assert_eq!(
            exact_invocation_kind(&first, projection, &inventory).unwrap(),
            SkillInvocationKind::PromptPrefix
        );
    }

    #[test]
    fn catalog_ownership_replaces_filesystem_provider_for_same_path() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join(".claude/skills/review");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: review\ndescription: Review\n---\nReview carefully.",
        )
        .unwrap();
        let mut existing = scan_directory(
            &temp.path().join(".claude/skills"),
            SkillProvider::Claude,
            SkillScope::Project,
            None,
        );
        let catalog = catalog_skill(
            SkillProvider::Opencode,
            "review".into(),
            Some("OpenCode winner".into()),
            skill_dir.to_string_lossy().to_string(),
            Some("Review carefully.".into()),
            true,
            SkillScope::Configured,
        );

        merge_catalog(&mut existing, vec![catalog]);

        assert_eq!(existing.len(), 1);
        assert_eq!(existing[0].provider, SkillProvider::Claude);
        assert_eq!(existing[0].scope, SkillScope::Project);
        assert!(existing[0].projections.iter().any(|projection| {
            projection.target_provider == SkillProvider::Opencode
                && projection.availability == SkillAvailability::Native
        }));
    }

    #[test]
    fn one_readable_path_can_be_native_in_multiple_catalogs() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join(".agents/skills/review");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: review\ndescription: Review\n---\nReview carefully.",
        )
        .unwrap();
        let mut existing = scan_directory(
            &temp.path().join(".agents/skills"),
            SkillProvider::Codex,
            SkillScope::Project,
            None,
        );
        let codex = catalog_skill(
            SkillProvider::Codex,
            "review".into(),
            None,
            skill_dir.to_string_lossy().to_string(),
            None,
            true,
            SkillScope::Project,
        );
        let opencode = catalog_skill(
            SkillProvider::Opencode,
            "review".into(),
            None,
            skill_dir.to_string_lossy().to_string(),
            Some("Review carefully.".into()),
            true,
            SkillScope::Configured,
        );

        merge_catalog(&mut existing, vec![codex]);
        merge_catalog(&mut existing, vec![opencode]);

        for provider in [SkillProvider::Codex, SkillProvider::Opencode] {
            assert!(existing[0].projections.iter().any(|projection| {
                projection.target_provider == provider
                    && projection.availability == SkillAvailability::Native
            }));
        }
    }

    #[test]
    fn project_preference_identity_is_stable_across_worktrees() {
        let canonical = Path::new("/repos/app");
        let main = project_preference_id(
            Path::new("/repos/app/.agents/skills/review/SKILL.md"),
            Path::new("/repos/app"),
            canonical,
        );
        let linked = project_preference_id(
            Path::new("/worktrees/app-review/.agents/skills/review/SKILL.md"),
            Path::new("/worktrees/app-review"),
            canonical,
        );
        assert_eq!(main, linked);
    }
}
