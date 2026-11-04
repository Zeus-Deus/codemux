//! Checks that every `codemux …` command in the outside-agent guide's and the
//! README's examples parses with this command tree: shell lines, including the
//! ones MCP clients are told to start (`claude mcp add … -- codemux mcp …`),
//! the `command` and `args` of MCP client configs, and `authorized_keys`
//! forced commands.
use super::Cli;
use clap::{CommandFactory, Parser};
use serde_json::Value;
use std::collections::BTreeSet;

const DOCS: [(&str, &str); 2] = [
    ("docs/outside-agents.md", include_str!("../../../docs/outside-agents.md")),
    ("README.md", include_str!("../../../README.md")),
];

/// One fenced code block: its info string and its lines, without the fence's
/// own indentation (YAML needs the rest).
struct Fence<'a> {
    info: &'a str,
    lines: Vec<&'a str>,
}

impl Fence<'_> {
    fn is_shell(&self) -> bool {
        matches!(self.info, "sh" | "bash" | "shell")
    }
}

fn fences(doc: &str) -> Vec<Fence<'_>> {
    let mut fences = Vec::new();
    let mut open: Option<(&str, Fence)> = None;
    for line in doc.lines() {
        let trimmed = line.trim_start();
        if let Some(info) = trimmed.strip_prefix("```") {
            match open.take() {
                Some((_, fence)) => fences.push(fence),
                None => open = Some((&line[..line.len() - trimmed.len()], Fence { info: info.trim(), lines: Vec::new() })),
            }
            continue;
        }
        if let Some((indent, fence)) = open.as_mut() {
            fence.lines.push(line.strip_prefix(*indent).unwrap_or(trimmed));
        }
    }
    assert!(open.is_none(), "a code fence is never closed");
    fences
}

/// A config fence parsed as JSON values, or `None` for other fences.
fn config(fence: &Fence<'_>) -> Option<Result<Value, String>> {
    let text = fence.lines.join("\n");
    match fence.info {
        "json" => Some(serde_json::from_str(&text).map_err(|error| error.to_string())),
        "toml" => Some(toml::from_str(&text).map_err(|error| error.to_string())),
        "yaml" | "yml" => Some(serde_yaml::from_str(&text).map_err(|error| error.to_string())),
        _ => None,
    }
}

/// Every program a config starts: an object's `command` followed by its
/// `args`, as in `mcpServers` JSON and Hermes's YAML.
fn launches(value: &Value, found: &mut Vec<Vec<String>>) {
    match value {
        Value::Object(map) => {
            if let (Some(Value::String(command)), Some(Value::Array(args))) = (map.get("command"), map.get("args")) {
                let args = args.iter().map(|arg| arg.as_str().map(str::to_owned).unwrap_or_else(|| panic!("`args` holds strings: {arg}")));
                found.push(std::iter::once(command.clone()).chain(args).collect());
            }
            map.values().for_each(|value| launches(value, found));
        }
        Value::Array(items) => items.iter().for_each(|value| launches(value, found)),
        _ => {}
    }
}

/// The forced commands in an `authorized_keys` line: `command="…"`.
fn forced_commands(line: &str) -> impl Iterator<Item = &str> {
    line.split("command=\"").skip(1).filter_map(|rest| rest.split_once('"').map(|(command, _)| command))
}

