use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct DetectedSetup {
    pub id: String,
    pub label: String,
    pub command: String,
    pub enabled: bool,
}

/// The JS package manager implied by the project's lockfile, if any.
fn js_lockfile_manager(root: &Path) -> Option<&'static str> {
    if root.join("bun.lock").exists() || root.join("bun.lockb").exists() {
        Some("bun")
    } else if root.join("pnpm-lock.yaml").exists() {
        Some("pnpm")
    } else if root.join("yarn.lock").exists() {
        Some("yarn")
    } else if root.join("package-lock.json").exists() {
        Some("npm")
    } else {
        None
    }
}

/// Scan a project directory for package managers, environment files, and other
/// tooling indicators. Returns suggested setup commands in priority order.
#[tauri::command]
pub fn detect_package_manager(project_path: String) -> Result<Vec<DetectedSetup>, String> {
    let root = Path::new(&project_path);
    if !root.is_dir() {
        return Err(format!("Not a directory: {project_path}"));
    }

    let mut results: Vec<DetectedSetup> = Vec::new();

    // ── JavaScript / Node ──
    match js_lockfile_manager(root) {
        Some(pm) => results.push(DetectedSetup {
            id: pm.into(),
            label: format!("Install dependencies ({pm})"),
            // A committed npm lockfile gets the reproducible install.
            command: if pm == "npm" { "npm ci".into() } else { format!("{pm} install") },
            enabled: true,
        }),
        None if root.join("package.json").exists() => results.push(DetectedSetup {
            id: "npm".into(),
            label: "Install dependencies (npm)".into(),
            command: "npm install".into(),
            enabled: true,
        }),
        None => {}
    }

    // ── Rust ──
    if root.join("Cargo.toml").exists() {
        results.push(DetectedSetup {
            id: "cargo".into(),
            label: "Build project (cargo)".into(),
            command: "cargo build".into(),
            enabled: true,
        });
    }

    // ── Go ──
    if root.join("go.mod").exists() {
        results.push(DetectedSetup {
            id: "go".into(),
            label: "Download Go modules".into(),
            command: "go mod download".into(),
            enabled: true,
        });
    }

    // ── Python ──
    let has_pyproject = root.join("pyproject.toml").exists();
    if root.join("poetry.lock").exists() || (has_pyproject && !root.join("uv.lock").exists()) {
        // poetry.lock present, or pyproject.toml without uv.lock → poetry
        if root.join("poetry.lock").exists() {
            results.push(DetectedSetup {
                id: "poetry".into(),
                label: "Install dependencies (poetry)".into(),
                command: "poetry install".into(),
                enabled: true,
            });
        }
    }
    if root.join("uv.lock").exists() {
        results.push(DetectedSetup {
            id: "uv".into(),
            label: "Sync dependencies (uv)".into(),
            command: "uv sync".into(),
            enabled: true,
        });
    }
    if root.join("requirements.txt").exists()
        && !root.join("poetry.lock").exists()
        && !root.join("uv.lock").exists()
    {
        results.push(DetectedSetup {
            id: "pip".into(),
            label: "Install Python dependencies".into(),
            command: "pip install -r requirements.txt".into(),
            enabled: true,
        });
    }

    // ── Ruby ──
    if root.join("Gemfile").exists() {
        results.push(DetectedSetup {
            id: "ruby".into(),
            label: "Install Ruby dependencies".into(),
            command: "bundle install".into(),
            enabled: true,
        });
    }

    // ── PHP ──
    if root.join("composer.json").exists() {
        results.push(DetectedSetup {
            id: "php".into(),
            label: "Install PHP dependencies".into(),
            command: "composer install".into(),
            enabled: true,
        });
    }

    // ── Environment ──
    if root.join(".env.example").exists() {
        results.push(DetectedSetup {
            id: "env".into(),
            label: "Copy environment template".into(),
            command: "cp .env.example .env".into(),
            enabled: true,
        });
    } else if root.join(".env.sample").exists() {
        results.push(DetectedSetup {
            id: "env".into(),
            label: "Copy environment template".into(),
            command: "cp .env.sample .env".into(),
            enabled: true,
        });
    } else if root.join(".env.template").exists() {
        results.push(DetectedSetup {
            id: "env".into(),
            label: "Copy environment template".into(),
            command: "cp .env.template .env".into(),
            enabled: true,
        });
    }

    // ── Git submodules ──
    if root.join(".gitmodules").exists() {
        results.push(DetectedSetup {
            id: "submodules".into(),
            label: "Init git submodules".into(),
            command: "git submodule update --init --recursive".into(),
            enabled: true,
        });
    }

    // ── Docker Compose (disabled by default) ──
    let has_compose = root.join("docker-compose.yml").exists()
        || root.join("docker-compose.yaml").exists()
        || root.join("compose.yml").exists()
        || root.join("compose.yaml").exists();
    if has_compose {
        results.push(DetectedSetup {
            id: "docker".into(),
            label: "Start Docker services".into(),
            command: "docker compose up -d".into(),
            enabled: false,
        });
    }

    Ok(results)
}

