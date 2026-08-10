//! `codemux ports` — deterministic, collision-free host ports per worktree.
//!
//! Every worktree of a project shares one host network, so the first stack
//! that publishes 4200/8000/5432 wins and every other worktree collides. The
//! usual workarounds are bad: stopping someone else's stack interrupts them,
//! and picking a "random" high port is a coin flip that two worktrees can
//! lose at the same time.
//!
//! This command hands a worktree its own port for a given name and remembers
//! it, so `codemux ports allocate web` returns the same number on every run
//! for the lifetime of that worktree — an ephemeral compose file written once
//! keeps working across restarts. Allocation is aware of every worktree on
//! the machine, so no two of them are ever handed the same number.
//!
//! State lives in one JSON file under the app data dir, sectioned by worktree
//! root. Each CLI invocation is its own process, so the load → mutate → save
//! cycle runs under an on-disk lock and the file is replaced by atomic
//! rename; two concurrent `allocate` calls can neither hand out the same port
//! nor lose each other's writes.

use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// First port considered by the allocator. Chosen to sit above the common
/// dev-server defaults (3000/4200/5173/8000/8080) and below the Linux
/// ephemeral range (32768–60999), so an allocation can't be stolen mid-run
/// by an outbound connection's source port.
const BASE_PORT: u16 = 10000;

/// Last port considered by the allocator — one below the start of the Linux
/// ephemeral range.
const MAX_PORT: u16 = 32767;

/// Bumped only if the on-disk shape changes in a way older builds can't read.
const SCHEMA_VERSION: u32 = 1;

/// How long to wait for another process to release the store lock before
/// giving up. Allocation is a few milliseconds of work, so anything near
/// this bound means a wedged process, not contention.
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// A lock file older than this is assumed to be left over from a process
/// that was killed before it could clean up, and is removed.
const STALE_LOCK_AFTER: Duration = Duration::from_secs(30);

/// Poll interval while waiting on the lock.
const LOCK_POLL: Duration = Duration::from_millis(25);

#[derive(Subcommand)]
pub enum PortsCommand {
    /// Reserve a free host port for this worktree under `<name>`, and print
    /// it. Calling it again with the same name in the same worktree always
    /// prints the same port, so a compose file written against it keeps
    /// working across restarts.
    Allocate {
        /// Short label for the service, e.g. `web`, `api`, `db`.
        name: String,
    },
    /// Print the ports this worktree owns, one `name<TAB>port` line each.
    List,
    /// Give a previously allocated port back so another worktree can use it.
    /// Safe to run when nothing is allocated under that name.
    Release {
        /// The name the port was allocated under.
        name: String,
    },
}

/// The persisted allocation table.
///
/// Keyed by worktree root first and name second — that pairing is the whole
/// point. Keying by name alone (the shape this replaced) hands two worktrees
/// asking for "web" the same port, which is the collision the command exists
/// to prevent.
#[derive(Debug, Serialize, Deserialize)]
struct PortStore {
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    /// worktree root → (name → port)
    #[serde(default)]
    worktrees: BTreeMap<String, BTreeMap<String, u16>>,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

impl Default for PortStore {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            worktrees: BTreeMap::new(),
        }
    }
}

/// What [`PortStore::load`] does with a file it cannot parse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OnCorruption {
    /// Move the unreadable file aside and carry on with an empty store.
    ///
    /// Only correct while holding the store lock. Unlocked, the rename can
    /// destroy good data: a concurrent writer may already have recovered from
    /// the same corruption and written a valid store, and renaming *that*
    /// aside would discard every live reservation.
    MoveAside,
    /// Report the corruption and carry on with an empty store, leaving the
    /// file untouched for whoever holds the lock to deal with.
    Report,
}

