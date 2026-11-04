//! `codemux connector …`: run the network door (the OAuth `/mcp` connector)
//! from a shell: turn it on or off, mint labelled tokens for headless
//! assistants, approve sign-ins and revoke access.
//!
//! These are admin socket commands, not registry tools: the owner at full
//! access runs them, and the socket refuses them to agent processes. A token
//! is never taken as an argument, so it never lands in shell history.

use super::args::{self, AccessArg, OutputArgs};
use super::door::{code, Door};
use super::output::{self, CliError, Exit};
use super::verbs;
use crate::agent_control::ControlAccess;
use clap::Subcommand;
use serde_json::{json, Value};

#[derive(Subcommand)]
pub enum ConnectorCommand {
    /// Show whether the network door is on, where it listens, and how many
    /// assistants are connected or waiting to sign in.
    Status {
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Turn the network door on. The public origin stays unless --origin or
    /// --no-origin is given.
    Enable {
        /// Listen on this loopback port.
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
        /// The HTTPS origin a proxy serves the door at, for other devices.
        #[arg(long, value_name = "URL", conflicts_with = "no_origin")]
        origin: Option<String>,
        /// Serve the door on this machine only.
        #[arg(long)]
        no_origin: bool,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Turn the network door off. Sign-ins and tokens are kept;
    /// `codemux connector revoke --all` ends them.
    Disable {
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Mint a labelled token for a headless assistant. It is shown once.
    /// Issuing again for a label replaces that label's token.
    Issue {
        /// A name for the assistant, e.g. hermes-bot.
        #[arg(long, value_name = "LABEL")]
        label: String,
        /// What the token may do.
        #[arg(long, value_enum, value_name = "LEVEL", default_value = "supervised")]
        access: AccessArg,
        /// How long the token lasts, e.g. 30d or 12h (default 30d, at most 365d).
        #[arg(long, value_name = "LIFETIME", value_parser = args::parse_expiry)]
        expires: Option<u32>,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// List connected assistants and tokens.
    List {
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Revoke a token by its label, or a connected assistant by its id.
    Revoke {
        /// A token label or a client id from `codemux connector list`.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        target: Option<String>,
        /// Revoke every sign-in and token.
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// List sign-ins waiting for approval.
    Pending {
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Approve a pending sign-in. Approve only a sign-in you just started.
    Approve {
        /// The sign-in's id from `codemux connector pending`.
        id: String,
        /// What the assistant may do.
        #[arg(long, value_enum, value_name = "LEVEL", default_value = "read-only")]
        access: AccessArg,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Deny a pending sign-in.
    Deny {
        /// The sign-in's id from `codemux connector pending`.
        id: String,
        #[command(flatten)]
        out: OutputArgs,
    },
}

/// The access value the socket takes.
fn wire(access: AccessArg) -> Value {
    json!(ControlAccess::from(access))
}

const FULL_ACCESS_WARNING: &str =
    "Full access lets the assistant run programs as you through the agents it starts. Prefer supervised for bots.";

pub(crate) async fn run(door: &Door, command: ConnectorCommand) -> Result<Exit, String> {
    match command {
        ConnectorCommand::Status { out } => {
            let outcome = door.command(verbs::CONNECTOR_STATUS, json!({})).await;
            output::report(verbs::CONNECTOR_STATUS, outcome, out.json, None, print_status)
        }
        ConnectorCommand::Enable { port, origin, no_origin, out } => {
            let outcome = async {
                let public_origin = match (origin, no_origin) {
                    (Some(origin), _) => json!(origin),
                    (None, true) => Value::Null,
                    (None, false) => current_origin(door).await?,
                };
                let mut params = json!({"enabled": true, "public_origin": public_origin});
                // Only when asked, so an app without the port setting still takes it.
                if let Some(port) = port {
                    params["port"] = json!(port);
                }
                door.command(verbs::CONNECTOR_SET_CONFIG, params).await
            }
            .await;
            output::report(verbs::CONNECTOR_SET_CONFIG, outcome, out.json, None, |status| {
                print_status(status)?;
                if let Some(revoked) = status["revokedGrants"].as_u64().filter(|revoked| *revoked > 0) {
                    output::note(&format!("Disconnected {revoked} assistants bound to the old address."));
                }
                Ok(())
            })
        }
        ConnectorCommand::Disable { out } => {
            let outcome = async {
                let public_origin = current_origin(door).await?;
                door.command(verbs::CONNECTOR_SET_CONFIG, json!({"enabled": false, "public_origin": public_origin})).await
            }
            .await;
            output::report(verbs::CONNECTOR_SET_CONFIG, outcome, out.json, None, |_| {
                output::out("Network door off. Assistants keep their sign-ins; `codemux connector revoke --all` ends them.")
            })
        }
        ConnectorCommand::Issue { label, access, expires, out } => {
            if access == AccessArg::FullAccess {
                output::note(FULL_ACCESS_WARNING);
            }
            let mut params = json!({"label": label, "access": wire(access)});
            // Only when asked, so the server's default lifetime applies otherwise.
            if let Some(hours) = expires {
                params["expires_in_hours"] = json!(hours);
            }
            let outcome = door.command(verbs::CONNECTOR_ISSUE, params).await;
            output::report(verbs::CONNECTOR_ISSUE, outcome, out.json, None, print_issued)
        }
        ConnectorCommand::List { out } => {
            let outcome = door.command(verbs::CONNECTOR_STATUS, json!({})).await;
            output::report(verbs::CONNECTOR_STATUS, outcome, out.json, None, |status| {
                let mut rows = vec![["ID", "KIND", "NAME", "ACCESS", "EXPIRES"].map(str::to_owned).to_vec()];
                for client in status["clients"].as_array().into_iter().flatten() {
                    let cell = |key: &str| client[key].as_str().unwrap_or_default().to_owned();
                    let kind = client["kind"].as_str().unwrap_or("oauth").to_owned();
                    let name = client["label"].as_str().map_or_else(|| cell("clientName"), str::to_owned);
                    rows.push(vec![cell("id"), kind, name, cell("access"), cell("expiresAt")]);
                }
                output::table(&rows)?;
                let pending = status["pending"].as_array().map_or(0, Vec::len);
                if pending > 0 {
                    output::note(&format!("{pending} sign-ins wait for approval: codemux connector pending"));
                }
                Ok(())
            })
        }
        ConnectorCommand::Revoke { target, all, out } => {
            if all {
                let outcome = door.command(verbs::CONNECTOR_REVOKE_ALL, json!({})).await;
                return output::report(verbs::CONNECTOR_REVOKE_ALL, outcome, out.json, None, |_| {
                    output::out("Revoked every sign-in and token.")
                });
            }
            let target = target.unwrap_or_default();
            let outcome = async {
                let status = door.command(verbs::CONNECTOR_STATUS, json!({})).await?;
                let client = resolve_client(&status, &target)?;
                door.command(verbs::CONNECTOR_REVOKE, json!({"client_id": client})).await?;
                Ok(json!({"client_id": client}))
            }
            .await;
            output::report(verbs::CONNECTOR_REVOKE, outcome, out.json, None, |revoked| {
                output::out(&format!("Revoked {}.", revoked["client_id"].as_str().unwrap_or(&target)))
            })
        }
        ConnectorCommand::Pending { out } => {
            let outcome = door.command(verbs::CONNECTOR_STATUS, json!({})).await.map(|status| json!({"pending": status["pending"]}));
            output::report(verbs::CONNECTOR_STATUS, outcome, out.json, None, |pending| {
                let mut rows = vec![["ID", "CLIENT", "CALLBACK", "EXPIRES"].map(str::to_owned).to_vec()];
                for request in pending["pending"].as_array().into_iter().flatten() {
                    let cell = |key: &str| request[key].as_str().unwrap_or("-").to_owned();
                    rows.push(vec![cell("id"), cell("clientName"), cell("callbackOrigin"), cell("expiresAt")]);
                }
                output::table(&rows)?;
                output::note("Approve only a sign-in you just started: codemux connector approve <ID>");
                Ok(())
            })
        }
        ConnectorCommand::Approve { id, access, out } => {
            if access == AccessArg::FullAccess {
                output::note(FULL_ACCESS_WARNING);
            }
            let outcome = door.command(verbs::CONNECTOR_APPROVE, json!({"request_id": id, "access": wire(access)})).await;
            output::report(verbs::CONNECTOR_APPROVE, outcome, out.json, None, |_| output::out("Approved."))
        }
        ConnectorCommand::Deny { id, out } => {
            let outcome = door.command(verbs::CONNECTOR_DENY, json!({"request_id": id})).await;
            output::report(verbs::CONNECTOR_DENY, outcome, out.json, None, |_| output::out("Denied."))
        }
    }
}

/// The configured public origin, kept across enable and disable.
async fn current_origin(door: &Door) -> Result<Value, CliError> {
    Ok(door.command(verbs::CONNECTOR_STATUS, json!({})).await?["publicOrigin"].clone())
}

/// A token's label (any case) first, then an exact client id. Revoking an
/// id that matches nothing would silently succeed, so it is refused here.
fn resolve_client(status: &Value, target: &str) -> Result<String, CliError> {
    let clients = status["clients"].as_array().cloned().unwrap_or_default();
    let by_label = clients.iter().find(|client| {
        client["kind"] == "token" && client["label"].as_str().is_some_and(|label| label.eq_ignore_ascii_case(target))
    });
    by_label
        .or_else(|| clients.iter().find(|client| client["id"] == target))
        .and_then(|client| client["id"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| {
            CliError::failed(code::INVALID_ARGUMENTS, format!("No connected assistant or token is called `{target}` (see codemux connector list)"))
        })
}

fn print_status(status: &Value) -> Result<(), String> {
    let text = |key: &str| status[key].as_str().map(str::to_owned);
    let on = status["enabled"] == true;
    output::out(&format!("Network door: {}", if on { "on" } else { "off" }))?;
    if on {
        let listening = status["listenerRunning"] == true;
        match (text("localUrl").or_else(|| text("mcpUrl")), listening) {
            (Some(url), true) => output::out(&format!("Listening:    {url}"))?,
            _ => output::out("Listening:    no")?,
        }
        if let Some(error) = text("bindError") {
            output::out(&format!("Bind error:   {error}"))?;
        }
        if let Some(url) = text("mcpUrl") {
            output::out(&format!("MCP URL:      {url}"))?;
        }
    }
    output::out(&format!("Public origin: {}", text("publicOrigin").unwrap_or_else(|| "none (this machine only)".into())))?;
    let count = |key: &str| status[key].as_array().map_or(0, Vec::len);
    output::out(&format!("Connected:    {}, pending sign-ins: {}", count("clients"), count("pending")))
}

fn print_issued(issued: &Value) -> Result<(), String> {
    let text = |key: &str| issued[key].as_str().unwrap_or_default().to_owned();
    let label = text("label");
    output::note(&format!(
        "Token for \"{label}\" ({}, expires {}). It is shown once; store it now:",
        text("access"),
        text("expiresAt")
    ));
    output::out(&text("token"))?;
    if issued["replaced"] == true {
        output::note(&format!("Replaced the previous token for \"{label}\"."));
    }
    let url = issued["mcpUrl"].as_str().or_else(|| issued["localUrl"].as_str()).unwrap_or("http://127.0.0.1:<port>/mcp");
    output::note(&format!("MCP URL:  {url}\nHeader:   Authorization: Bearer <the token above>\nRevoke:   codemux connector revoke {label}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, CommandSet};
    use clap::Parser;

    fn connector(argv: &[&str]) -> ConnectorCommand {
        match Cli::try_parse_from([&["codemux", "connector"][..], argv].concat()).expect("valid argv").command {
            Some(CommandSet::Connector { command }) => command,
            _ => panic!("expected connector"),
        }
    }

    fn status(origin: Value) -> Value {
        json!({"enabled": true, "publicOrigin": origin, "listenerRunning": true, "mcpUrl": "http://127.0.0.1:4378/mcp",
            "localCommand": "codemux mcp", "pending": [], "clients": [
                {"id": "cmcp_token_1", "clientName": "hermes-bot", "kind": "token", "label": "Hermes-Bot", "access": "supervised", "expiresAt": "x"},
                {"id": "hermes-bot", "clientName": "Claude", "kind": "oauth", "label": null, "access": "read_only", "expiresAt": "y"},
            ]})
    }

    #[tokio::test]
    async fn enable_keeps_the_origin_unless_asked_and_sends_a_port_only_when_given() {
        let door = Door::fake(vec![
            ("agent_connector_status", Ok(status(json!("https://desk.example")))),
            ("agent_connector_set_config", Ok(status(json!("https://desk.example")))),
            ("agent_connector_set_config", Ok(status(Value::Null))),
            ("agent_connector_set_config", Ok(status(json!("https://new.example")))),
        ]);
        run(&door, connector(&["enable"])).await.unwrap();
        run(&door, connector(&["enable", "--no-origin", "--port", "4400"])).await.unwrap();
        run(&door, connector(&["enable", "--origin", "https://new.example", "--json"])).await.unwrap();
        let calls = door.script().calls();
        assert_eq!(calls[1].1, json!({"enabled": true, "public_origin": "https://desk.example"}));
        assert_eq!(calls[2].1, json!({"enabled": true, "public_origin": null, "port": 4400}));
        assert_eq!(calls[3].1, json!({"enabled": true, "public_origin": "https://new.example"}));
        assert!(Cli::try_parse_from(["codemux", "connector", "enable", "--origin", "https://a", "--no-origin"]).is_err());
    }

    #[tokio::test]
    async fn disable_keeps_the_origin() {
        let door = Door::fake(vec![
            ("agent_connector_status", Ok(status(json!("https://desk.example")))),
            ("agent_connector_set_config", Ok(status(json!("https://desk.example")))),
        ]);
        assert_eq!(run(&door, connector(&["disable"])).await.unwrap(), Exit::Done);
        assert_eq!(door.script().calls()[1].1, json!({"enabled": false, "public_origin": "https://desk.example"}));
    }

    #[tokio::test]
    async fn issue_defaults_to_supervised_and_sends_a_lifetime_only_when_given() {
        let issued = || Ok(json!({"token": "cmcp_x", "clientId": "cmcp_token_1", "label": "bot", "access": "supervised", "expiresAt": "z", "mcpUrl": "u", "localUrl": "u", "replaced": false}));
        let door = Door::fake(vec![("agent_connector_issue", issued()), ("agent_connector_issue", issued())]);
        run(&door, connector(&["issue", "--label", "bot", "--json"])).await.unwrap();
        run(&door, connector(&["issue", "--label", "bot", "--access", "full", "--expires", "2d", "--json"])).await.unwrap();
        let calls = door.script().calls();
        assert_eq!(calls[0].1, json!({"label": "bot", "access": "supervised"}));
        assert_eq!(calls[1].1, json!({"label": "bot", "access": "full_access", "expires_in_hours": 48}));
        assert!(Cli::try_parse_from(["codemux", "connector", "issue", "--label", "b", "--expires", "400d"]).is_err());
    }

    #[tokio::test]
    async fn revoke_resolves_a_token_label_before_a_client_id() {
        let door = Door::fake(vec![
            ("agent_connector_status", Ok(status(Value::Null))),
            ("agent_connector_revoke", Ok(Value::Null)),
            ("agent_connector_status", Ok(status(Value::Null))),
            ("agent_connector_revoke", Ok(Value::Null)),
            ("agent_connector_status", Ok(status(Value::Null))),
            ("agent_connector_revoke_all", Ok(Value::Null)),
        ]);
        // "hermes-bot" is both a token's label (any case) and an OAuth client's id: the label wins.
        run(&door, connector(&["revoke", "hermes-bot"])).await.unwrap();
        run(&door, connector(&["revoke", "cmcp_token_1"])).await.unwrap();
        assert_eq!(run(&door, connector(&["revoke", "nobody", "--json"])).await.unwrap(), Exit::Failed);
        run(&door, connector(&["revoke", "--all"])).await.unwrap();
        let calls = door.script().calls();
        assert_eq!(calls[1].1, json!({"client_id": "cmcp_token_1"}));
        assert_eq!(calls[3].1, json!({"client_id": "cmcp_token_1"}));
        assert_eq!(door.script().names().last().map(String::as_str), Some("agent_connector_revoke_all"));
        assert!(Cli::try_parse_from(["codemux", "connector", "revoke"]).is_err());
        assert!(Cli::try_parse_from(["codemux", "connector", "revoke", "x", "--all"]).is_err());
    }

    #[tokio::test]
    async fn approve_defaults_to_read_only() {
        let door = Door::fake(vec![("agent_connector_approve", Ok(Value::Null)), ("agent_connector_deny", Ok(Value::Null))]);
        run(&door, connector(&["approve", "req-1"])).await.unwrap();
        run(&door, connector(&["deny", "req-2"])).await.unwrap();
        let calls = door.script().calls();
        assert_eq!(calls[0].1, json!({"request_id": "req-1", "access": "read_only"}));
        assert_eq!(calls[1].1, json!({"request_id": "req-2"}));
    }
}
