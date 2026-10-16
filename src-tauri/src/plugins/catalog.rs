//! The plugin catalog: `catalog/plugins/catalog-v1.json` in this repo,
//! published at codemux.org and re-checked every 24 hours. Entries are added
//! by pull request and the owner sets each tier. The `blocked` list refuses
//! an id or commit at install and disables it at load.

use serde::{Deserialize, Serialize};
use std::{
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const CATALOG_URL: &str = "https://codemux.org/plugins/catalog-v1.json";
pub const RECHECK_SECONDS: u64 = 24 * 60 * 60;
pub const MAX_CATALOG_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub schema_version: u32,
    pub revision: u64,
    #[serde(default)]
    pub plugins: Vec<CatalogPlugin>,
    #[serde(default)]
    pub blocked: Vec<Blocked>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPlugin {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// `"official"` or `"unverified"`.
    pub tier: String,
    pub repo: String,
    pub sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_codemux: Option<String>,
    #[serde(default)]
    pub screenshots: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Blocked {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    pub reason: String,
}

pub fn is_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

impl Catalog {
    /// Parse and validate. A catalog with a bad entry is refused whole, so a
    /// typo can't publish a half-valid list.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let catalog: Catalog =
            serde_json::from_slice(bytes).map_err(|e| format!("The plugin catalog is not valid: {e}"))?;
        if catalog.schema_version != 1 {
            return Err(format!(
                "Unsupported plugin catalog schema {}",
                catalog.schema_version
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for p in &catalog.plugins {
            if !super::valid_id(&p.id) {
                return Err(format!("Catalog id \"{}\" is not a valid plugin id", p.id));
            }
            if !ids.insert(p.id.as_str()) {
                return Err(format!("Catalog lists \"{}\" twice", p.id));
            }
            if p.tier != "official" && p.tier != "unverified" {
                return Err(format!("Catalog entry \"{}\" has an unknown tier", p.id));
            }
            if !is_sha(&p.sha) {
                return Err(format!("Catalog entry \"{}\" must pin a 40-character commit", p.id));
            }
            super::install::parse_repo(&p.repo)?;
            if let Some(subdir) = &p.subdir {
                super::install::check_subdir(subdir)?;
            }
        }
        for b in &catalog.blocked {
            if b.id.is_none() && b.sha.is_none() {
                return Err("A blocked entry needs an id or a sha".into());
            }
        }
        Ok(catalog)
    }

    /// The reason an id or commit is blocked, if it is.
    pub fn blocked_reason(&self, id: &str, sha: Option<&str>) -> Option<String> {
        self.blocked
            .iter()
            .find(|b| {
                b.id.as_deref() == Some(id)
                    || (sha.is_some() && b.sha.as_deref().map(str::to_ascii_lowercase).as_deref() == sha)
            })
            .map(|b| b.reason.clone())
    }

    pub fn entry(&self, id: &str) -> Option<&CatalogPlugin> {
        self.plugins.iter().find(|p| p.id == id)
    }
}

/// The bundled copy of the catalog: the app works offline and before the
/// first fetch, and a fetched catalog only replaces it if its revision is not
/// older.
pub fn bundled() -> Catalog {
    Catalog::parse(include_bytes!("../../../catalog/plugins/catalog-v1.json"))
        .expect("the bundled plugin catalog is valid")
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogView {
    pub catalog: Catalog,
    /// Seconds since the Unix epoch of the last successful fetch, if any.
    pub fetched_at: Option<u64>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct CatalogCache {
    state: Mutex<Option<(Catalog, Option<u64>, Option<String>)>>,
    #[cfg(test)]
    pub fetcher: Mutex<Option<Box<dyn Fn() -> Result<Vec<u8>, String> + Send + Sync>>>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl CatalogCache {
    /// The current catalog without touching the network.
    pub fn current(&self) -> CatalogView {
        let guard = self.state.lock().unwrap();
        match guard.as_ref() {
            Some((catalog, fetched_at, error)) => CatalogView {
                catalog: catalog.clone(),
                fetched_at: *fetched_at,
                error: error.clone(),
            },
            None => CatalogView { catalog: bundled(), fetched_at: None, error: None },
        }
    }

    /// Restore the last fetched catalog (persisted in `plugins.sqlite`), so
    /// the blocklist applies at load without touching the network.
    pub fn seed(&self, catalog: Catalog, fetched_at: u64) {
        let mut guard = self.state.lock().unwrap();
        if guard.is_none() && catalog.revision >= bundled().revision {
            *guard = Some((catalog, Some(fetched_at), None));
        }
    }

    /// The last successfully fetched catalog, for persisting.
    pub fn fetched(&self) -> Option<(Catalog, u64)> {
        match self.state.lock().unwrap().as_ref() {
            Some((catalog, Some(at), None)) => Some((catalog.clone(), *at)),
            _ => None,
        }
    }

    fn due(&self) -> bool {
        // A development catalog file is re-read every time.
        #[cfg(debug_assertions)]
        if std::env::var_os("CODEMUX_PLUGIN_CATALOG").is_some() {
            return true;
        }
        match self.state.lock().unwrap().as_ref() {
            Some((_, Some(at), _)) => now().saturating_sub(*at) >= RECHECK_SECONDS,
            _ => true,
        }
    }

    async fn fetch(&self) -> Result<Vec<u8>, String> {
        #[cfg(test)]
        if let Some(fetch) = self.fetcher.lock().unwrap().as_ref() {
            return fetch();
        }
        // Development builds can point at a local catalog file to try entries
        // and blocklists before they are published.
        #[cfg(debug_assertions)]
        if let Some(path) = std::env::var_os("CODEMUX_PLUGIN_CATALOG") {
            return std::fs::read(&path).map_err(|e| format!("Cannot read CODEMUX_PLUGIN_CATALOG: {e}"));
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()
            .map_err(|e| e.to_string())?;
        let mut response = client
            .get(CATALOG_URL)
            .header("User-Agent", "Codemux-Plugin-Catalog/1")
            .send()
            .await
            .map_err(|e| format!("Couldn't reach the plugin catalog: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("The plugin catalog returned {}", response.status()));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            if bytes.len() + chunk.len() > MAX_CATALOG_BYTES {
                return Err("The plugin catalog is too large".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    /// The catalog, re-fetched when the last fetch is older than 24 hours
    /// (or `force`). A failed fetch keeps the previous catalog and reports
    /// the error beside it.
    pub async fn get(&self, force: bool) -> CatalogView {
        if force || self.due() {
            let previous = self.current();
            let result = match self.fetch().await {
                Ok(bytes) => Catalog::parse(&bytes),
                Err(e) => Err(e),
            };
            let next = match result {
                // Never roll back to an older revision than we already have.
                Ok(catalog) if catalog.revision >= previous.catalog.revision => {
                    (catalog, Some(now()), None)
                }
                Ok(_) => (previous.catalog, Some(now()), None),
                Err(error) => (previous.catalog, previous.fetched_at, Some(error)),
            };
            *self.state.lock().unwrap() = Some(next);
        }
        self.current()
    }
}
