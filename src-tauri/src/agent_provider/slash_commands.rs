//! Slash syntax shared by native adapters. A path or an inline slash is prose.

pub fn leading_command(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start().strip_prefix('/')?;
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    let name = &text[..end];
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':' | '.'))
    {
        return None;
    }
    Some((name, text[end..].trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_command_syntax_preserves_arguments_and_rejects_paths() {
        assert_eq!(
            leading_command("  /plugin:review first\nsecond"),
            Some(("plugin:review", "first\nsecond"))
        );
        for text in [
            "/",
            "hello /review",
            "/home/user/file",
            "https://host/review",
        ] {
            assert_eq!(leading_command(text), None);
        }
    }
}
