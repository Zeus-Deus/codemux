//! Session adapter system for tool-specific resume behavior.
//!
//! Adapters are defined in `~/.config/codemux/session-adapters.toml`. Each adapter
//! describes how to detect a CLI tool, what metadata to capture from its output,
//! and how to construct a resume command from captured metadata.
//!
//! The core restore logic never hardcodes tool-specific knowledge — everything
//! goes through adapters.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// ── Config Types ───────────────────────────────────────────────

/// A single capture pattern that extracts metadata from terminal output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturePattern {
    pub key: String,
    pub pattern: String,
}

/// A session adapter definition loaded from TOML config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterConfig {
    /// Regex matched against the original spawn command.
    pub detect_pattern: String,
    /// Patterns to capture from terminal output during the session.
    #[serde(default)]
    pub capture: Vec<CapturePattern>,
    /// Args to append to the preset command for resume. Uses `{key}` placeholders.
    #[serde(default)]
    pub resume_args: Option<String>,
    /// Button label shown in the restored pane.
    #[serde(default)]
    pub resume_label: Option<String>,
    /// Optional shell command to validate resume is possible. Uses `{key}` placeholders.
    #[serde(default)]
    pub validate: Option<String>,
    /// Fallback args when `resume_args` has unresolved `{key}` placeholders (missing captures).
    #[serde(default)]
    pub fallback_resume_args: Option<String>,
}

/// Bump this when the default adapter config changes so stale files get updated.
const ADAPTERS_CONFIG_VERSION: u32 = 3;

/// Top-level TOML structure.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AdaptersFile {
    #[serde(default)]
    pub config_version: u32,
    #[serde(default)]
    pub adapters: HashMap<String, AdapterConfig>,
}

// ── Runtime Types ──────────────────────────────────────────────

/// Runtime state for an active output scanner on a terminal session.
#[derive(Debug, Clone)]
pub struct OutputScanner {
    pub adapter_id: String,
    pub patterns: Vec<CompiledCapture>,
    pub captures: HashMap<String, String>,
    pub lines_scanned: usize,
    pub started_at: std::time::Instant,
    pub complete: bool,
}

#[derive(Debug, Clone)]
pub struct CompiledCapture {
    pub key: String,
    pub regex: Regex,
}

/// Result from adapter matching + validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterMatch {
    pub adapter_id: String,
    pub resume_args: Option<String>,
    pub resume_label: Option<String>,
    pub validate_command: Option<String>,
    pub captures: HashMap<String, String>,
}

/// Shared state for all active scanners, keyed by session_id.
#[derive(Default, Clone)]
pub struct AdapterState {
    scanners: Arc<Mutex<HashMap<String, OutputScanner>>>,
    pub config: Arc<Mutex<AdaptersFile>>,
}

// ── Constants ──────────────────────────────────────────────────

/// Stop scanning after this many lines of output.
const MAX_SCAN_LINES: usize = 200;
/// Stop scanning after this many seconds.
const MAX_SCAN_SECONDS: u64 = 60;

// ── Config Loading ─────────────────────────────────────────────

fn config_path() -> PathBuf {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"));
    config_dir.join("codemux").join("session-adapters.toml")
}

/// Default adapter config shipped with Codemux.
fn default_config() -> AdaptersFile {
    let mut adapters = HashMap::new();

    adapters.insert(
        "claude-code".into(),
        AdapterConfig {
            detect_pattern: "claude".into(),
            // Session ID captured via hooks (stdin JSON), not output scanning.
            capture: vec![],
            resume_args: Some("--resume {claude_session_id}".into()),
            resume_label: Some("Resume Claude Code session".into()),
            validate: None,
            // When no session ID was captured (hooks didn't fire, jq missing, etc.),
            // fall back to --continue which resumes the most recent session in the CWD.
            fallback_resume_args: Some("--continue".into()),
        },
    );

    AdaptersFile {
        config_version: ADAPTERS_CONFIG_VERSION,
        adapters,
    }
}

