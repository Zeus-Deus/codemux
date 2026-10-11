//! Local launch definitions. Environment values never appear in listing DTOs.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Serialize, Deserialize)]
pub struct AcpAgent {
    pub id: String,
    pub name: String,
    pub executable: String,
    pub args: Vec<String>,
    pub environment: BTreeMap<String, Option<String>>,
    pub enabled: bool,
    pub auth_method: Option<String>,
    pub revision: String,
}

#[derive(Clone, Deserialize)]
pub struct AcpAgentInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, Option<String>>,
    #[serde(default = "enabled_default")]
    pub enabled: bool,
    #[serde(default)]
    pub auth_method: Option<String>,
}

fn enabled_default() -> bool { true }

#[derive(Clone)]
pub struct AcpLaunchConfig {
    pub agent: AcpAgent,
    pub environment: HashMap<String, String>,
}

impl AcpAgentInput {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.name.len() > 128 || self.name.chars().any(char::is_control) {
            return Err("A name of at most 128 characters is required.".into());
        }
        if self.executable.trim().is_empty() || self.executable.len() > 4096 || self.executable.chars().any(char::is_control) {
            return Err("A valid executable name or path is required.".into());
        }
        if self.args.len() > 256 || self.args.iter().any(|arg| arg.len() > 16384 || arg.contains('\0')) {
            return Err("Arguments must be literal strings without NUL characters; at most 256 arguments are allowed.".into());
        }
        if self.environment.len() > 128 {
            return Err("At most 128 environment variables are allowed.".into());
        }
        for (name, value) in &self.environment {
            let mut bytes = name.bytes();
            if name.len() > 128
                || !bytes.next().is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
                || !bytes.all(|c| c.is_ascii_alphanumeric() || c == b'_')
            {
                return Err("Environment names must start with a letter or underscore and contain only letters, numbers, or underscores.".into());
            }
            if value.as_ref().is_some_and(|value| value.len() > 65536 || value.contains('\0')) {
                return Err("Environment values must not contain NUL characters or exceed 65536 bytes.".into());
            }
        }
        if self.args.iter().map(String::len).sum::<usize>()
            + self.environment.iter().map(|(name, value)| name.len() + value.as_ref().map_or(0, String::len)).sum::<usize>() > 262144
        {
            return Err("The configured arguments and environment exceed the launch size limit.".into());
        }
        if self.auth_method.as_ref().is_some_and(|id| id.is_empty() || id.len() > 256 || id.chars().any(char::is_control)) {
            return Err("Use an advertised authentication method ID, or leave it unset.".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> AcpAgentInput {
        AcpAgentInput { id: None, name: "Local harness".into(), executable: "dsh".into(), args: vec!["--profile".into(), "acp".into()], environment: BTreeMap::new(), enabled: true, auth_method: None }
    }
    #[test]
    fn definition_requires_a_name_and_an_executable() {
        let mut value = input(); value.name = "  ".into();
        assert!(value.validate().is_err());
        value = input(); value.executable = "".into();
        assert!(value.validate().is_err());
    }
    #[test]
    fn launch_values_reject_nul_without_revealing_environment_values() {
        let mut value = input(); value.environment.insert("TOKEN".into(), Some("private\0value".into()));
        let error = value.validate().expect_err("NUL is not executable environment data");
        assert!(!error.contains("private"));
        value = input(); value.args.push("bad\0argument".into());
        assert!(value.validate().is_err());
    }
    #[test]
    fn arguments_and_environment_values_are_literal_not_shell_expressions() {
        let mut value = input(); value.args = vec!["".into(), " spaced ".into(), "$(touch should-not-run); $VALUE".into(), "[\"provider\",\"model\"]".into()];
        value.environment.insert("VALUE".into(), Some(" literal $(value) ".into()));
        value.validate().unwrap();
        assert_eq!(value.args[1], " spaced ");
        assert_eq!(value.environment["VALUE"].as_deref(), Some(" literal $(value) "));
    }
    #[test]
    fn invalid_environment_keys_and_unbounded_definitions_are_rejected() {
        let mut value = input(); value.environment.insert("bad=key".into(), Some("value".into()));
        assert!(value.validate().is_err());
        value = input(); value.args = vec!["argument".into(); 257];
        assert!(value.validate().is_err());
    }
}
