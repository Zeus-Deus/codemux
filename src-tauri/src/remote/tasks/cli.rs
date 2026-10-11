//! Typed CLI boundary: one bounded JSON request on stdin, one JSON reply on
//! stdout. State-directory selection is an operator flag, never a task field.
use std::io::Read;
use std::path::PathBuf;

#[derive(clap::Subcommand)]
pub enum TaskCommand {
    Capabilities {
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    Launch {
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    Read {
        #[arg(long)]
        id: String,
        #[arg(long, default_value_t = 0)]
        after: i64,
        #[arg(long, default_value_t = 0)]
        wait_ms: u64,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    Cancel {
        #[arg(long)]
        id: String,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    Respond {
        #[arg(long)]
        id: String,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    #[command(hide = true)]
    Run {
        #[arg(long)]
        id: String,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
}
fn input<T: serde::de::DeserializeOwned>() -> Result<T, String> {
    let mut bytes = vec![];
    std::io::stdin()
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65_536 {
        return Err("task JSON request exceeds 64 KiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("invalid task JSON: {e}"))
}
fn root(path: Option<PathBuf>) -> Result<PathBuf, String> {
    let root = path.unwrap_or_else(super::super::config::default_state_dir);
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    root.canonicalize()
        .map_err(|e| format!("task state directory: {e}"))
}
pub async fn dispatch(command: TaskCommand) -> Result<Option<serde_json::Value>, String> {
    let value = match command {
        TaskCommand::Capabilities { state_dir } => {
            serde_json::to_value(super::capabilities(&root(state_dir)?).await?)
        }
        TaskCommand::Launch { state_dir } => {
            serde_json::to_value(super::launch(&root(state_dir)?, input()?).await?)
        }
        TaskCommand::Read {
            id,
            after,
            wait_ms,
            state_dir,
        } => serde_json::to_value(super::read(&root(state_dir)?, &id, after, wait_ms).await?),
        TaskCommand::Cancel { id, state_dir } => {
            serde_json::to_value(super::cancel(&root(state_dir)?, &id)?)
        }
        TaskCommand::Respond { id, state_dir } => {
            serde_json::to_value(super::respond(&root(state_dir)?, &id, input()?)?)
        }
        TaskCommand::Run { id, state_dir } => {
            super::run(&root(state_dir)?, &id).await?;
            return Ok(None);
        }
    }
    .map_err(|e| e.to_string())?;
    Ok(Some(value))
}