/// Load adapters from disk. If the file doesn't exist, create it with defaults.
/// If the config_version is older than the current, re-apply defaults for builtin
/// adapters (user-added adapters are preserved).
pub fn load_adapters() -> AdaptersFile {
    let path = config_path();

    if !path.exists() {
        let defaults = default_config();
        let _ = write_config(&defaults);
        return defaults;
    }

    match fs::read_to_string(&path) {
        Ok(contents) => match toml::from_str::<AdaptersFile>(&contents) {
            Ok(mut user_config) => {
                let defaults = default_config();
                let mut changed = false;

                // If config version is older, re-apply builtin adapter defaults.
                // This handles cases where the default adapter config changed
                // (e.g., claude-code switching from --resume to --continue).
                if user_config.config_version < ADAPTERS_CONFIG_VERSION {
                    eprintln!(
                        "[codemux::session_adapters] Upgrading config from v{} to v{}",
                        user_config.config_version, ADAPTERS_CONFIG_VERSION
                    );
                    for (id, adapter) in &defaults.adapters {
                        user_config.adapters.insert(id.clone(), adapter.clone());
                    }
                    user_config.config_version = ADAPTERS_CONFIG_VERSION;
                    changed = true;
                } else {
                    // Just add new defaults that the user hasn't overridden
                    for (id, adapter) in defaults.adapters {
                        if !user_config.adapters.contains_key(&id) {
                            user_config.adapters.insert(id, adapter);
                            changed = true;
                        }
                    }
                }

                if changed {
                    let _ = write_config(&user_config);
                }
                user_config
            }
            Err(e) => {
                eprintln!(
                    "[codemux::session_adapters] Failed to parse {}: {e}",
                    path.display()
                );
                default_config()
            }
        },
        Err(e) => {
            eprintln!(
                "[codemux::session_adapters] Failed to read {}: {e}",
                path.display()
            );
            default_config()
        }
    }
}

fn write_config(config: &AdaptersFile) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    let toml_str =
        toml::to_string_pretty(config).map_err(|e| format!("TOML serialize: {e}"))?;

    let header = "# Codemux Session Adapters\n\
                  # Define how CLI tools can be detected and resumed after app restart.\n\
                  # See: docs/features/session-persistence.md\n\n";

    fs::write(&path, format!("{header}{toml_str}"))
        .map_err(|e| format!("write: {e}"))?;
    Ok(())
}

// ── AdapterState Implementation ────────────────────────────────

