use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Args)]
pub struct PortsArgs {
    #[command(subcommand)]
    pub command: PortsCommand,
}

#[derive(Subcommand)]
pub enum PortsCommand {
    /// Allocate a deterministic free port for this worktree + name
    Allocate(AllocateArgs),
    /// Show ports owned by the current worktree
    List,
    /// Free a previously allocated port
    Release(ReleaseArgs),
}

#[derive(Args)]
struct AllocateArgs {
    name: String,
}

#[derive(Args)]
struct ReleaseArgs {
    name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct PortStore {
    next_port: u16,
    allocations: std::collections::HashMap<String, u16>,
}

impl PortStore {
    fn new() -> Self {
        Self {
            next_port: 10000,
            allocations: std::collections::HashMap::new(),
        }
    }

    fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(Self::new)
    }

    fn save(&self, path: &Path) {
        if let Ok(dir) = path.parent() {
            fs::create_dir_all(dir).ok();
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            fs::write(path, json).ok();
        }
    }
}

fn storage_path() -> PathBuf {
    dirs::app_data_dir("codemux")
        .unwrap_or_else(|| PathBuf::from("./.codemux"))
        .join("ports.json")
}

static STORE: Mutex<PortStore> = Mutex::new(PortStore::new());

fn get_store() -> std::sync::MutexGuard<'static, PortStore> {
    let mut store = STORE.lock().unwrap();
    if store.allocations.is_empty() && store.next_port == 10000 {
        *store = PortStore::load(&storage_path());
    }
    store
}

fn find_free_port(used: &std::collections::HashMap<String, u16>, start: u16) -> u16 {
    let mut port = start;
    while used.values().any(|p| *p == port) {
        port += 1;
        if port > 65535 {
            panic!("No free ports available in range");
        }
    }
    port
}

pub fn run(args: PortsArgs) {
    match args.command {
        PortsCommand::Allocate(AllocateArgs { name }) => {
            let mut store = get_store();
            let port = if let Some(&existing) = store.allocations.get(&name) {
                existing
            } else {
                let port = find_free_port(&store.allocations, store.next_port);
                store.next_port = port + 1;
                store.allocations.insert(name.clone(), port);
                store.save(&storage_path());
                port
            };
            println!("{}", port);
        }
        PortsCommand::List => {
            let store = get_store();
            if store.allocations.is_empty() {
                println!("No ports allocated.");
            } else {
                println!("Allocated ports for this worktree:");
                for (name, port) in &store.allocations {
                    println!("  {} -> {}", name, port);
                }
            }
        }
        PortsCommand::Release(ReleaseArgs { name }) => {
            let mut store = get_store();
            if store.allocations.remove(&name).is_some() {
                store.save(&storage_path());
                println!("Released port for '{}'.", name);
            } else {
                eprintln!("No allocation found for '{}'.", name);
                std::process::exit(1);
            }
        }
    }
}
