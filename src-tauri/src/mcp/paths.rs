// MCP config path enumeration.
//
// Locked decisions on which files Codemux scans. Existence is filtered
// HERE (unlike skills, which lets the scanner skip silently) because the
// parser hits each path with a JSON read — pre-filtering keeps per-call
// I/O bounded.
//
// Storage locations confirmed against a real Claude Code install:
//
// * `~/.claude.json` — single multi-section JSON file. Top-level
//   `mcpServers` is the user scope (what `claude mcp add --scope user`
//   writes to). `projects.<absolute-path>.mcpServers` is the per-user
//   per-project local scope. The legacy `~/.claude/.mcp.json` path used
//   in Stage 1 does NOT exist on real installs.
// * `<project>/.mcp.json` — checked-in project scope. Codemux's own
//   `upsert_mcp_config` writes here too, so the parser drops entries
//   named `codemux` to avoid double-listing the always-on row.
// * `~/.codemux/mcp.json` and `<project>/.codemux/mcp.json` — Codemux's
//   own canonical paths (Stage 4 writes here when users add servers
//   from the UI).
// * `~/.cursor/mcp.json` and `<project>/.cursor/mcp.json` — Cursor's
//   format is identical to Claude's `mcpServers` shape.

use std::path::{Path, PathBuf};

use super::McpConfigSource;

pub struct McpScanPaths {
    pub paths: Vec<(PathBuf, McpConfigSource)>,
}

/// Enumerate every config path Codemux knows about. Production callers pass
/// `project_root = Some(...)` for the active workspace; tests inject `None`
/// to assert user-only behavior, or pass a fake home via the `_with_home`
/// variant.
pub fn enumerate_mcp_paths(project_root: Option<&Path>) -> McpScanPaths {
    enumerate_mcp_paths_with_home(project_root, dirs::home_dir().as_deref())
}

pub fn enumerate_mcp_paths_with_home(
    project_root: Option<&Path>,
    home: Option<&Path>,
) -> McpScanPaths {
    let mut all: Vec<(PathBuf, McpConfigSource)> = Vec::new();

    if let Some(home) = home {
        // Codemux's own canonical config — the path Stage 4 will write to.
        all.push((home.join(".codemux").join("mcp.json"), McpConfigSource::CodemuxUser));
        // Claude Code multi-section JSON. The parser handles the two
        // sub-scopes (user + per-project local); enumeration tags it as
        // ClaudeUser so the dispatcher can route to `parse_claude_wrapped_config`.
        all.push((home.join(".claude.json"), McpConfigSource::ClaudeUser));
        // Cursor — same shape as Claude's `mcpServers`. Surface only.
        all.push((home.join(".cursor").join("mcp.json"), McpConfigSource::CursorUser));
    }

    if let Some(root) = project_root {
        all.push((root.join(".codemux").join("mcp.json"), McpConfigSource::CodemuxProject));
        // Claude Code project scope is at the repo root, NOT under .claude/.
        all.push((root.join(".mcp.json"), McpConfigSource::ClaudeProject));
        all.push((root.join(".cursor").join("mcp.json"), McpConfigSource::CursorProject));
    }

    // Skip non-existent paths — discovery is silent.
    all.retain(|(p, _)| p.exists());

    McpScanPaths { paths: all }
}

/// True if `path` is the Claude wrapped-config file (`~/.claude.json` or
/// equivalent). The parser dispatch uses this to pick
/// `parse_claude_wrapped_config` instead of the simple `mcpServers`-key
/// reader.
pub fn is_claude_wrapped_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == ".claude.json")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_empty_json(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, "{}").unwrap();
    }

    #[test]
    fn missing_paths_silently_skipped() {
        let home = TempDir::new().unwrap();
        let paths = enumerate_mcp_paths_with_home(None, Some(home.path()));
        assert!(paths.paths.is_empty());
    }

    #[test]
    fn user_paths_present_when_files_exist() {
        let home = TempDir::new().unwrap();
        write_empty_json(&home.path().join(".codemux/mcp.json"));
        write_empty_json(&home.path().join(".claude.json"));
        write_empty_json(&home.path().join(".cursor/mcp.json"));

        let paths = enumerate_mcp_paths_with_home(None, Some(home.path()));
        let sources: Vec<_> = paths.paths.iter().map(|(_, s)| *s).collect();

        assert!(sources.contains(&McpConfigSource::CodemuxUser));
        assert!(sources.contains(&McpConfigSource::ClaudeUser));
        assert!(sources.contains(&McpConfigSource::CursorUser));
        assert_eq!(paths.paths.len(), 3);
    }

    #[test]
    fn project_paths_only_when_root_given() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();

        write_empty_json(&project.path().join(".codemux/mcp.json"));
        write_empty_json(&project.path().join(".mcp.json"));
        write_empty_json(&project.path().join(".cursor/mcp.json"));

        let no_project = enumerate_mcp_paths_with_home(None, Some(home.path()));
        assert!(no_project.paths.is_empty());

        let with_project =
            enumerate_mcp_paths_with_home(Some(project.path()), Some(home.path()));
        assert_eq!(with_project.paths.len(), 3);

        let sources: Vec<_> = with_project.paths.iter().map(|(_, s)| *s).collect();
        assert!(sources.contains(&McpConfigSource::CodemuxProject));
        assert!(sources.contains(&McpConfigSource::ClaudeProject));
        assert!(sources.contains(&McpConfigSource::CursorProject));
    }

    #[test]
    fn user_and_project_paths_coexist() {
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();

        write_empty_json(&home.path().join(".codemux/mcp.json"));
        write_empty_json(&project.path().join(".mcp.json"));

        let paths =
            enumerate_mcp_paths_with_home(Some(project.path()), Some(home.path()));
        assert_eq!(paths.paths.len(), 2);

        let sources: Vec<_> = paths.paths.iter().map(|(_, s)| *s).collect();
        assert!(sources.contains(&McpConfigSource::CodemuxUser));
        assert!(sources.contains(&McpConfigSource::ClaudeProject));
    }

    #[test]
    fn is_claude_wrapped_path_recognizes_dot_claude_json() {
        assert!(is_claude_wrapped_path(Path::new("/home/u/.claude.json")));
        assert!(!is_claude_wrapped_path(Path::new("/home/u/.codemux/mcp.json")));
        assert!(!is_claude_wrapped_path(Path::new("/home/u/.cursor/mcp.json")));
        assert!(!is_claude_wrapped_path(Path::new("/proj/.mcp.json")));
    }
}