impl AdapterState {
    pub fn new() -> Self {
        let config = load_adapters();
        Self {
            scanners: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(config)),
        }
    }

    /// Try to match a command against loaded adapters and start a scanner.
    /// Returns the adapter ID if matched, None otherwise.
    pub fn start_scanner(&self, session_id: &str, command: &str) -> Option<String> {
        let config = self.config.lock().unwrap();

        for (id, adapter) in &config.adapters {
            let Ok(re) = Regex::new(&adapter.detect_pattern) else {
                continue;
            };
            if !re.is_match(command) {
                continue;
            }

            // Compile capture patterns
            let patterns: Vec<CompiledCapture> = adapter
                .capture
                .iter()
                .filter_map(|cap| {
                    Regex::new(&cap.pattern).ok().map(|regex| CompiledCapture {
                        key: cap.key.clone(),
                        regex,
                    })
                })
                .collect();

            let scanner = OutputScanner {
                adapter_id: id.clone(),
                patterns,
                captures: HashMap::new(),
                lines_scanned: 0,
                started_at: std::time::Instant::now(),
                complete: false,
            };

            self.scanners
                .lock()
                .unwrap()
                .insert(session_id.to_string(), scanner);

            return Some(id.clone());
        }

        None
    }

    /// Find the first adapter whose detect_pattern matches a command string.
    pub fn match_adapter_id_for_command(&self, command: &str) -> Option<String> {
        let config = self.config.lock().unwrap();

        for (id, adapter) in &config.adapters {
            let Ok(re) = Regex::new(&adapter.detect_pattern) else {
                continue;
            };
            if re.is_match(command) {
                return Some(id.clone());
            }
        }

        None
    }

    /// Feed a line of terminal output to the scanner for the given session.
    /// Returns true if the scanner captured something new.
    pub fn scan_line(&self, session_id: &str, line: &str) -> bool {
        let mut scanners = self.scanners.lock().unwrap();
        let Some(scanner) = scanners.get_mut(session_id) else {
            return false;
        };

        if scanner.complete {
            return false;
        }

        scanner.lines_scanned += 1;

        // Check time and line limits
        if scanner.lines_scanned > MAX_SCAN_LINES
            || scanner.started_at.elapsed().as_secs() > MAX_SCAN_SECONDS
        {
            scanner.complete = true;
            return false;
        }

        let mut captured_new = false;

        for pattern in &scanner.patterns {
            if scanner.captures.contains_key(&pattern.key) {
                continue; // Already captured
            }
            if let Some(caps) = pattern.regex.captures(line) {
                if let Some(m) = caps.get(1) {
                    scanner
                        .captures
                        .insert(pattern.key.clone(), m.as_str().to_string());
                    captured_new = true;
                }
            }
        }

        // Check if all patterns are captured
        if scanner
            .patterns
            .iter()
            .all(|p| scanner.captures.contains_key(&p.key))
        {
            scanner.complete = true;
        }

        captured_new
    }

    /// Get the current captures for a session's scanner.
    pub fn get_captures(&self, session_id: &str) -> Option<(String, HashMap<String, String>)> {
        let scanners = self.scanners.lock().unwrap();
        scanners
            .get(session_id)
            .map(|s| (s.adapter_id.clone(), s.captures.clone()))
    }

    /// Remove the scanner for a session (e.g. when the session is closed).
    pub fn remove_scanner(&self, session_id: &str) {
        self.scanners.lock().unwrap().remove(session_id);
    }

    /// Build a resume command from an adapter's resume_args template and captured values.
    pub fn build_resume_command(
        &self,
        adapter_id: &str,
        captures: &HashMap<String, String>,
    ) -> Option<String> {
        let config = self.config.lock().unwrap();
        let adapter = config.adapters.get(adapter_id)?;
        let template = adapter.resume_args.as_ref()?;
        Some(substitute_placeholders(template, captures))
    }

    /// Get adapter match info for a restored pane, using saved metadata.
    /// If `resume_args` has unresolved `{key}` placeholders after substitution,
    /// falls back to `fallback_resume_args`.
    pub fn get_adapter_match(
        &self,
        adapter_id: &str,
        captures: &HashMap<String, String>,
    ) -> Option<AdapterMatch> {
        let config = self.config.lock().unwrap();
        let adapter = config.adapters.get(adapter_id)?;

        let resume_args = adapter.resume_args.as_ref().map(|t| {
            let resolved = substitute_placeholders(t, captures);
            // If resolved string still contains {placeholders}, use fallback
            if has_unresolved_placeholders(&resolved) {
                adapter
                    .fallback_resume_args
                    .as_ref()
                    .map(|fb| substitute_placeholders(fb, captures))
                    .unwrap_or(resolved)
            } else {
                resolved
            }
        });

        Some(AdapterMatch {
            adapter_id: adapter_id.to_string(),
            resume_args,
            resume_label: adapter.resume_label.clone(),
            validate_command: adapter
                .validate
                .as_ref()
                .map(|t| substitute_placeholders(t, captures)),
            captures: captures.clone(),
        })
    }
}

/// Replace `{key}` placeholders in a template with captured values.
fn substitute_placeholders(template: &str, captures: &HashMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in captures {
        result = result.replace(&format!("{{{key}}}"), value);
    }
    result
}

/// Check if a string still contains unresolved `{key}` placeholders.
fn has_unresolved_placeholders(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = bytes[i..].iter().position(|&b| b == b'}') {
                let inner = &s[i + 1..i + end];
                // Must be a valid identifier (alphanumeric + underscore)
                if !inner.is_empty() && inner.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    return true;
                }
                i += end + 1;
            } else {
                break;
            }
        } else {
            i += 1;
        }
    }
    false
}

// ── Tauri Commands ─────────────────────────────────────────────

