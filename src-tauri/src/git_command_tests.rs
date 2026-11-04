//! Git runs repository content that agents can write (hooks,
//! `core.fsmonitor`), so every git Codemux starts goes through
//! [`git_command`] and carries the agent-process marker.

use super::{git_command, git_command_tokio, run_git, source_control_cli_command, GIT_PROCESS_KIND};
use crate::control_identity::AGENT_PROCESS_ENV;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// `GIT_*`, SSH agents and credential helpers rely on git inheriting the
/// app's environment: the marker is the only variable added (with the
/// AppImage loader fixups, which are empty outside an AppImage).
#[test]
fn git_command_adds_only_the_agent_marker() {
    let cmd = git_command();
    assert_eq!(cmd.get_program(), "git");
    let lossy = |value: &std::ffi::OsStr| value.to_string_lossy().into_owned();
    let mut envs: Vec<(String, Option<String>)> = cmd.get_envs().map(|(key, value)| (lossy(key), value.map(lossy))).collect();
    let mut expected = crate::execution::appimage_env_fixups();
    expected.push((AGENT_PROCESS_ENV.to_string(), Some(GIT_PROCESS_KIND.to_string())));
    envs.sort();
    expected.sort();
    assert_eq!(envs, expected);
}

/// `gh` and `glab` run git themselves, so they carry exactly git's marker.
#[test]
fn source_control_clis_carry_the_git_marker() {
    for program in ["gh", "glab"] {
        let cli = source_control_cli_command(program);
        assert_eq!(cli.get_program(), program);
        assert!(cli.get_envs().eq(git_command().get_envs()), "{program}");
    }
}

/// A hook and an fsmonitor command set in the repository's config, as an
/// agent working there could, run with the marker, so a `codemux` they start
/// is an agent on the control socket.
#[cfg(unix)]
#[tokio::test]
async fn hooks_and_fsmonitor_run_with_the_agent_marker() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (repo, hooks) = (dir.path().join("repo"), dir.path().join("hooks"));
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&hooks).unwrap();
    // Setup runs raw git, which is not marked.
    let setup = |args: &[&str]| {
        let out = std::process::Command::new("git").current_dir(&repo).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    setup(&["init", "-q"]);
    setup(&[
        "-c", "user.name=Codemux test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false",
        "commit", "-q", "--allow-empty", "-m", "seed",
    ]);
    // Each probe records the marker it sees. The fsmonitor one then fails, so
    // git scans the worktree itself.
    let probe = |script: &Path, exit: u8| {
        let seen = script.with_extension("seen");
        let body = format!("#!/bin/sh\nprintenv {AGENT_PROCESS_ENV} > '{}'\nexit {exit}\n", seen.display());
        std::fs::write(script, body).unwrap();
        std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o755)).unwrap();
        seen
    };
    let hook = probe(&hooks.join("post-checkout"), 0);
    let fsmonitor = probe(&dir.path().join("fsmonitor"), 1);
    // Repository config wins over a global `core.hooksPath` or fsmonitor.
    setup(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    setup(&["config", "core.fsmonitor", dir.path().join("fsmonitor").to_str().unwrap()]);

    let clear = || {
        for seen in [&hook, &fsmonitor] {
            let _ = std::fs::remove_file(seen);
        }
    };
    // Exact: a test run from an agent's shell inherits another value.
    let saw_marker = |seen: &Path, what: &str| {
        let value = std::fs::read_to_string(seen).unwrap_or_else(|_| panic!("{what}: the probe never ran"));
        assert_eq!(value.trim_end(), GIT_PROCESS_KIND, "{what}");
    };
    clear();
    assert!(git_command().current_dir(&repo).args(["checkout", "-q", "-b", "one"]).status().unwrap().success());
    saw_marker(&hook, "git_command checkout");
    clear();
    run_git(&repo, &["status", "--porcelain"]).unwrap();
    saw_marker(&fsmonitor, "run_git status");
    clear();
    assert!(git_command_tokio().current_dir(&repo).args(["checkout", "-q", "-b", "two"]).status().await.unwrap().success());
    saw_marker(&hook, "git_command_tokio checkout");
}

/// What production code must not call: anything else that starts git, or
/// `gh` and `glab`, which start git themselves.
const RAW_GIT: [&str; 9] = [
    "Command::new(\"git\")", "host_command(\"git\")", "host_command_tokio(\"git\")",
    "Command::new(\"gh\")", "host_command(\"gh\")", "host_command_tokio(\"gh\")",
    "Command::new(\"glab\")", "host_command(\"glab\")", "host_command_tokio(\"glab\")",
];