/// Splits one shell line into its commands, each a list of words with the
/// quotes removed. Commands end at `|`, `;`, `&`, `(`, `)` and `$(` outside
/// quotes, so `WS=$(codemux workspace create …)` yields the inner command.
/// Enough for the docs' examples: single and double quotes, no escapes.
fn commands(line: &str) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word: Option<String> = None;
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    let end_word = |word: &mut Option<String>, words: &mut Vec<String>| words.extend(word.take());
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (Some(_), c) => word.get_or_insert_with(String::new).push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                word.get_or_insert_with(String::new);
            }
            (None, '#') if word.is_none() => break,
            (None, c) if c.is_whitespace() => end_word(&mut word, &mut words),
            (None, '|' | ';' | '&' | '(' | ')') => {
                end_word(&mut word, &mut words);
                commands.push(std::mem::take(&mut words));
            }
            (None, '$') if chars.peek() == Some(&'(') => {
                chars.next();
                end_word(&mut word, &mut words);
                commands.push(std::mem::take(&mut words));
            }
            (None, c) => word.get_or_insert_with(String::new).push(c),
        }
    }
    assert!(quote.is_none(), "unbalanced quote in `{line}`");
    end_word(&mut word, &mut words);
    commands.push(words);
    commands.retain(|words| !words.is_empty());
    commands
}

/// `NAME=value`, a variable assignment before or instead of a command.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

/// What may follow `codemux` when it runs as a program: a command, an alias
/// or a root flag. After anything else, `codemux` is the MCP server's name,
/// as in `claude mcp add --scope user codemux -- …`.
fn command_starts() -> BTreeSet<String> {
    let cli = Cli::command();
    let mut starts: BTreeSet<String> = cli
        .get_subcommands()
        .flat_map(|command| std::iter::once(command.get_name()).chain(command.get_all_aliases()))
        .map(str::to_owned)
        .collect();
    starts.extend(cli.get_arguments().filter_map(|arg| arg.get_long()).map(|long| format!("--{long}")));
    starts
}

/// The `codemux` invocation in one command, if any: `codemux` as the program
/// (first word, or right after `--`), followed by a command or root flag (as
/// after `ssh -T host`), or Hermes's `--command codemux --args …`.
fn invocation(words: &[String], starts: &BTreeSet<String>) -> Option<Vec<String>> {
    let program = words.iter().position(|word| !is_assignment(word))?;
    for (index, word) in words.iter().enumerate().skip(program) {
        if word != "codemux" {
            continue;
        }
        let next = words.get(index + 1).map(String::as_str);
        if next == Some("--args") {
            return Some(std::iter::once(word.clone()).chain(words[index + 2..].iter().cloned()).collect());
        }
        let runs = index == program || words[index - 1] == "--" || next.is_some_and(|next| starts.contains(next));
        if runs {
            return Some(words[index..].to_vec());
        }
    }
    None
}