/// Validate whether a resume is possible by running the adapter's validate command.
#[tauri::command]
pub async fn validate_resume(
    adapter_id: String,
    captures: HashMap<String, String>,
    adapter_state: tauri::State<'_, AdapterState>,
) -> Result<bool, String> {
    let config = adapter_state.config.lock().unwrap().clone();
    let Some(adapter) = config.adapters.get(&adapter_id) else {
        return Ok(false);
    };
    let Some(validate_template) = &adapter.validate else {
        // No validation command — assume resume is possible
        return Ok(true);
    };

    let validate_cmd = substitute_placeholders(validate_template, &captures);

    // Timeout after 5 seconds — a hanging validate command must not block
    // the entire restore flow.
    let child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&validate_cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn validate command: {e}"))?;

    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(output)) => Ok(output.status.success()),
        Ok(Err(e)) => {
            eprintln!("[codemux::session_adapters] validate command error: {e}");
            Ok(false)
        }
        Err(_) => {
            eprintln!("[codemux::session_adapters] validate command timed out after 5s");
            Ok(false) // Timeout → assume resume not possible
        }
    }
}

/// Get the adapter match info for a restored pane.
#[tauri::command]
pub fn get_adapter_info(
    adapter_id: String,
    captures: HashMap<String, String>,
    adapter_state: tauri::State<'_, AdapterState>,
) -> Result<Option<AdapterMatch>, String> {
    Ok(adapter_state.get_adapter_match(&adapter_id, &captures))
}