/// Every production file starts git only through [`git_command`], and `gh`
/// and `glab` only through [`source_control_cli_command`]. Test code (gated
/// by `cfg(test)` alone or inside `all(..)`, inline or as a module file) may
/// run them raw.
#[test]
fn production_code_starts_git_only_through_git_command() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rust_files(&src, &mut files);
    let sources: Vec<(PathBuf, String)> = files
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).unwrap();
            (path, text)
        })
        .collect();

    // A module file declared under a test cfg is test code, and so are its
    // own submodules.
    let mut test_files = Vec::new();
    for (path, text) in &sources {
        let code = blank_non_code(text);
        test_files.extend(test_items(&code).filter_map(|item| test_module_file(path, text, &code, item)));
    }
    let is_test_file = |path: &Path| {
        test_files.iter().any(|file| {
            let children = if file.ends_with("mod.rs") { file.parent().unwrap().to_path_buf() } else { file.with_extension("") };
            path == file || path.starts_with(children)
        })
    };

    let constructor = src.join("git.rs");
    let mut found = Vec::new();
    let mut production_files = 0;
    for (path, text) in sources.iter().filter(|(path, _)| !is_test_file(path)) {
        production_files += 1;
        let allowed = (*path == constructor).then_some("fn git_command() -> Command {");
        let (lines, allowed_hits) = raw_git_lines(text, allowed);
        if allowed.is_some() {
            assert_eq!(allowed_hits, 1, "git_command no longer builds git itself; update this test");
        }
        let name = path.strip_prefix(&src).unwrap().display();
        found.extend(lines.into_iter().map(|line| format!("src/{name}:{line}")));
    }
    // Guards against a scan that takes everything for test code.
    assert!(production_files > 100, "only {production_files} production files scanned");
    assert!(
        found.is_empty(),
        "start git with crate::git::git_command() or git_command_tokio(), and gh or glab with crate::git::source_control_cli_command():\n{}",
        found.join("\n")
    );
}