impl PortStore {
    /// Read the store, falling back to an empty one when the file is missing
    /// or unreadable.
    ///
    /// A corrupt file is recovered from rather than fatal: the store is a
    /// cache of reservations, and refusing to allocate because a half-written
    /// JSON file exists would leave an agent with no way forward.
    fn load(path: &Path, on_corruption: OnCorruption) -> Self {
        let Ok(raw) = fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str(&raw) {
            Ok(store) => store,
            Err(error) => {
                match on_corruption {
                    OnCorruption::MoveAside => {
                        let backup = path.with_extension("json.corrupt");
                        let _ = fs::rename(path, &backup);
                        eprintln!(
                            "[codemux::ports] {} is not valid JSON ({error}); starting a fresh store, previous contents kept at {}",
                            path.display(),
                            backup.display()
                        );
                    }
                    OnCorruption::Report => eprintln!(
                        "[codemux::ports] {} is not valid JSON ({error}); reporting an empty store",
                        path.display()
                    ),
                }
                Self::default()
            }
        }
    }

    /// Replace the store on disk atomically: write a sibling temp file, flush
    /// it, then rename over the target. A reader can only ever observe the
    /// old file or the new one, never a partial write.
    fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| format!("cannot serialize the port store: {error}"))?;
        let temp = path.with_extension(format!("json.tmp.{}", std::process::id()));
        let write = (|| -> std::io::Result<()> {
            let mut file = fs::File::create(&temp)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()
        })();
        if let Err(error) = write {
            let _ = fs::remove_file(&temp);
            return Err(format!("cannot write {}: {error}", temp.display()));
        }
        fs::rename(&temp, path).map_err(|error| {
            let _ = fs::remove_file(&temp);
            format!("cannot replace {}: {error}", path.display())
        })
    }

    /// Every port held by any worktree. The allocator skips all of these, not
    /// just this worktree's, so two worktrees never land on the same number.
    fn reserved_ports(&self) -> HashSet<u16> {
        self.worktrees
            .values()
            .flat_map(|names| names.values().copied())
            .collect()
    }

    fn ports_for(&self, worktree: &str) -> Option<&BTreeMap<String, u16>> {
        self.worktrees.get(worktree)
    }

    /// Reserve a port for `(worktree, name)`.
    ///
    /// An existing reservation is returned as-is and never re-probed: the
    /// store records that this worktree owns the port, and a bind test would
    /// report it busy precisely when the worktree's own stack is up — which
    /// would move the port out from under a running service and break the
    /// determinism the command promises.
    ///
    /// A fresh reservation scans upward from `base`, skipping ports held by
    /// any worktree and ports that fail a real bind test, so a service that
    /// Codemux doesn't track can't be trampled either.
    ///
    /// Returns the port and whether the store changed.
    fn allocate(
        &mut self,
        worktree: &str,
        name: &str,
        base: u16,
        max: u16,
        is_free: &dyn Fn(u16) -> bool,
    ) -> Result<(u16, bool), String> {
        if let Some(port) = self.ports_for(worktree).and_then(|names| names.get(name)) {
            return Ok((*port, false));
        }
        let reserved = self.reserved_ports();
        // An inclusive range terminates correctly at u16::MAX, so there is no
        // counter to overflow here.
        for port in base..=max {
            if reserved.contains(&port) || !is_free(port) {
                continue;
            }
            self.worktrees
                .entry(worktree.to_string())
                .or_default()
                .insert(name.to_string(), port);
            return Ok((port, true));
        }
        Err(format!(
            "no free port in {base}-{max}; free one with `codemux ports release <name>`"
        ))
    }

    /// Drop a reservation. Returns the port that was freed, if there was one.
    fn release(&mut self, worktree: &str, name: &str) -> Option<u16> {
        let names = self.worktrees.get_mut(worktree)?;
        let port = names.remove(name);
        if names.is_empty() {
            self.worktrees.remove(worktree);
        }
        port
    }
}

/// An advisory lock held for the length of a load → mutate → save cycle.
///
/// `create_new` is atomic on every platform we ship, which is enough for a
/// mutual exclusion primitive without pulling in a locking crate. Each holder
/// writes a token that is unique to the acquisition, so a lock file can always
/// be traced back to the process that created it — that identity is what makes
/// releasing and reclaiming safe rather than "whatever is at this path".
struct StoreLock {
    path: PathBuf,
    token: String,
}