#[test]
fn shell_splitting_follows_quotes_and_substitutions() {
    assert_eq!(commands(r#"WS=$(codemux workspace create ~/src/api)"#), [vec!["WS="], vec!["codemux", "workspace", "create", "~/src/api"]]);
    assert_eq!(
        commands(r#"codemux call x '{"a": "b c"}' -m "it's $WS" # note"#),
        [vec!["codemux", "call", "x", r#"{"a": "b c"}"#, "-m", "it's $WS"]]
    );
    let starts = command_starts();
    let found = |line: &str| commands(line).iter().find_map(|words| invocation(words, &starts));
    assert_eq!(found("claude mcp add --scope user codemux -- codemux mcp --client a").unwrap(), ["codemux", "mcp", "--client", "a"]);
    assert_eq!(found("hermes mcp add codemux --command codemux --args mcp --client h").unwrap(), ["codemux", "mcp", "--client", "h"]);
    assert_eq!(found("x -- ssh -T desktop codemux mcp").unwrap(), ["codemux", "mcp"]);
    assert_eq!(found("codemux treads start").unwrap(), ["codemux", "treads", "start"], "a misspelt command is still checked");
    assert_eq!(found("claude mcp get codemux"), None);
    assert_eq!(found("codex mcp add codemux --url \"$URL\""), None);
}

#[test]
fn config_examples_yield_the_programs_clients_start() {
    let doc = "Text\n  ```yaml\n  servers:\n    a:\n      command: codemux\n      args: [\"mcp\", \"--client\", \"h\"]\n  ```\n";
    let [fence] = fences(doc).try_into().ok().expect("one fence");
    assert_eq!(fence.info, "yaml");
    let mut found = Vec::new();
    launches(&config(&fence).unwrap().unwrap(), &mut found);
    assert_eq!(found, [["codemux", "mcp", "--client", "h"]], "the fence's indentation is not YAML's");
    let mut found = Vec::new();
    launches(&serde_json::json!({"s": {"x": {"command": "ssh", "args": ["-T", "d", "codemux", "mcp"]}, "y": {"url": "u"}}}), &mut found);
    assert_eq!(found, [["ssh", "-T", "d", "codemux", "mcp"]]);
    let keys = r#"restrict,command="codemux mcp --access supervised" ssh-ed25519 AAAA… bot"#;
    assert_eq!(forced_commands(keys).collect::<Vec<_>>(), ["codemux mcp --access supervised"]);
    assert_eq!(forced_commands("ssh-ed25519 AAAA… bot").count(), 0);
}

/// Checks one `codemux` invocation the docs show.
fn check(doc: &str, shown: &str, argv: &[String]) {
    for word in argv {
        assert!(!word.contains(['<', '>']), "{doc}: write `{word}` as a \"$VARIABLE\", not a <placeholder>: `{shown}`");
    }
    if let Err(error) = Cli::try_parse_from(argv) {
        panic!("{doc}: `{shown}` does not parse:\n{}", error.render());
    }
}

/// The CLI reports an unreachable or older app with codes of its own, where
/// `codemux mcp` reports `app_unavailable` and `server_too_old`. The guide's
/// troubleshooting names both, so a script author finds either.
#[test]
fn outside_agents_guide_names_the_clis_own_transport_codes() {
    let (_, guide) = DOCS[0];
    for code in [super::door::code::UNAVAILABLE, super::door::code::OUTDATED] {
        assert!(guide.contains(&format!("`{code}`")), "the guide names `{code}`");
    }
}

#[test]
fn outside_agents_guide_cli_examples_parse() {
    let starts = command_starts();
    let mut verbs = BTreeSet::new();
    let mut checked = 0;
    // The kinds of non-shell example that started `codemux`.
    let mut configs = BTreeSet::new();
    for (doc, text) in DOCS {
        for fence in fences(text) {
            if fence.is_shell() {
                for line in fence.lines.iter().map(|line| line.trim()) {
                    for words in commands(line) {
                        let Some(argv) = invocation(&words, &starts) else { continue };
                        check(doc, line, &argv);
                        // The first word after the root flags names the command.
                        verbs.extend(argv.iter().skip(1).find(|word| !word.starts_with('-') && starts.contains(word.as_str())).cloned());
                        checked += 1;
                    }
                }
                continue;
            }
            if let Some(parsed) = config(&fence) {
                let value = parsed.unwrap_or_else(|error| panic!("{doc}: a ```{} example does not parse: {error}", fence.info));
                let mut found = Vec::new();
                launches(&value, &mut found);
                for words in found {
                    let Some(argv) = invocation(&words, &starts) else { continue };
                    check(doc, &words.join(" "), &argv);
                    configs.insert(fence.info);
                }
            }
            for command in fence.lines.iter().flat_map(|line| forced_commands(*line)) {
                for words in commands(command) {
                    let Some(argv) = invocation(&words, &starts) else { continue };
                    check(doc, command, &argv);
                    configs.insert("authorized_keys");
                }
            }
        }
    }
    for verb in ["mcp", "tools", "call", "workspace", "worktree", "thread", "connector"] {
        assert!(verbs.contains(verb), "the guide shows `codemux {verb}` in a shell example");
    }
    assert!(checked >= 20, "only {checked} shell examples were checked");
    for kind in ["json", "yaml", "authorized_keys"] {
        assert!(configs.contains(kind), "a {kind} example that starts `codemux` was checked");
    }
}
