// SKILL.md frontmatter parser.
//
// SKILL.md follows the Agent Skills convention shared by Claude, Codex, and
// OpenCode: an optional `---`-delimited YAML block at the top of the file,
// then markdown body. We extract the YAML as a generic JSON value so callers
// can introspect provider-specific fields (allowed-tools, version, license,
// metadata.short-description, ...) without us having to model each one.

use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;

pub struct ParsedSkillFile {
    pub frontmatter: JsonValue,
    pub body: String,
}

/// Splits a SKILL.md into frontmatter + body. If the file has no frontmatter
/// the entire file is the body. Returns `Err` only when the frontmatter
/// region is present but cannot be parsed as YAML — caller decides whether
/// to skip the skill or surface the error.
pub fn parse_skill_file(content: &str) -> Result<ParsedSkillFile, String> {
    // Be tolerant of BOMs and leading blank lines so user-written skills
    // that came from a Windows editor still parse.
    let trimmed = content.trim_start_matches('\u{feff}');

    let after_first_delim = match strip_opening_delim(trimmed) {
        Some(rest) => rest,
        None => {
            return Ok(ParsedSkillFile {
                frontmatter: JsonValue::Object(serde_json::Map::new()),
                body: content.to_string(),
            });
        }
    };

    let (yaml_text, body) = match split_at_closing_delim(after_first_delim) {
        Some(pair) => pair,
        None => {
            return Err("frontmatter is missing closing `---` delimiter".to_string());
        }
    };

    let yaml: YamlValue = serde_yaml::from_str(yaml_text)
        .map_err(|e| format!("invalid YAML frontmatter: {e}"))?;

    let frontmatter = yaml_to_json(yaml);

    Ok(ParsedSkillFile {
        frontmatter,
        body: body.to_string(),
    })
}

fn strip_opening_delim(text: &str) -> Option<&str> {
    // Accept `---` followed by newline. Reject `----` etc. by requiring the
    // line to be exactly the delimiter.
    let mut lines = text.splitn(2, '\n');
    let first = lines.next()?.trim_end_matches('\r');
    if first.trim() != "---" {
        return None;
    }
    Some(lines.next().unwrap_or(""))
}

fn split_at_closing_delim(text: &str) -> Option<(&str, &str)> {
    // Find a line that is exactly `---` (or `...`, an alternate YAML
    // end-of-document marker, just in case).
    let mut cursor = 0usize;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
        if trimmed == "---" || trimmed == "..." {
            let yaml_text = &text[..cursor];
            let body_start = cursor + line.len();
            let body = &text[body_start..];
            return Some((yaml_text, body));
        }
        cursor += line.len();
    }
    None
}

/// Convert a `serde_yaml::Value` into a `serde_json::Value`. We do this so
/// the frontmatter can be sent to the frontend through Tauri (which uses
/// JSON) and inspected with normal JSON tools downstream.
fn yaml_to_json(value: YamlValue) -> JsonValue {
    match value {
        YamlValue::Null => JsonValue::Null,
        YamlValue::Bool(b) => JsonValue::Bool(b),
        YamlValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                JsonValue::from(i)
            } else if let Some(u) = n.as_u64() {
                JsonValue::from(u)
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(JsonValue::Number)
                    .unwrap_or(JsonValue::Null)
            } else {
                JsonValue::Null
            }
        }
        YamlValue::String(s) => JsonValue::String(s),
        YamlValue::Sequence(items) => JsonValue::Array(items.into_iter().map(yaml_to_json).collect()),
        YamlValue::Mapping(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                let key = match k {
                    YamlValue::String(s) => s,
                    other => serde_yaml::to_string(&other)
                        .map(|s| s.trim().to_string())
                        .unwrap_or_default(),
                };
                obj.insert(key, yaml_to_json(v));
            }
            JsonValue::Object(obj)
        }
        YamlValue::Tagged(tag) => yaml_to_json(tag.value),
    }
}