impl StoreLock {
    fn acquire(path: PathBuf) -> Result<Self, String> {
        Self::acquire_with(path, LOCK_TIMEOUT, STALE_LOCK_AFTER)
    }

    fn acquire_with(
        path: PathBuf,
        timeout: Duration,
        stale_after: Duration,
    ) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        }
        // The pid makes a wedged holder identifiable by hand; the uuid makes
        // the token unique even against a recycled pid or a second attempt
        // from this same process.
        let token = format!("{}\n{}", std::process::id(), uuid::Uuid::new_v4());
        let deadline = Instant::now() + timeout;
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    file.write_all(token.as_bytes())
                        .and_then(|()| file.sync_all())
                        .map_err(|error| {
                            format!("cannot write the lock at {}: {error}", path.display())
                        })?;
                    return Ok(Self { path, token });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Read the holder before judging staleness so the reclaim
                    // can prove it is moving *this* file and not a fresh lock
                    // that replaced it in the meantime.
                    let holder = fs::read_to_string(&path).unwrap_or_default();
                    if lock_is_stale(&path, stale_after) && reclaim_stale_lock(&path, &holder) {
                        continue;
                    }
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "timed out waiting for the port store lock at {}; remove it if no codemux process is running",
                            path.display()
                        ));
                    }
                    std::thread::sleep(LOCK_POLL);
                }
                Err(error) => {
                    return Err(format!("cannot lock {}: {error}", path.display()));
                }
            }
        }
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        // Remove the file only while it still carries our token. Deleting by
        // path alone is what turns one reclaim race into a cascade: a process
        // whose lock was reclaimed would drop the *next* holder's lock on its
        // way out, and so on.
        if fs::read_to_string(&self.path).unwrap_or_default() == self.token {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Move a stale lock out of the way so acquisition can retry. Returns whether
/// the caller should retry immediately.
///
/// The move is a `rename` to a name only this call knows. Renaming is atomic,
/// so of several processes that judged the same lock stale exactly one can win
/// it; the losers see their rename fail and go back to waiting. Removing the
/// path directly instead — the obvious version — lets every one of them
/// "succeed", and a loser's delete then lands on the *fresh* lock the winner
/// has already created, putting two processes inside the critical section at
/// once.
///
/// Winning the rename is still not proof that the file moved is the stale one:
/// an earlier winner may have reclaimed and replaced it between the staleness
/// check and the rename. The moved file is therefore matched against the
/// holder token observed as stale, and put back untouched when it turns out to
/// belong to somebody alive.
fn reclaim_stale_lock(path: &Path, observed_holder: &str) -> bool {
    let mut reclaimed = path.as_os_str().to_owned();
    reclaimed.push(format!(".reclaim.{}", uuid::Uuid::new_v4()));
    let reclaimed = PathBuf::from(reclaimed);

    if fs::rename(path, &reclaimed).is_err() {
        // Someone else reclaimed it first (or it was released normally).
        return false;
    }
    if fs::read_to_string(&reclaimed).unwrap_or_default() == observed_holder {
        let _ = fs::remove_file(&reclaimed);
        return true;
    }

    // A live lock, created after the observation. Put it back with
    // `hard_link`, which fails if the path exists rather than overwriting the
    // way a second `rename` would.
    match fs::hard_link(&reclaimed, path) {
        Ok(()) => {
            let _ = fs::remove_file(&reclaimed);
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // A lock reappeared while ours was moved aside; that one is live
            // and the copy in hand is redundant.
            let _ = fs::remove_file(&reclaimed);
        }
        Err(_) => {
            // A filesystem without hard links. Fall back to renaming it back,
            // which restores the same bytes minus the exclusivity guarantee.
            let _ = fs::rename(&reclaimed, path);
        }
    }
    false
}

fn lock_is_stale(path: &Path, stale_after: Duration) -> bool {
    let Ok(modified) = fs::metadata(path).and_then(|meta| meta.modified()) else {
        // No readable timestamp means we can't prove the holder is alive;
        // treat it as stale rather than blocking forever.
        return true;
    };
    modified
        .elapsed()
        .map(|age| age >= stale_after)
        .unwrap_or(false)
}

/// Where the allocation table lives. Deliberately not named `ports.json`:
/// that name already belongs to the per-workspace static-port file read by
/// `crate::ports::load_static_ports`.
fn store_path() -> PathBuf {
    let root = dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"));
    root.join(crate::APP_DIR_NAME).join("port_allocations.json")
}

fn lock_path(store: &Path) -> PathBuf {
    store.with_extension("json.lock")
}

/// Identify the caller's worktree by its root directory.
///
/// `git rev-parse --show-toplevel` reports the root of the *linked* worktree,
/// not the main repository, which is exactly the granularity we want: two
/// worktrees of one project are two different keys. Outside a repository the
/// canonical working directory stands in, so the command still works in a
/// plain directory.
fn worktree_key(cwd: &Path) -> String {
    let toplevel = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            (!path.is_empty()).then(|| PathBuf::from(path))
        });
    let root = toplevel.unwrap_or_else(|| cwd.to_path_buf());
    canonical_string(&root)
}