/// A command the project probably starts its app with, offered when the user
/// has not set a run command yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunCandidate {
    pub command: String,
    /// The file the suggestion was read from, shown so the user can tell
    /// `make dev` from `npm run dev`.
    pub source: String,
}

/// Script and target names that usually start a long-running app, best first.
const RUN_NAMES: [&str; 5] = ["dev", "start", "serve", "run", "watch"];

/// Name parts that mark a script as a one-off task even when another part is
/// a run name, as in `test:watch`, `build:watch` or `test:run`.
const TASK_NAMES: [&str; 9] = [
    "test", "build", "lint", "check", "typecheck", "format", "fmt", "e2e", "coverage",
];

/// Suggest run commands for a project: package.json scripts (run with the
/// lockfile's package manager), Makefile and justfile targets, Cargo, Go,
/// Django and Docker Compose. Best guesses come first.
#[tauri::command]
pub async fn detect_run_candidates(project_path: String) -> Result<Vec<RunCandidate>, String> {
    // File reads stay off the GTK main thread, like `commands/files.rs`.
    tokio::task::spawn_blocking(move || detect_run_candidates_blocking(Path::new(&project_path)))
        .await
        .map_err(|e| e.to_string())?
}

/// Largest manifest read when detecting run commands. Real package.json,
/// Makefile and justfile files are far smaller.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// Read a manifest only when it is a regular file within the size cap, so a
/// huge file, FIFO or symlink to a device cannot stall detection.
fn read_manifest(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn detect_run_candidates_blocking(root: &Path) -> Result<Vec<RunCandidate>, String> {
    if !root.is_dir() {
        return Err(format!("Not a directory: {}", root.display()));
    }
    let mut out: Vec<RunCandidate> = Vec::new();
    let mut push = |command: String, source: &str| {
        if !out.iter().any(|c| c.command == command) {
            out.push(RunCandidate {
                command,
                source: source.into(),
            });
        }
    };

    let pm = js_lockfile_manager(root).unwrap_or("npm");
    for script in package_json_run_scripts(root) {
        push(format!("{pm} run {script}"), "package.json");
    }
    for (file, tool) in [
        ("Makefile", "make"),
        ("makefile", "make"),
        ("GNUmakefile", "make"),
        ("justfile", "just"),
        ("Justfile", "just"),
        (".justfile", "just"),
    ] {
        if let Some(text) = read_manifest(&root.join(file)) {
            for target in run_targets(&text) {
                push(format!("{tool} {target}"), file);
            }
        }
    }
    // A virtual workspace manifest has no [package], and a bare `cargo run`
    // there fails until a default member is chosen.
    if read_manifest(&root.join("Cargo.toml"))
        .is_some_and(|t| t.lines().any(|l| l.trim_start().starts_with("[package]")))
    {
        push("cargo run".into(), "Cargo.toml");
    }
    if root.join("go.mod").exists() && root.join("main.go").exists() {
        push("go run .".into(), "go.mod");
    }
    if root.join("manage.py").exists() {
        push("python manage.py runserver".into(), "manage.py");
    }
    for file in [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yml",
        "docker-compose.yaml",
    ] {
        if root.join(file).exists() {
            push("docker compose up".into(), file);
            break;
        }
    }
    Ok(out)
}

/// package.json scripts that look like they start the app: the well-known
/// names first, then variants such as `dev:web` or `tauri:dev`.
fn package_json_run_scripts(root: &Path) -> Vec<String> {
    let Some(text) = read_manifest(&root.join("package.json")) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = RUN_NAMES
        .iter()
        .filter(|n| scripts.contains_key(**n))
        .map(|n| n.to_string())
        .collect();
    for name in scripts.keys() {
        // npm runs `predev` / `postdev` around `dev` on its own.
        let is_hook = ["pre", "post"].iter().any(|p| {
            name.strip_prefix(p)
                .is_some_and(|base| scripts.contains_key(base))
        });
        // A variant must start or end with a run name (`dev:web`,
        // `tauri:dev`) and name no task, so watchers for tests or builds
        // are not offered as the app's run command.
        let parts: Vec<&str> = name.split([':', '-', '_']).collect();
        let edge_is_run = [parts.first(), parts.last()]
            .into_iter()
            .flatten()
            .any(|part| RUN_NAMES.contains(part));
        let names_task = parts.iter().any(|part| TASK_NAMES.contains(part));
        let looks_like_run = edge_is_run && !names_task;
        if looks_like_run && !is_hook && !names.contains(name) {
            names.push(name.clone());
        }
    }
    names
}

/// Makefile / justfile targets named like a run entry point, best first.
fn run_targets(text: &str) -> Vec<&'static str> {
    let defined: Vec<&str> = text
        .lines()
        .filter(|l| !l.starts_with([' ', '\t', '#', '.']))
        .filter_map(|l| {
            let (head, rest) = l.split_once(':')?;
            // `name := value`, `name ::= value` and `A ?= x:y` are
            // assignments, not targets. `run::` is a double-colon rule.
            // just recipe parameters (`dev port='3000':`) stay targets.
            if rest.trim_start_matches(':').starts_with('=') {
                return None;
            }
            let mut words = head.split_whitespace();
            let name = words.next()?;
            if words
                .next()
                .is_some_and(|w| ["=", "?=", "+=", "!="].iter().any(|op| w.starts_with(op)))
            {
                return None;
            }
            // just marks quiet recipes with a leading `@`.
            Some(name.trim_start_matches('@'))
        })
        .collect();
    RUN_NAMES
        .iter()
        .copied()
        .filter(|n| defined.contains(n))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_detect_npm_with_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(dir.path().join("package-lock.json"), "{}").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "npm");
        assert_eq!(results[0].command, "npm ci");
    }

    #[test]
    fn test_detect_npm_without_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "npm");
        assert_eq!(results[0].command, "npm install");
    }

    #[test]
    fn test_detect_bun() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(dir.path().join("bun.lock"), "").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "bun");
    }

    #[test]
    fn test_detect_cargo() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "cargo");
    }

    #[test]
    fn test_detect_multiple() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        fs::write(dir.path().join(".env.example"), "").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"pnpm"));
        assert!(ids.contains(&"cargo"));
        assert!(ids.contains(&"env"));
    }

    #[test]
    fn test_docker_compose_disabled_by_default() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("docker-compose.yml"), "").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "docker");
        assert!(!results[0].enabled);
    }

    #[test]
    fn test_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_poetry_detection() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "").unwrap();
        fs::write(dir.path().join("poetry.lock"), "").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"poetry"));
        assert!(!ids.contains(&"pip"));
    }

    #[test]
    fn test_uv_detection() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "").unwrap();
        fs::write(dir.path().join("uv.lock"), "").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"uv"));
        assert!(!ids.contains(&"pip"));
    }

    fn run_candidates(dir: &tempfile::TempDir) -> Vec<String> {
        detect_run_candidates_blocking(dir.path())
            .unwrap()
            .into_iter()
            .map(|c| c.command)
            .collect()
    }

    #[test]
    fn run_candidates_prefer_dev_then_start_with_lockfile_runner() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"vite build","start":"node .","predev":"x","dev":"vite","tauri:dev":"tauri dev","test":"vitest"}}"#,
        )
        .unwrap();
        fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(
            run_candidates(&dir),
            ["pnpm run dev", "pnpm run start", "pnpm run tauri:dev"]
        );
    }

    #[test]
    fn run_candidates_default_to_npm_without_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"serve":"http-server"}}"#,
        )
        .unwrap();
        assert_eq!(run_candidates(&dir), ["npm run serve"]);
    }

    #[test]
    fn run_candidates_ignore_invalid_package_json() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{ not json").unwrap();
        assert!(run_candidates(&dir).is_empty());
    }

    #[test]
    fn run_candidates_cover_make_just_cargo_go_django_and_compose() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Makefile"),
            "PORT := 3000\n.PHONY: run\nbuild:\n\tgo build\nrun: build\n\t./app\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("justfile"),
            "dev port='3000':\n    cargo watch\n",
        )
        .unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
        fs::write(dir.path().join("go.mod"), "module app").unwrap();
        fs::write(dir.path().join("main.go"), "package main").unwrap();
        fs::write(dir.path().join("manage.py"), "").unwrap();
        fs::write(dir.path().join("compose.yaml"), "").unwrap();
        assert_eq!(
            run_candidates(&dir),
            [
                "make run",
                "just dev",
                "cargo run",
                "go run .",
                "python manage.py runserver",
                "docker compose up",
            ]
        );
    }

    #[test]
    fn run_candidates_skip_test_and_build_watchers() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"dev":"vite","test:watch":"vitest","build:watch":"tsc -w","test:run":"vitest run","dev:web":"vite","my-dev-tools":"x"}}"#,
        )
        .unwrap();
        assert_eq!(run_candidates(&dir), ["npm run dev", "npm run dev:web"]);
    }

    #[test]
    fn run_targets_handle_quiet_recipes_and_posix_assignments() {
        assert_eq!(run_targets("@dev:\n    cargo watch\n"), ["dev"]);
        assert!(run_targets("run ::= ./app\nstart :::= x\ndev ?= x:y\nserve += a:b\n").is_empty());
        // just recipe parameters with defaults are still recipes.
        assert_eq!(run_targets("watch port='3000':\n    x\n"), ["watch"]);
        assert_eq!(run_targets("serve:: deps\n\t./app\n"), ["serve"]);
    }

    #[test]
    fn run_candidates_skip_cargo_virtual_workspace() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\"]\n",
        )
        .unwrap();
        assert!(run_candidates(&dir).is_empty());
    }

    #[test]
    fn run_candidates_read_hidden_justfile_and_commented_cargo_header() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".justfile"), "serve:\n    ./app\n").unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package] # the app\nname = \"app\"\n",
        )
        .unwrap();
        assert_eq!(run_candidates(&dir), ["just serve", "cargo run"]);
    }

    #[test]
    fn run_candidates_skip_oversized_and_non_file_manifests() {
        let dir = tempfile::tempdir().unwrap();
        let mut big = "dev:\n\t./app\n".to_string();
        big.push_str(&"#".repeat(MAX_MANIFEST_BYTES as usize));
        fs::write(dir.path().join("Makefile"), big).unwrap();
        // A directory named like a manifest is not read either.
        fs::create_dir(dir.path().join("justfile")).unwrap();
        assert!(run_candidates(&dir).is_empty());
    }

    #[test]
    fn run_candidates_reject_missing_directory() {
        assert!(detect_run_candidates_blocking(Path::new("/nonexistent/codemux-test")).is_err());
    }
}