/// The scanner itself: comments, strings and test-gated items are skipped,
/// production code after them is not.
#[test]
fn raw_git_scan_skips_only_comments_strings_and_test_code() {
    let source = r##"
fn a() { std::process::Command::new("git"); }
// Command::new("git") in a comment
const S: &str = r#"Command::new("git")"#;
#[cfg(test)]
mod tests {
    fn t() { let close = '}'; Command::new("git"); }
}
#[cfg(all(test, unix))]
#[allow(dead_code)]
fn u() { host_command("git"); }
#[cfg(any(test, windows))]
fn w<'a>() { tokio::process::Command::new("git"); }
fn git_command() -> Command { host_command("git") }
fn pr() -> Command { host_command("gh") }
"##;
    assert_eq!(raw_git_lines(source, None), (vec![2, 13, 14, 15], 0));
    assert_eq!(raw_git_lines(source, Some("fn git_command() -> Command {")), (vec![2, 13, 15], 1));
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Lines of `text` that start git outside test code, and how many starts sat
/// inside the item that begins with `allowed`.
fn raw_git_lines(text: &str, allowed: Option<&str>) -> (Vec<usize>, usize) {
    let code = blank_non_code(text);
    let skipped: Vec<Range<usize>> = test_items(&code).collect();
    let allowed = allowed.map(|signature| {
        let start = code.find(signature).unwrap_or_else(|| panic!("`{signature}` not found"));
        start..item_end(&code, start)
    });
    let (mut lines, mut allowed_hits) = (Vec::new(), 0);
    for pattern in RAW_GIT {
        for (at, _) in text.match_indices(pattern) {
            // Comments and string literals are blanked in `code`.
            if code.as_bytes()[at] == b' ' || skipped.iter().any(|range| range.contains(&at)) {
                continue;
            }
            if allowed.as_ref().is_some_and(|range| range.contains(&at)) {
                allowed_hits += 1;
            } else {
                lines.push(text[..at].matches('\n').count() + 1);
            }
        }
    }
    lines.sort();
    (lines, allowed_hits)
}

/// `source` with comments and string and char literals replaced by spaces
/// (newlines kept), byte for byte, so offsets still match `source` and no
/// bracket or `#[cfg` inside them counts.
fn blank_non_code(source: &str) -> String {
    let s = source.as_bytes();
    let at = |i: usize| s.get(i).copied().unwrap_or(0);
    let ident = |i: usize| i > 0 && (s[i - 1].is_ascii_alphanumeric() || s[i - 1] == b'_');
    let mut out = s.to_vec();
    let mut i = 0;
    while i < s.len() {
        let end = match s[i] {
            b'/' if at(i + 1) == b'/' => s[i..].iter().position(|&b| b == b'\n').map_or(s.len(), |n| i + n),
            b'/' if at(i + 1) == b'*' => {
                let (mut depth, mut j) = (0, i);
                while j < s.len() {
                    if s[j..].starts_with(b"/*") {
                        (depth, j) = (depth + 1, j + 2);
                    } else if s[j..].starts_with(b"*/") {
                        (depth, j) = (depth - 1, j + 2);
                        if depth == 0 {
                            break;
                        }
                    } else {
                        j += 1;
                    }
                }
                j
            }
            b'"' => {
                let mut j = i + 1;
                while j < s.len() && s[j] != b'"' {
                    j += if s[j] == b'\\' { 2 } else { 1 };
                }
                j + 1
            }
            // `r"…"`, `r#"…"#` or `br"…"`, not a raw identifier or a name ending in r.
            b'r' if !ident(i) || (at(i - 1) == b'b' && !ident(i - 1)) => {
                let hashes = s[i + 1..].iter().take_while(|&&b| b == b'#').count();
                if at(i + 1 + hashes) != b'"' {
                    i += 1;
                    continue;
                }
                let close: Vec<u8> = std::iter::once(b'"').chain(std::iter::repeat(b'#').take(hashes)).collect();
                let body = i + 2 + hashes;
                s[body..].windows(close.len()).position(|w| w == close).map_or(s.len(), |n| body + n + close.len())
            }
            // An escaped char literal (`'\n'`, `'\''`, `'\u{..}'`).
            b'\'' if at(i + 1) == b'\\' => s[(i + 3).min(s.len())..].iter().position(|&b| b == b'\'').map_or(s.len(), |n| i + 4 + n),
            // A char literal, unless it is a lifetime or a label.
            b'\'' => {
                let len = match at(i + 1) {
                    0xF0.. => 4,
                    0xE0.. => 3,
                    0xC0.. => 2,
                    _ => 1,
                };
                if at(i + 1 + len) != b'\'' {
                    i += 1;
                    continue;
                }
                i + 2 + len
            }
            _ => {
                i += 1;
                continue;
            }
        };
        let end = end.min(s.len());
        for byte in &mut out[i..end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
        i = end;
    }
    String::from_utf8(out).unwrap()
}

/// Byte ranges of the items in `code` (from [`blank_non_code`]) that a cfg
/// requiring `test` gates, each from its `#[cfg` to the item's end.
fn test_items(code: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    code.match_indices("#[cfg(").filter_map(|(start, _)| {
        let attr_end = item_end(code, start + 1);
        let predicate: String = code[start + "#[cfg(".len()..attr_end - ")]".len()].split_whitespace().collect();
        let all_args = predicate.strip_prefix("all(").and_then(|rest| rest.strip_suffix(')'));
        let gated = predicate == "test" || all_args.is_some_and(|args| top_level_args(args).any(|arg| arg == "test"));
        gated.then(|| start..item_end(code, item_start(code, attr_end)))
    })
}

/// Where the item after an attribute ending at `from` starts, past any more
/// attributes.
fn item_start(code: &str, mut from: usize) -> usize {
    loop {
        from += code[from..].len() - code[from..].trim_start().len();
        if !code[from..].starts_with("#[") {
            return from;
        }
        from = item_end(code, from + 1);
    }
}

/// End of the bracketed group that opens at `start` or, when `start` is not
/// a bracket, of the item there: its first `;` outside brackets or the `}`
/// that closes its body.
fn item_end(code: &str, start: usize) -> usize {
    let group = matches!(code.as_bytes()[start], b'{' | b'[' | b'(');
    let mut depth = 0usize;
    for (i, byte) in code.bytes().enumerate().skip(start) {
        match byte {
            b'{' | b'[' | b'(' => depth += 1,
            // An unmatched closer ends the enclosing block, and the item with it.
            b'}' | b']' | b')' if depth == 0 => return i,
            b'}' | b']' | b')' => {
                depth -= 1;
                if depth == 0 && (group || byte == b'}') {
                    return i + 1;
                }
            }
            b';' if depth == 0 => return i + 1,
            _ => {}
        }
    }
    code.len()
}

/// The comma-separated arguments at the top level of a cfg predicate.
fn top_level_args(args: &str) -> impl Iterator<Item = &str> {
    let mut depth = 0;
    args.split(move |c: char| {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        c == ',' && depth == 0
    })
}

/// The file of `mod name;` when `item` declares one, found as rustc finds
/// it from the declaring file at `path` (a `#[path]` attribute wins).
fn test_module_file(path: &Path, text: &str, code: &str, item: Range<usize>) -> Option<PathBuf> {
    let body = item_start(code, item_end(code, item.start + 1));
    let declaration = code[body..item.end].trim();
    let declaration = match declaration.strip_prefix("pub") {
        Some(rest) => rest.trim_start().strip_prefix('(').and_then(|scoped| scoped.split_once(')')).map_or(rest, |(_, after)| after),
        None => declaration,
    };
    let name = declaration.trim_start().strip_prefix("mod ")?.strip_suffix(';')?.trim();
    let dir = path.parent().unwrap();
    // The path literal is blanked in `code`, so it is read from `text`.
    if let Some(rest) = text[item.start..body].split("#[").find_map(|attr| attr.trim_start().strip_prefix("path")) {
        return Some(dir.join(rest.split('"').nth(1).unwrap()));
    }
    let file_name = path.file_name().unwrap();
    let owns_dir = dir.ends_with("bin") || ["mod.rs", "lib.rs", "main.rs"].iter().any(|root| file_name == *root);
    let base = if owns_dir { dir.to_path_buf() } else { path.with_extension("") };
    let file = [base.join(format!("{name}.rs")), base.join(name).join("mod.rs")].into_iter().find(|file| file.exists());
    Some(file.unwrap_or_else(|| panic!("{}: cannot find the file of test module `{name}`", path.display())))
}
