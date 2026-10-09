//! Host aliases from the user's OpenSSH client config, offered as
//! suggestions when adding a device. Read-only and deliberately shallow:
//! the alias is what gets saved as the SSH target, so `ssh` itself stays
//! the authority on what it resolves to (HostName, User, Port, keys).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// OpenSSH's own nesting limit for `Include`.
const MAX_INCLUDE_DEPTH: usize = 16;

/// Concrete `Host` aliases from `<home>/.ssh/config` and the files it
/// includes for every destination, deduplicated in the order they
/// appear. Patterns with `*`, `?` or `!` name no single machine and are
/// skipped. A missing or unreadable file contributes nothing.
pub fn config_hosts(home: &Path) -> Vec<String> {
    let mut collector = Collector {
        home,
        visited: HashSet::new(),
        seen: HashSet::new(),
        hosts: Vec::new(),
    };
    collector.read_file(&home.join(".ssh").join("config"), 0);
    collector.hosts
}

struct Collector<'a> {
    home: &'a Path,
    /// Files already read, so an `Include` cycle ends at once.
    visited: HashSet<PathBuf>,
    seen: HashSet<String>,
    hosts: Vec<String>,
}

impl Collector<'_> {
    fn read_file(&mut self, path: &Path, depth: usize) {
        if depth > MAX_INCLUDE_DEPTH {
            return;
        }
        let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !self.visited.insert(key) {
            return;
        }
        if let Ok(text) = std::fs::read_to_string(path) {
            self.parse(&text, depth);
        }
    }

    fn parse(&mut self, text: &str, depth: usize) {
        // Options under `Match`, or under a `Host` that doesn't match
        // every destination, apply only to some destinations. ssh reads
        // an `Include` there only for those, and the `Host` lines it
        // pulls in are matched against that same destination, so their
        // aliases never resolve under their own names: such Includes are
        // skipped. A block runs until the next `Host` or `Match`.
        let mut conditional = false;
        for line in text.lines() {
            let Some((keyword, args)) = split_line(line) else {
                continue;
            };
            match keyword.to_ascii_lowercase().as_str() {
                "host" => {
                    conditional = !args.iter().all(|pattern| pattern == "*");
                    for pattern in args {
                        if is_concrete(&pattern) && self.seen.insert(pattern.clone()) {
                            self.hosts.push(pattern);
                        }
                    }
                }
                "match" => conditional = true,
                "include" if !conditional => {
                    for arg in &args {
                        for path in self.include_paths(arg) {
                            self.read_file(&path, depth + 1);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Resolve one `Include` argument the way ssh does for the user
    /// config: `~` is the home directory, a relative path is relative to
    /// `~/.ssh`, and globs expand in sorted order.
    fn include_paths(&self, arg: &str) -> Vec<PathBuf> {
        let (base, rest) = if arg == "~" {
            (Some(self.home.to_path_buf()), "")
        } else if let Some(rest) = arg.strip_prefix("~/") {
            (Some(self.home.to_path_buf()), rest)
        } else if Path::new(arg).is_absolute() {
            (None, arg)
        } else {
            (Some(self.home.join(".ssh")), arg)
        };
        // The fixed prefix is escaped so a home path containing `[` or
        // `*` can't turn into a pattern; only the user's part globs.
        let pattern = match base {
            Some(base) => {
                let base = PathBuf::from(glob::Pattern::escape(&base.to_string_lossy()));
                if rest.is_empty() {
                    base
                } else {
                    base.join(rest)
                }
            }
            None => PathBuf::from(rest),
        };
        match glob::glob(&pattern.to_string_lossy()) {
            Ok(paths) => paths.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// Split a config line into its keyword and arguments. `Keyword value`,
/// `Keyword=value` and `Keyword = value` are all valid, and arguments may
/// be double-quoted. Blank lines and comments yield `None`.
fn split_line(line: &str) -> Option<(&str, Vec<String>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let (keyword, rest) = line.split_at(end);
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest);
    Some((keyword, split_args(rest)))
}

fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let Some(&first) = chars.peek() else {
            break;
        };
        // A trailing comment ends the line.
        if first == '#' {
            break;
        }
        let mut arg = String::new();
        if first == '"' {
            chars.next();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                arg.push(c);
            }
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                arg.push(c);
            }
        }
        args.push(arg);
    }
    args
}

fn is_concrete(pattern: &str) -> bool {
    !pattern.is_empty() && !pattern.contains(['*', '?', '!'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A fake home with `.ssh/config` holding `config`.
    fn home_with(config: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join(".ssh")).unwrap();
        fs::write(home.path().join(".ssh/config"), config).unwrap();
        home
    }

    #[test]
    fn lists_concrete_aliases_in_order_and_skips_patterns() {
        let home = home_with(
            "# personal machines\n\
             Host zeus pandora\n\
             \x20 HostName 10.0.0.5\n\
             \x20 User deus\n\
             \n\
             Host *.internal !bastion web-?\n\
             Host *\n\
             \x20 ServerAliveInterval 30\n\
             host homelab   # lower-case keyword, trailing comment\n\
             Host zeus\n",
        );
        assert_eq!(
            config_hosts(home.path()),
            vec!["zeus", "pandora", "homelab"]
        );
    }

    #[test]
    fn accepts_equals_separator_and_quoted_names() {
        let home = home_with("Host=alpha\nHost = beta\nHost \"gamma\" delta\n");
        assert_eq!(
            config_hosts(home.path()),
            vec!["alpha", "beta", "gamma", "delta"]
        );
    }

    #[test]
    fn match_blocks_end_at_the_next_host_and_their_includes_are_skipped() {
        let home = home_with(
            "Match host foo exec \"true\"\n\
             \x20 Include conditional.conf\n\
             Host after-match\n",
        );
        fs::write(home.path().join(".ssh/conditional.conf"), "Host hidden\n").unwrap();
        assert_eq!(config_hosts(home.path()), vec!["after-match"]);
    }

    #[test]
    fn follows_includes_relative_to_ssh_dir_tilde_and_globs() {
        let home = home_with(
            "Include config.d/*.conf\n\
             Host main\n\
             Host *\n\
             \x20 Include ~/extra/hosts\n",
        );
        let ssh = home.path().join(".ssh");
        fs::create_dir_all(ssh.join("config.d")).unwrap();
        fs::write(ssh.join("config.d/b.conf"), "Host bravo\n").unwrap();
        fs::write(ssh.join("config.d/a.conf"), "Host alpha\n").unwrap();
        fs::write(ssh.join("config.d/notes.txt"), "Host ignored\n").unwrap();
        fs::create_dir_all(home.path().join("extra")).unwrap();
        fs::write(home.path().join("extra/hosts"), "Host tilde-host\n").unwrap();

        assert_eq!(
            config_hosts(home.path()),
            vec!["alpha", "bravo", "main", "tilde-host"],
            "glob matches expand in sorted order, at the point of the Include"
        );
    }

    #[test]
    fn includes_under_a_specific_host_are_skipped() {
        // ssh reads these only when connecting to `work-bastion`, so
        // `build01` would never resolve as a target of its own.
        let home = home_with(
            "Host work-bastion\n\
             \x20 Include work.d/*\n\
             Host * !jump\n\
             \x20 Include negated.conf\n\
             Host *\n\
             \x20 Include shared.conf\n",
        );
        let ssh = home.path().join(".ssh");
        fs::create_dir_all(ssh.join("work.d")).unwrap();
        fs::write(
            ssh.join("work.d/a.conf"),
            "Host build01\nHostName 10.1.2.3\n",
        )
        .unwrap();
        fs::write(ssh.join("negated.conf"), "Host hidden\n").unwrap();
        fs::write(ssh.join("shared.conf"), "Host shared\n").unwrap();
        assert_eq!(config_hosts(home.path()), vec!["work-bastion", "shared"]);
    }

    #[test]
    fn include_cycles_terminate() {
        let home = home_with("Include config\nInclude loop.conf\nHost top\n");
        fs::write(
            home.path().join(".ssh/loop.conf"),
            "Include loop.conf\nHost looped\n",
        )
        .unwrap();
        assert_eq!(config_hosts(home.path()), vec!["looped", "top"]);
    }

    #[test]
    fn missing_config_or_include_yields_nothing() {
        let home = tempfile::tempdir().unwrap();
        assert!(config_hosts(home.path()).is_empty());

        let home = home_with("Include nope/*.conf\nInclude /does/not/exist\nHost real\n");
        assert_eq!(config_hosts(home.path()), vec!["real"]);
    }
}
