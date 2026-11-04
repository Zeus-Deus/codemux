//! Checks that every `codemux …` command in the outside-agent guide's and the
//! README's shell examples parses with this command tree, including the ones
//! MCP clients are told to start (`claude mcp add … -- codemux mcp …`).
use super::Cli;
use clap::{CommandFactory, Parser};
use std::collections::BTreeSet;

const DOCS: [(&str, &str); 2] = [
    ("docs/outside-agents.md", include_str!("../../../docs/outside-agents.md")),
    ("README.md", include_str!("../../../README.md")),
];

/// The lines inside ```` ```sh ````, ```` ```bash ```` and ```` ```shell ```` fences.
fn shell_lines(doc: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut fence: Option<bool> = None;
    for line in doc.lines() {
        let trimmed = line.trim();
        if let Some(info) = trimmed.strip_prefix("```") {
            fence = match fence {
                Some(_) => None,
                None => Some(matches!(info.trim(), "sh" | "bash" | "shell")),
            };
            continue;
        }
        if fence == Some(true) {
            lines.push(trimmed);
        }
    }
    assert!(fence.is_none(), "a code fence is never closed");
    lines
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
fn outside_agents_guide_cli_examples_parse() {
    let starts = command_starts();
    let mut verbs = BTreeSet::new();
    let mut checked = 0;
    for (doc, text) in DOCS {
        for line in shell_lines(text) {
            for words in commands(line) {
                let Some(argv) = invocation(&words, &starts) else { continue };
                for word in &argv {
                    assert!(!word.contains(['<', '>']), "{doc}: write `{word}` as a \"$VARIABLE\", not a <placeholder>: `{line}`");
                }
                if let Err(error) = Cli::try_parse_from(&argv) {
                    panic!("{doc}: `{line}` does not parse:\n{}", error.render());
                }
                // The first word after the root flags names the command.
                verbs.extend(argv.iter().skip(1).find(|word| !word.starts_with('-') && starts.contains(word.as_str())).cloned());
                checked += 1;
            }
        }
    }
    for verb in ["mcp", "tools", "call", "workspace", "worktree", "thread", "connector"] {
        assert!(verbs.contains(verb), "the guide shows `codemux {verb}` in a shell example");
    }
    assert!(checked >= 20, "only {checked} shell examples were checked");
}