/// Get the current scanner captures for a live session.
/// Returns (adapter_id, captures) or null if no scanner is active.
#[tauri::command]
pub fn get_scanner_captures(
    session_id: String,
    adapter_state: tauri::State<'_, AdapterState>,
) -> Result<Option<(String, HashMap<String, String>)>, String> {
    Ok(adapter_state.get_captures(&session_id))
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let config = default_config();
        assert!(config.adapters.contains_key("claude-code"));
        let claude = &config.adapters["claude-code"];
        assert_eq!(claude.detect_pattern, "claude");
        assert!(claude.capture.is_empty());
        assert_eq!(
            claude.resume_args.as_deref(),
            Some("--resume {claude_session_id}")
        );
        assert_eq!(claude.fallback_resume_args.as_deref(), Some("--continue"));
    }

    #[test]
    fn toml_roundtrip() {
        let config = default_config();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let back: AdaptersFile = toml::from_str(&toml_str).unwrap();
        assert!(back.adapters.contains_key("claude-code"));
    }

    #[test]
    fn toml_parse_with_unknown_fields() {
        let toml_str = r#"
            [adapters.my-tool]
            detect_pattern = "mytool"
            resume_args = "--continue"
            resume_label = "Resume my tool"
            some_future_field = "ignored"
        "#;
        // Should not fail — unknown fields are ignored by serde default
        let result: Result<AdaptersFile, _> = toml::from_str(toml_str);
        // toml with deny_unknown_fields would fail, but we don't use that
        assert!(result.is_ok());
    }

    #[test]
    fn adapter_matching() {
        let state = AdapterState {
            scanners: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(default_config())),
        };

        // Should match
        let matched = state.start_scanner("sess-1", "claude --dangerously-skip-permissions");
        assert_eq!(matched, Some("claude-code".into()));

        // Should not match
        let not_matched = state.start_scanner("sess-2", "vim foo.rs");
        assert!(not_matched.is_none());
    }

    #[test]
    fn scanner_captures_from_output() {
        // Use a custom adapter with capture patterns to test the scanning mechanism.
        let mut adapters = HashMap::new();
        adapters.insert(
            "my-agent".into(),
            AdapterConfig {
                detect_pattern: "my-agent".into(),
                capture: vec![CapturePattern {
                    key: "run_id".into(),
                    pattern: r"Run started: ([A-Z0-9-]+)".into(),
                }],
                resume_args: Some("--continue {run_id}".into()),
                resume_label: None,
                validate: None,
                fallback_resume_args: None,
            },
        );

        let state = AdapterState {
            scanners: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(AdaptersFile { config_version: 0, adapters })),
        };

        state.start_scanner("sess-1", "my-agent --auto");

        assert!(!state.scan_line("sess-1", "Starting my-agent..."));
        assert!(!state.scan_line("sess-1", "Loading context..."));
        assert!(state.scan_line("sess-1", "Run started: ABC-12345"));

        let (adapter_id, captures) = state.get_captures("sess-1").unwrap();
        assert_eq!(adapter_id, "my-agent");
        assert_eq!(captures.get("run_id").unwrap(), "ABC-12345");
    }

    #[test]
    fn claude_adapter_matches_without_captures() {
        // Claude Code adapter uses --continue with no output capture.
        let state = AdapterState {
            scanners: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(default_config())),
        };

        let matched = state.start_scanner("sess-1", "claude --dangerously-skip-permissions");
        assert_eq!(matched, Some("claude-code".into()));

        // No captures expected — scanner completes immediately (0 patterns)
        let (adapter_id, captures) = state.get_captures("sess-1").unwrap();
        assert_eq!(adapter_id, "claude-code");
        assert!(captures.is_empty());
    }

    #[test]
    fn scanner_stops_after_max_lines() {
        let state = AdapterState {
            scanners: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(default_config())),
        };

        state.start_scanner("sess-1", "claude");

        for i in 0..MAX_SCAN_LINES + 50 {
            state.scan_line("sess-1", &format!("line {i}"));
        }

        // Scanner should be complete even without capturing
        let scanners = state.scanners.lock().unwrap();
        assert!(scanners["sess-1"].complete);
    }

    #[test]
    fn substitute_placeholders_works() {
        let mut captures = HashMap::new();
        captures.insert("session_id".into(), "abc-123".into());
        captures.insert("run_id".into(), "run-456".into());

        let result = substitute_placeholders("--resume {session_id} --run {run_id}", &captures);
        assert_eq!(result, "--resume abc-123 --run run-456");
    }

    #[test]
    fn build_resume_command_works() {
        let state = AdapterState {
            scanners: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(default_config())),
        };

        // With session ID captured → uses --resume
        let mut caps = HashMap::new();
        caps.insert("claude_session_id".into(), "abc-123".into());
        let cmd = state.build_resume_command("claude-code", &caps).unwrap();
        assert_eq!(cmd, "--resume abc-123");

        // Without session ID → raw template (fallback handled by get_adapter_match)
        let cmd = state
            .build_resume_command("claude-code", &HashMap::new())
            .unwrap();
        assert_eq!(cmd, "--resume {claude_session_id}");
    }

    #[test]
    fn get_adapter_match_falls_back_when_captures_missing() {
        let state = AdapterState {
            scanners: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(default_config())),
        };

        // No captures → fallback_resume_args used (--continue)
        let m = state
            .get_adapter_match("claude-code", &HashMap::new())
            .unwrap();
        assert_eq!(m.adapter_id, "claude-code");
        assert_eq!(m.resume_args.as_deref(), Some("--continue"));

        // With captures → exact --resume {id}
        let mut caps = HashMap::new();
        caps.insert("claude_session_id".into(), "abc-123".into());
        let m = state.get_adapter_match("claude-code", &caps).unwrap();
        assert_eq!(m.resume_args.as_deref(), Some("--resume abc-123"));

        // Unknown adapter
        assert!(state.get_adapter_match("unknown", &HashMap::new()).is_none());
    }

    #[test]
    fn two_adapters_match_first_wins() {
        // When two adapters both match, the first one found is used.
        // HashMap iteration order is non-deterministic, so both are valid.
        // The key assertion: exactly one scanner is created, not two.
        let mut adapters = HashMap::new();
        adapters.insert(
            "tool-a".into(),
            AdapterConfig {
                detect_pattern: "mytool".into(),
                capture: vec![],
                resume_args: Some("--resume-a".into()),
                resume_label: None,
                validate: None,
                fallback_resume_args: None,
            },
        );
        adapters.insert(
            "tool-b".into(),
            AdapterConfig {
                detect_pattern: "mytool".into(),
                capture: vec![],
                resume_args: Some("--resume-b".into()),
                resume_label: None,
                validate: None,
                fallback_resume_args: None,
            },
        );

        let state = AdapterState {
            scanners: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(AdaptersFile { config_version: 0, adapters })),
        };

        let matched = state.start_scanner("sess-1", "mytool --flag");
        assert!(matched.is_some()); // one of them matched
        // Exactly one scanner was created
        assert_eq!(state.scanners.lock().unwrap().len(), 1);
    }

    #[test]
    fn no_scanner_for_unmatched_command() {
        let state = AdapterState {
            scanners: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(default_config())),
        };

        // Plain shell — should not match any adapter
        let matched = state.start_scanner("sess-1", "bash");
        assert!(matched.is_none());

        // scan_line should be a no-op (no scanner registered)
        assert!(!state.scan_line("sess-1", "some output"));
        assert!(state.get_captures("sess-1").is_none());
    }

    #[test]
    fn invalid_detect_regex_skips_adapter() {
        let mut adapters = HashMap::new();
        adapters.insert(
            "broken".into(),
            AdapterConfig {
                detect_pattern: "([unclosed".into(), // invalid regex
                capture: vec![],
                resume_args: None,
                resume_label: None,
                validate: None,
                fallback_resume_args: None,
            },
        );
        adapters.insert(
            "working".into(),
            AdapterConfig {
                detect_pattern: "mytool".into(),
                capture: vec![],
                resume_args: Some("--resume".into()),
                resume_label: None,
                validate: None,
                fallback_resume_args: None,
            },
        );

        let state = AdapterState {
            scanners: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(AdaptersFile { config_version: 0, adapters })),
        };

        // The broken adapter is skipped; the working one matches
        let matched = state.start_scanner("sess-1", "mytool");
        assert_eq!(matched, Some("working".into()));
    }

    #[test]
    fn invalid_capture_regex_skipped_gracefully() {
        let mut adapters = HashMap::new();
        adapters.insert(
            "tool".into(),
            AdapterConfig {
                detect_pattern: "test".into(),
                capture: vec![
                    CapturePattern {
                        key: "bad".into(),
                        pattern: "([unclosed".into(), // invalid
                    },
                    CapturePattern {
                        key: "good".into(),
                        pattern: r"ID: (\w+)".into(), // valid
                    },
                ],
                resume_args: Some("--id {good}".into()),
                resume_label: None,
                validate: None,
                fallback_resume_args: None,
            },
        );

        let state = AdapterState {
            scanners: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(AdaptersFile { config_version: 0, adapters })),
        };

        state.start_scanner("sess-1", "test");
        // Only the valid pattern should be compiled
        let scanners = state.scanners.lock().unwrap();
        assert_eq!(scanners["sess-1"].patterns.len(), 1);
        assert_eq!(scanners["sess-1"].patterns[0].key, "good");
    }

    #[test]
    fn has_unresolved_placeholders_works() {
        assert!(has_unresolved_placeholders("--resume {session_id}"));
        assert!(has_unresolved_placeholders("--a {foo} --b {bar}"));
        assert!(!has_unresolved_placeholders("--resume abc-123"));
        assert!(!has_unresolved_placeholders("--continue"));
        assert!(!has_unresolved_placeholders("")); // empty
        assert!(!has_unresolved_placeholders("{}")); // empty braces
        assert!(!has_unresolved_placeholders("{ spaces }")); // not an identifier
    }

    #[test]
    fn fallback_resume_args_used_when_primary_unresolved() {
        let mut adapters = HashMap::new();
        adapters.insert(
            "tool".into(),
            AdapterConfig {
                detect_pattern: "tool".into(),
                capture: vec![],
                resume_args: Some("--resume {sid}".into()),
                resume_label: None,
                validate: None,
                fallback_resume_args: Some("--continue".into()),
            },
        );

        let state = AdapterState {
            scanners: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(AdaptersFile {
                config_version: 0,
                adapters,
            })),
        };

        // No captures → fallback
        let m = state.get_adapter_match("tool", &HashMap::new()).unwrap();
        assert_eq!(m.resume_args.as_deref(), Some("--continue"));

        // With capture → primary
        let mut caps = HashMap::new();
        caps.insert("sid".into(), "xyz".into());
        let m = state.get_adapter_match("tool", &caps).unwrap();
        assert_eq!(m.resume_args.as_deref(), Some("--resume xyz"));
    }

    #[test]
    fn pane_adapter_captures_stored_per_session() {
        use crate::state::AppStateStore;
        let store = AppStateStore::default();

        // Create a terminal session
        let sid = store.create_terminal_session().unwrap();

        // Store a capture on it
        store.set_terminal_adapter_capture(&sid.0, "claude_session_id", "uuid-aaa");

        // Read it back
        let caps = store.get_terminal_adapter_captures(&sid.0);
        assert_eq!(caps.get("claude_session_id").unwrap(), "uuid-aaa");

        // Different session has empty captures
        let sid2 = store.create_terminal_session().unwrap();
        let caps2 = store.get_terminal_adapter_captures(&sid2.0);
        assert!(caps2.is_empty());
    }
}
