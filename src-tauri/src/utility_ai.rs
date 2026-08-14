//! Small, isolated text-generation runner shared by non-agentic Codemux
//! features. Unlike the merge resolver this path never grants workspace
//! writes: conversation text is untrusted input and summarisation must not be
//! able to turn an old instruction into a shell/edit action.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

const UTILITY_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UtilityModelSelection {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub effort: Option<String>,
}

#[derive(Debug)]
struct UtilityInvocation {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    output_file: Option<PathBuf>,
}

/// Every provider receives the prompt on stdin, so the argv stays small
/// enough for Windows' ~32k command-line limit regardless of transcript size.
fn build_invocation(
    selection: &UtilityModelSelection,
    temp_dir: &Path,
) -> Result<UtilityInvocation, String> {
    if selection.model.trim().is_empty() {
        return Err("utility_model_required".into());
    }
    match selection.provider.as_str() {
        "codex" => {
            let output_file = temp_dir.join("last-message.md");
            let mut args = vec![
                "exec".into(),
                "--ephemeral".into(),
                "--skip-git-repo-check".into(),
                "--ignore-user-config".into(),
                "--ignore-rules".into(),
                "--sandbox".into(),
                "read-only".into(),
                "--color".into(),
                "never".into(),
                "--model".into(),
                selection.model.clone(),
            ];
            if let Some(effort) = selection.effort.as_deref().filter(|v| !v.is_empty()) {
                args.extend([
                    "--config".into(),
                    format!("model_reasoning_effort=\"{effort}\""),
                ]);
            }
            args.extend([
                "--output-last-message".into(),
                output_file.to_string_lossy().to_string(),
                "-".into(),
            ]);
            Ok(UtilityInvocation {
                program: "codex".into(),
                args,
                env: Vec::new(),
                output_file: Some(output_file),
            })
        }
        "claude" => {
            let mut args = vec![
                "--print".into(),
                "--no-session-persistence".into(),
                "--output-format".into(),
                "text".into(),
                "--strict-mcp-config".into(),
                "--tools".into(),
                "".into(),
                "--model".into(),
                selection.model.clone(),
            ];
            if let Some(effort) = selection.effort.as_deref().filter(|v| !v.is_empty()) {
                args.extend(["--effort".into(), effort.into()]);
            }
            Ok(UtilityInvocation {
                program: "claude".into(),
                args,
                env: Vec::new(),
                output_file: None,
            })
        }
        "opencode" => {
            let mut args = vec![
                "run".into(),
                "--pure".into(),
                "--format".into(),
                "default".into(),
                "--model".into(),
                selection.model.clone(),
                "--dir".into(),
                temp_dir.to_string_lossy().to_string(),
            ];
            if let Some(effort) = selection.effort.as_deref().filter(|v| !v.is_empty()) {
                args.extend(["--variant".into(), effort.into()]);
            }
            // `opencode run` reads a piped stdin as the message when no
            // positional message is given. Passing the transcript on argv
            // instead would blow past Windows' ~32k CreateProcess limit for
            // any conversation worth summarising.
            //
            // `--pure` removes plugins. The ephemeral config also denies
            // every tool/permission so a transcript cannot ask OpenCode to
            // inspect or mutate the machine while it is being summarized.
            Ok(UtilityInvocation {
                program: "opencode".into(),
                args,
                env: vec![(
                    "OPENCODE_CONFIG_CONTENT".into(),
                    r#"{"permission":{"*":"deny"},"tools":{"*":false},"mcp":{}}"#.into(),
                )],
                output_file: None,
            })
        }
        _ => Err(format!(
            "utility_provider_unsupported: {}",
            selection.provider
        )),
    }
}

fn strip_ansi(value: &str) -> String {
    let mut clean = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        } else {
            clean.push(ch);
        }
    }
    clean
}

pub async fn generate_utility_text(
    selection: &UtilityModelSelection,
    prompt: &str,
) -> Result<String, String> {
    let temp = tempfile::tempdir().map_err(|e| format!("utility_temp_dir_failed: {e}"))?;
    let invocation = build_invocation(selection, temp.path())?;
    let mut command = crate::execution::host_command_tokio(&invocation.program);
    command
        .args(&invocation.args)
        .current_dir(temp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in &invocation.env {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("utility_spawn_failed: {}: {e}", selection.provider))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "utility_stdin_unavailable".to_string())?;
    stdin
        .write_all(prompt.as_bytes())
        .await
        .map_err(|e| format!("utility_prompt_write_failed: {e}"))?;
    drop(stdin);
    let output = tokio::time::timeout(UTILITY_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| "utility_generation_timeout".to_string())?
        .map_err(|e| format!("utility_generation_io_failed: {e}"))?;
    if !output.status.success() {
        let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));
        let detail = stderr.trim().chars().take(600).collect::<String>();
        return Err(format!(
            "utility_generation_failed: {}{}",
            selection.provider,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ));
    }
    let raw = if let Some(path) = invocation.output_file {
        tokio::fs::read_to_string(path)
            .await
            .map_err(|e| format!("utility_output_read_failed: {e}"))?
    } else {
        String::from_utf8_lossy(&output.stdout).to_string()
    };
    let text = strip_ansi(&raw).trim().to_string();
    if text.is_empty() {
        Err("utility_generation_empty".into())
    } else {
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(provider: &str, model: &str, effort: Option<&str>) -> UtilityModelSelection {
        UtilityModelSelection {
            provider: provider.into(),
            model: model.into(),
            effort: effort.map(str::to_string),
        }
    }

    #[test]
    fn codex_utility_is_ephemeral_and_read_only() {
        let inv = build_invocation(
            &selection("codex", "gpt-5.6-luna", Some("low")),
            Path::new("/tmp/codemux-utility-test"),
        )
        .unwrap();
        assert!(inv.args.iter().any(|a| a == "--ephemeral"));
        assert!(inv.args.windows(2).any(|a| a == ["--sandbox", "read-only"]));
        assert!(!inv.args.iter().any(|a| a.contains("dangerously")));
    }

    #[test]
    fn claude_utility_disables_tools_and_persistence() {
        let inv = build_invocation(
            &selection("claude", "claude-haiku-4-5", None),
            Path::new("/tmp/codemux-utility-test"),
        )
        .unwrap();
        assert!(inv.args.iter().any(|a| a == "--no-session-persistence"));
        assert!(inv.args.windows(2).any(|a| a == ["--tools", ""]));
        assert!(!inv.args.iter().any(|a| a.contains("dangerously")));
    }

    #[test]
    fn opencode_utility_is_pure_and_denies_tools() {
        let inv = build_invocation(
            &selection("opencode", "openai/gpt-5", Some("low")),
            Path::new("/tmp/codemux-utility-test"),
        )
        .unwrap();
        assert!(inv.args.iter().any(|a| a == "--pure"));
        assert!(inv.env.iter().any(|(_, value)| value.contains("deny")));
        assert!(!inv.args.iter().any(|a| a == "--auto"));
        // No positional message: `opencode run` takes it from stdin instead.
        // On argv a summary chunk would blow past Windows' ~32k limit.
        assert_eq!(inv.args.last().map(String::as_str), Some("low"));
    }
}