fn canonical_string(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// A port counts as free only when it binds on both the loopback and the
/// wildcard address: container stacks publish on `0.0.0.0` while a plain dev
/// server often binds `127.0.0.1` only, and either one occupying the port
/// makes it unusable for the other. Mirrors the bind test the browser-stream
/// allocator uses in `agent_browser::AgentBrowserManager::allocate_port`.
fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
        && std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn normalize_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("port name cannot be empty".to_string());
    }
    Ok(trimmed.to_string())
}

pub fn run(command: PortsCommand) -> Result<(), String> {
    let cwd = std::env::current_dir()
        .map_err(|error| format!("cannot read the current directory: {error}"))?;
    let worktree = worktree_key(&cwd);
    let path = store_path();

    match command {
        PortsCommand::Allocate { name } => {
            let name = normalize_name(&name)?;
            let _lock = StoreLock::acquire(lock_path(&path))?;
            let mut store = PortStore::load(&path, OnCorruption::MoveAside);
            let (port, changed) =
                store.allocate(&worktree, &name, BASE_PORT, MAX_PORT, &port_is_free)?;
            if changed {
                store.save(&path)?;
            }
            // Bare number on stdout so `$(codemux ports allocate web)` is
            // directly usable in a compose file or an env var.
            println!("{port}");
        }
        PortsCommand::List => {
            // Read-only, so no lock is needed: the atomic rename in `save`
            // means a reader sees either the old file or the new one. Without
            // the lock it must not move a corrupt file aside either — see
            // `OnCorruption::MoveAside`.
            let store = PortStore::load(&path, OnCorruption::Report);
            eprintln!("worktree: {worktree}");
            match store.ports_for(&worktree) {
                Some(names) if !names.is_empty() => {
                    for (name, port) in names {
                        println!("{name}\t{port}");
                    }
                }
                _ => eprintln!("no ports allocated for this worktree"),
            }
        }
        PortsCommand::Release { name } => {
            let name = normalize_name(&name)?;
            let _lock = StoreLock::acquire(lock_path(&path))?;
            let mut store = PortStore::load(&path, OnCorruption::MoveAside);
            // Releasing something that isn't allocated is a success, not an
            // error: cleanup usually runs from a trap or a teardown step that
            // can't know whether the allocation happened.
            match store.release(&worktree, &name) {
                Some(port) => {
                    store.save(&path)?;
                    eprintln!("released {name} (port {port})");
                }
                None => eprintln!("no port allocated for {name} in this worktree"),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKTREE_A: &str = "/tmp/project/worktree-a";
    const WORKTREE_B: &str = "/tmp/project/worktree-b";

    /// Test double for the bind test: every port looks free.
    fn all_free(_port: u16) -> bool {
        true
    }

    fn allocate(store: &mut PortStore, worktree: &str, name: &str) -> u16 {
        store
            .allocate(worktree, name, 10000, 10100, &all_free)
            .expect("allocation succeeds")
            .0
    }

    /// The core promise: same worktree + same name → same port, forever.
    /// Re-running the command must not move a port out from under a compose
    /// file that already references it.
    #[test]
    fn allocate_is_deterministic_per_worktree_and_name() {
        let mut store = PortStore::default();
        let first = allocate(&mut store, WORKTREE_A, "web");
        let second = allocate(&mut store, WORKTREE_A, "web");
        assert_eq!(first, second);

        // And across a save/load round trip, which is what "survives an agent
        // restart" actually means.
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("port_allocations.json");
        store.save(&path).expect("store saves");
        let mut reloaded = PortStore::load(&path, OnCorruption::MoveAside);
        assert_eq!(allocate(&mut reloaded, WORKTREE_A, "web"), first);
    }

    /// The regression this command exists to prevent: two worktrees both
    /// asking for "web" must not be handed the same port.
    #[test]
    fn allocate_never_repeats_a_port_across_worktrees() {
        let mut store = PortStore::default();
        let a = allocate(&mut store, WORKTREE_A, "web");
        let b = allocate(&mut store, WORKTREE_B, "web");
        assert_ne!(a, b, "two worktrees were handed the same port");

        // Same for a third name in a third worktree — every handed-out port
        // is distinct.
        let c = allocate(&mut store, "/tmp/project/worktree-c", "web");
        let mut all = vec![a, b, c];
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 3);
    }

    /// Different names inside one worktree get their own ports too.
    #[test]
    fn allocate_gives_each_name_its_own_port() {
        let mut store = PortStore::default();
        let web = allocate(&mut store, WORKTREE_A, "web");
        let api = allocate(&mut store, WORKTREE_A, "api");
        assert_ne!(web, api);
    }

    /// After a release the port returns to the pool, and the next allocation
    /// in another worktree can take it.
    #[test]
    fn release_returns_the_port_to_the_pool() {
        let mut store = PortStore::default();
        let web = allocate(&mut store, WORKTREE_A, "web");
        assert_eq!(store.release(WORKTREE_A, "web"), Some(web));
        assert!(store.ports_for(WORKTREE_A).is_none());
        assert_eq!(allocate(&mut store, WORKTREE_B, "web"), web);
    }

    /// Releasing an unknown name is a no-op, so teardown scripts can run
    /// unconditionally.
    #[test]
    fn release_of_an_unknown_name_is_a_no_op() {
        let mut store = PortStore::default();
        assert_eq!(store.release(WORKTREE_A, "web"), None);
        allocate(&mut store, WORKTREE_A, "web");
        assert_eq!(store.release(WORKTREE_A, "api"), None);
        assert!(store.ports_for(WORKTREE_A).is_some(), "web survives");
    }

    /// A port that something else is actively listening on must be skipped,
    /// even though no worktree in the store claims it. Uses the real bind
    /// test, so a regression to store-only bookkeeping fails here.
    #[test]
    fn allocate_skips_ports_that_are_bound_by_another_process() {
        let blocker =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind an ephemeral port");
        let blocked = blocker.local_addr().expect("local addr").port();
        let mut store = PortStore::default();
        let (port, _) = store
            .allocate(
                WORKTREE_A,
                "web",
                blocked,
                blocked.saturating_add(64),
                &port_is_free,
            )
            .expect("some port in the window is free");
        assert_ne!(
            port, blocked,
            "allocator handed out a port we are actively listening on"
        );
        drop(blocker);
    }

    /// Exhausting the window is an error, not a panic and not a wrapped
    /// counter.
    #[test]
    fn allocate_errors_when_the_window_is_exhausted() {
        let mut store = PortStore::default();
        let err = store
            .allocate(WORKTREE_A, "web", 20000, 20000, &|_| false)
            .expect_err("nothing is free");
        assert!(err.contains("no free port"), "unexpected message: {err}");
    }

    /// The top of the u16 range must terminate rather than overflow the
    /// candidate counter.
    #[test]
    fn allocate_terminates_at_the_top_of_the_port_range() {
        let mut store = PortStore::default();
        let err = store
            .allocate(WORKTREE_A, "web", u16::MAX - 1, u16::MAX, &|_| false)
            .expect_err("nothing is free");
        assert!(err.contains("no free port"), "unexpected message: {err}");
    }

    /// A truncated or hand-mangled store must not wedge allocation. Under the
    /// lock the bad file is moved aside so it is still inspectable.
    #[test]
    fn a_corrupt_store_recovers_instead_of_failing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("port_allocations.json");
        fs::write(&path, "{ \"worktrees\": { truncated").expect("write junk");

        let mut store = PortStore::load(&path, OnCorruption::MoveAside);
        assert!(store.worktrees.is_empty());
        assert!(
            path.with_extension("json.corrupt").exists(),
            "the unreadable file is kept for inspection"
        );

        // And allocation carries on normally from the fresh store.
        allocate(&mut store, WORKTREE_A, "web");
        store.save(&path).expect("store saves");
        assert_eq!(
            PortStore::load(&path, OnCorruption::MoveAside)
                .ports_for(WORKTREE_A)
                .unwrap()
                .len(),
            1
        );
    }

    /// The unlocked reader (`ports list`) must never move a file aside. It can
    /// only be looking at bytes it read without the lock, so the file it would
    /// rename may already have been replaced by a valid store that a locked
    /// writer recovered — renaming that away would drop every reservation.
    #[test]
    fn a_corrupt_store_is_left_in_place_without_the_lock() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("port_allocations.json");
        fs::write(&path, "{ \"worktrees\": { truncated").expect("write junk");

        let store = PortStore::load(&path, OnCorruption::Report);
        assert!(store.worktrees.is_empty(), "reports an empty store");
        assert!(path.exists(), "the store file is left alone");
        assert!(
            !path.with_extension("json.corrupt").exists(),
            "an unlocked reader must not move the store aside"
        );
    }

    /// A missing store is simply an empty one — first run must not error.
    #[test]
    fn a_missing_store_loads_as_empty() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = PortStore::load(&dir.path().join("nope.json"), OnCorruption::MoveAside);
        assert!(store.worktrees.is_empty());
        assert_eq!(store.schema_version, SCHEMA_VERSION);
    }

    /// Saving creates the parent directory and leaves no temp file behind.
    #[test]
    fn save_creates_missing_directories_and_cleans_up() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("nested").join("port_allocations.json");
        let mut store = PortStore::default();
        allocate(&mut store, WORKTREE_A, "web");
        store.save(&path).expect("store saves");

        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .expect("read dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    /// The lock is what makes concurrent `allocate` calls safe across
    /// processes: while one holds it, another cannot take it.
    #[test]
    fn the_store_lock_is_exclusive_until_dropped() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("port_allocations.json.lock");

        let held =
            StoreLock::acquire_with(path.clone(), Duration::from_millis(50), STALE_LOCK_AFTER)
                .expect("first acquire succeeds");
        assert!(
            StoreLock::acquire_with(path.clone(), Duration::from_millis(50), STALE_LOCK_AFTER)
                .is_err(),
            "a second holder must not get the lock"
        );

        drop(held);
        assert!(!path.exists(), "dropping the lock removes the file");
        StoreLock::acquire_with(path, Duration::from_millis(50), STALE_LOCK_AFTER)
            .expect("the lock is reusable once released");
    }

    /// A lock left behind by a killed process must not block every later
    /// invocation forever.
    #[test]
    fn a_stale_lock_is_reclaimed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("port_allocations.json.lock");
        fs::write(&path, "999999").expect("plant an orphaned lock");

        // Zero staleness window stands in for "the holder died a long time
        // ago" without having to fake the file's mtime.
        StoreLock::acquire_with(path, Duration::from_millis(50), Duration::ZERO)
            .expect("a stale lock is reclaimed");
    }

    /// Only one of several processes that judge the same lock stale may
    /// reclaim it. The second attempt replays what a loser does: it still
    /// carries the holder token it observed, and finds the file already gone.
    #[test]
    fn only_one_reclaim_of_a_stale_lock_can_win() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("port_allocations.json.lock");
        fs::write(&path, "stale-holder").expect("plant an orphaned lock");

        assert!(
            reclaim_stale_lock(&path, "stale-holder"),
            "the first reclaimer wins the rename"
        );
        assert!(!path.exists(), "the winner clears the stale lock");
        assert!(
            !reclaim_stale_lock(&path, "stale-holder"),
            "a loser whose rename finds nothing must not claim a reclaim"
        );
    }

    /// The race the token check exists for: a loser gets as far as the rename,
    /// but by then the winner has already reclaimed and taken a *fresh* lock.
    /// Deleting by path would drop that live holder out of its critical
    /// section, so the reclaim must recognise the mismatch and put it back.
    #[test]
    fn a_reclaim_that_grabs_a_live_lock_restores_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("port_allocations.json.lock");

        // The winner's fresh lock, sitting where the stale one used to be.
        let live =
            StoreLock::acquire_with(path.clone(), Duration::from_millis(50), STALE_LOCK_AFTER)
                .expect("the winner holds a fresh lock");

        // The loser is still working from the token it observed as stale.
        assert!(
            !reclaim_stale_lock(&path, "stale-holder"),
            "grabbing a live lock is not a successful reclaim"
        );
        assert!(path.exists(), "the live lock is put back");
        assert_eq!(
            fs::read_to_string(&path).expect("read the lock"),
            live.token,
            "the restored lock still belongs to its original holder"
        );

        // And the holder can still release its own lock normally.
        drop(live);
        assert!(!path.exists(), "the original holder still owns the release");

        // No reclaim scratch files are left lying around.
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".reclaim."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "reclaim files left behind: {leftovers:?}"
        );
    }

    /// Releasing must be by identity, not by path. A holder whose lock was
    /// reclaimed out from under it must not delete whatever now sits there —
    /// that is what turns a single race into a cascade of stolen locks.
    #[test]
    fn dropping_a_lock_never_removes_someone_elses() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("port_allocations.json.lock");

        let stolen =
            StoreLock::acquire_with(path.clone(), Duration::from_millis(50), STALE_LOCK_AFTER)
                .expect("acquire");
        // Stand in for "someone reclaimed this lock and took it themselves".
        fs::write(&path, "a-different-holder").expect("overwrite the lock");

        drop(stolen);
        assert!(path.exists(), "the other holder's lock survives our drop");
        assert_eq!(
            fs::read_to_string(&path).expect("read the lock"),
            "a-different-holder"
        );
    }

    /// Every acquisition gets its own token, so two locks are never
    /// mistaken for each other.
    #[test]
    fn each_lock_acquisition_has_a_distinct_token() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("port_allocations.json.lock");

        let first =
            StoreLock::acquire_with(path.clone(), Duration::from_millis(50), STALE_LOCK_AFTER)
                .expect("acquire");
        let first_token = first.token.clone();
        drop(first);
        let second = StoreLock::acquire_with(path, Duration::from_millis(50), STALE_LOCK_AFTER)
            .expect("re-acquire");
        assert_ne!(first_token, second.token);
    }

    #[test]
    fn empty_names_are_rejected() {
        assert!(normalize_name("  ").is_err());
        assert_eq!(normalize_name(" web ").unwrap(), "web");
    }

    /// Outside a git repository the working directory itself is the key, so
    /// the command still works in a plain folder.
    #[test]
    fn worktree_key_falls_back_to_the_working_directory() {
        let dir = tempfile::tempdir().expect("temp dir");
        // The temp root is not normally inside a repository, but a machine
        // with TMPDIR pointing under one would legitimately report a
        // toplevel; only the fallback path is under test here.
        let inside_repo = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false);
        if inside_repo {
            eprintln!("[test] temp dir is inside a git repo — skipping fallback assertion");
            return;
        }
        assert_eq!(worktree_key(dir.path()), canonical_string(dir.path()));
    }
}