/// Pull the user-facing name + description out of parsed frontmatter,
/// falling back to the directory name when frontmatter is missing or empty.
/// Codex skills sometimes nest the short description under
/// `metadata.short-description`; we accept that as a secondary source.
pub fn extract_name_description(
    frontmatter: &JsonValue,
    fallback_name: &str,
) -> (String, Option<String>) {
    let name = frontmatter
        .get("name")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| fallback_name.to_string());

    let description = frontmatter
        .get("description")
        .and_then(JsonValue::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            frontmatter
                .get("metadata")
                .and_then(|m| m.get("short-description"))
                .and_then(JsonValue::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });

    (name, description)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_frontmatter_parses() {
        let content = "---\nname: foo\ndescription: A demo skill\n---\nBody here.\n";
        let parsed = parse_skill_file(content).unwrap();
        assert_eq!(parsed.frontmatter["name"], "foo");
        assert_eq!(parsed.frontmatter["description"], "A demo skill");
        assert_eq!(parsed.body, "Body here.\n");
    }

    #[test]
    fn folded_multiline_description() {
        let content = "---\nname: omarchy\ndescription: >\n  REQUIRED for\n  end-user customization\n  of Linux desktop\n---\nBody.";
        let parsed = parse_skill_file(content).unwrap();
        let desc = parsed.frontmatter["description"].as_str().unwrap();
        // Folded scalar collapses newlines into spaces.
        assert!(desc.contains("REQUIRED for end-user customization of Linux desktop"));
    }

    #[test]
    fn block_multiline_description() {
        let content = "---\nname: skill\ndescription: |\n  line one\n  line two\n---\nBody.";
        let parsed = parse_skill_file(content).unwrap();
        let desc = parsed.frontmatter["description"].as_str().unwrap();
        assert!(desc.contains("line one\nline two"));
    }

    #[test]
    fn nested_metadata_short_description_fallback() {
        let content =
            "---\nname: codex-skill\nmetadata:\n  short-description: Brief summary here\n---\nBody.";
        let parsed = parse_skill_file(content).unwrap();
        let (name, desc) = extract_name_description(&parsed.frontmatter, "fallback");
        assert_eq!(name, "codex-skill");
        assert_eq!(desc.as_deref(), Some("Brief summary here"));
    }

    #[test]
    fn no_frontmatter_returns_empty_obj_and_full_body() {
        let content = "Just a markdown body, no YAML.\n";
        let parsed = parse_skill_file(content).unwrap();
        assert!(parsed.frontmatter.is_object());
        assert_eq!(parsed.frontmatter.as_object().unwrap().len(), 0);
        assert_eq!(parsed.body, content);
    }

    #[test]
    fn no_frontmatter_then_fallback_name() {
        let content = "Body only.";
        let parsed = parse_skill_file(content).unwrap();
        let (name, desc) = extract_name_description(&parsed.frontmatter, "my-dir-name");
        assert_eq!(name, "my-dir-name");
        assert!(desc.is_none());
    }

    #[test]
    fn malformed_yaml_returns_error() {
        let content = "---\nname: foo\n: : :\ndescription:\n  - bad indent\n     more bad\n---\nBody.";
        let result = parse_skill_file(content);
        assert!(result.is_err());
    }

    #[test]
    fn missing_closing_delimiter_returns_error() {
        let content = "---\nname: foo\ndescription: bar\nNo closing delimiter.\n";
        let result = parse_skill_file(content);
        assert!(result.is_err());
    }

    #[test]
    fn unknown_fields_preserved_in_frontmatter() {
        let content = "---\nname: foo\nallowed-tools: [Bash, Read]\nversion: 2\nlicense: MIT\n---\nBody.";
        let parsed = parse_skill_file(content).unwrap();
        assert!(parsed.frontmatter["allowed-tools"].is_array());
        assert_eq!(parsed.frontmatter["version"], 2);
        assert_eq!(parsed.frontmatter["license"], "MIT");
    }

    #[test]
    fn bom_at_start_is_tolerated() {
        let content = "\u{feff}---\nname: foo\n---\nBody.";
        let parsed = parse_skill_file(content).unwrap();
        assert_eq!(parsed.frontmatter["name"], "foo");
    }

    #[test]
    fn crlf_line_endings_supported() {
        let content = "---\r\nname: foo\r\ndescription: bar\r\n---\r\nBody line.\r\n";
        let parsed = parse_skill_file(content).unwrap();
        assert_eq!(parsed.frontmatter["name"], "foo");
        assert_eq!(parsed.frontmatter["description"], "bar");
    }

    #[test]
    fn empty_string_name_falls_back_to_dir_name() {
        let content = "---\nname: \"\"\ndescription: x\n---\nBody.";
        let parsed = parse_skill_file(content).unwrap();
        let (name, _) = extract_name_description(&parsed.frontmatter, "dir-name");
        assert_eq!(name, "dir-name");
    }

    #[test]
    fn description_field_only_no_metadata_fallback() {
        let content = "---\nname: foo\ndescription: top-level\nmetadata:\n  short-description: nested\n---";
        let parsed = parse_skill_file(content).unwrap();
        let (_, desc) = extract_name_description(&parsed.frontmatter, "fallback");
        assert_eq!(desc.as_deref(), Some("top-level"));
    }
}
