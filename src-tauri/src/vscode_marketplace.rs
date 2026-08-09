//! VS Code Marketplace theme import.
//!
//! Two Tauri commands back the "import a theme from the VS Code
//! Marketplace" flow in Settings → Theme:
//!
//! - [`vscode_marketplace_search`] queries the public gallery
//!   `extensionquery` endpoint (the same API `code --install-extension`
//!   talks to) restricted to the `Microsoft.VisualStudio.Code` category,
//!   and returns at most 12 hits that actually ship a `.vsix` asset.
//! - [`vscode_marketplace_fetch_themes`] downloads one `.vsix` (a plain
//!   ZIP), reads `extension/package.json`, and returns the raw JSONC text
//!   of every *dark* colour theme it contributes.
//!
//! Codemux is dark-only, so `uiTheme` values of `vs` (light) are dropped
//! here rather than shipped to the frontend to be filtered again.
//!
//! Safety notes: the download is streamed with a hard 60 MB ceiling so a
//! hostile or accidentally huge asset can't balloon memory, archive member
//! paths are validated against zip-slip (`..` / absolute / drive-relative),
//! and each extracted member is capped independently. The `zip` crate is
//! synchronous, so all archive work happens inside `spawn_blocking`.
//!
//! Field naming follows the repo convention for command payloads: plain
//! snake_case (serde default), matching e.g. `commands::package_detect`
//! and `presets`. The frontend types in `src/tauri/types.ts` are written
//! in snake_case to match.

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::io::Read;

/// Public gallery endpoint. Undocumented but stable; it is what the VS Code
/// client itself uses.
const EXTENSION_QUERY_URL: &str =
    "https://marketplace.visualstudio.com/_apis/public/gallery/extensionquery";

/// The gallery API is versioned through the Accept header, not the URL.
const GALLERY_ACCEPT: &str = "application/json;api-version=3.0-preview.1";

/// Asset type of the packaged extension inside a version's `files` list.
const VSIX_ASSET_TYPE: &str = "Microsoft.VisualStudio.Services.VSIXPackage";

/// Hard ceiling on a downloaded `.vsix`. Colour-theme extensions are a few
/// hundred KB; anything past this is either mispackaged or hostile.
const MAX_VSIX_BYTES: u64 = 60 * 1024 * 1024;

/// Per-archive-member ceiling. Guards against zip bombs — a single theme
/// JSON or manifest well past this is not something we want to buffer.
const MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;

/// Result cap, mirrored in the request body's `pageSize`.
const MAX_RESULTS: usize = 12;

/// One marketplace hit, as shown in the import picker.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarketplaceTheme {
    /// `publisher.name` — the identifier `code --install-extension` takes.
    pub extension_id: String,
    pub display_name: String,
    pub publisher: String,
    pub install_count: u64,
    pub version: String,
    /// Direct download URL for the newest version's `.vsix` asset.
    pub vsix_url: String,
}

/// One dark colour theme contributed by a downloaded extension.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarketplaceThemeVariant {
    pub label: String,
    /// `vs-dark` or `hc-black`.
    pub ui_theme: String,
    /// Raw JSONC text of the theme file (comments and trailing commas
    /// intact — the frontend parser tolerates both).
    pub content: String,
}

// ---------------------------------------------------------------------------
// extensionquery response shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct QueryResponse {
    #[serde(default)]
    results: Vec<QueryResult>,
}

#[derive(Debug, Deserialize)]
struct QueryResult {
    #[serde(default)]
    extensions: Vec<QueryExtension>,
}

#[derive(Debug, Deserialize)]
struct QueryExtension {
    #[serde(rename = "extensionName", default)]
    extension_name: String,
    #[serde(rename = "displayName", default)]
    display_name: String,
    #[serde(default)]
    publisher: QueryPublisher,
    #[serde(default)]
    versions: Vec<QueryVersion>,
    #[serde(default)]
    statistics: Vec<QueryStatistic>,
}

#[derive(Debug, Default, Deserialize)]
struct QueryPublisher {
    #[serde(rename = "publisherName", default)]
    publisher_name: String,
    #[serde(rename = "displayName", default)]
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct QueryVersion {
    #[serde(default)]
    version: String,
    #[serde(default)]
    files: Vec<QueryFile>,
}

#[derive(Debug, Deserialize)]
struct QueryFile {
    #[serde(rename = "assetType", default)]
    asset_type: String,
    #[serde(default)]
    source: String,
}

#[derive(Debug, Deserialize)]
struct QueryStatistic {
    #[serde(rename = "statisticName", default)]
    statistic_name: String,
    #[serde(default)]
    value: f64,
}

/// Pure parser for an `extensionquery` response body.
///
/// Entries without a VSIX asset on their newest version are skipped —
/// there is nothing to download for them.
fn parse_extension_query(body: &str) -> Result<Vec<MarketplaceTheme>, String> {
    let parsed: QueryResponse = serde_json::from_str(body)
        .map_err(|e| format!("Marketplace returned an unexpected response: {e}"))?;

    let mut out = Vec::new();
    for result in parsed.results {
        for ext in result.extensions {
            // Versions come back newest-first.
            let Some(version) = ext.versions.first() else {
                continue;
            };
            let Some(vsix_url) = version
                .files
                .iter()
                .find(|f| f.asset_type == VSIX_ASSET_TYPE)
                .map(|f| f.source.clone())
                .filter(|s| !s.is_empty())
            else {
                continue;
            };

            let install_count = ext
                .statistics
                .iter()
                .find(|s| s.statistic_name == "install")
                .map(|s| s.value.max(0.0) as u64)
                .unwrap_or(0);

            let publisher = if ext.publisher.display_name.is_empty() {
                ext.publisher.publisher_name.clone()
            } else {
                ext.publisher.display_name.clone()
            };

            let display_name = if ext.display_name.is_empty() {
                ext.extension_name.clone()
            } else {
                ext.display_name.clone()
            };

            out.push(MarketplaceTheme {
                extension_id: format!("{}.{}", ext.publisher.publisher_name, ext.extension_name),
                display_name,
                publisher,
                install_count,
                version: version.version.clone(),
                vsix_url,
            });

            if out.len() >= MAX_RESULTS {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// package.json / contributes.themes
// ---------------------------------------------------------------------------

/// A `contributes.themes[]` entry that survived the dark-only filter.
#[derive(Debug, Clone, PartialEq)]
struct ThemeEntry {
    label: String,
    ui_theme: String,
    /// Archive-relative path, already resolved to `extension/...`.
    archive_path: String,
}

fn is_dark_ui_theme(ui_theme: &str) -> bool {
    matches!(ui_theme, "vs-dark" | "hc-black")
}

/// Turn a `contributes.themes[].path` into a path inside the `.vsix`.
///
/// The manifest lives at `extension/package.json` and theme paths are
/// relative to that directory, commonly written `./themes/foo.json`.
/// Rejects anything that would escape `extension/` (zip-slip).
fn resolve_theme_path(raw: &str) -> Result<String, String> {
    let normalized = raw.replace('\\', "/");
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return Err("theme entry has an empty path".to_string());
    }
    if trimmed.starts_with('/') || trimmed.contains(':') {
        return Err(format!("theme path \"{raw}\" is not relative"));
    }

    let mut parts: Vec<&str> = Vec::new();
    for segment in trimmed.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return Err(format!("theme path \"{raw}\" escapes the extension folder")),
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return Err(format!("theme path \"{raw}\" resolves to nothing"));
    }
    Ok(format!("extension/{}", parts.join("/")))
}

/// Pure parser for a VSIX `extension/package.json`.
///
/// Returns the extension's human name (for error messages) plus its dark
/// theme entries with archive-resolved paths. Entries with an unusable
/// path are dropped rather than failing the whole import.
fn parse_contributes_themes(package_json: &str) -> Result<(String, Vec<ThemeEntry>), String> {
    let value: serde_json::Value = serde_json::from_str(package_json)
        .map_err(|e| format!("The extension's package.json is not valid JSON: {e}"))?;

    let name = value
        .get("displayName")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("name").and_then(|v| v.as_str()))
        .unwrap_or("this extension")
        .to_string();

    let themes = value
        .get("contributes")
        .and_then(|c| c.get("themes"))
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();

    let mut entries = Vec::new();
    for theme in themes {
        let ui_theme = theme.get("uiTheme").and_then(|v| v.as_str()).unwrap_or("");
        if !is_dark_ui_theme(ui_theme) {
            continue;
        }
        let Some(path) = theme.get("path").and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(archive_path) = resolve_theme_path(path) else {
            continue;
        };
        let label = theme
            .get("label")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&name)
            .to_string();

        entries.push(ThemeEntry {
            label,
            ui_theme: ui_theme.to_string(),
            archive_path,
        });
    }

    Ok((name, entries))
}

// ---------------------------------------------------------------------------
// Archive extraction (sync — always called from spawn_blocking)
// ---------------------------------------------------------------------------

/// Read one archive member as UTF-8 text, capped at [`MAX_ENTRY_BYTES`].
///
/// Falls back to a case-insensitive name match: a few publishers ship
/// manifest paths whose case doesn't match the stored entry.
fn read_archive_text<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    path: &str,
) -> Result<String, String> {
    let resolved = if archive.index_for_name(path).is_some() {
        path.to_string()
    } else {
        let lower = path.to_ascii_lowercase();
        let found = (0..archive.len()).find_map(|i| {
            let entry = archive.by_index_raw(i).ok()?;
            let name = entry.name().to_string();
            (name.to_ascii_lowercase() == lower).then_some(name)
        });
        found.ok_or_else(|| format!("The extension archive has no \"{path}\"."))?
    };

    let mut entry = archive
        .by_name(&resolved)
        .map_err(|e| format!("Could not read \"{path}\" from the extension archive: {e}"))?;

    if entry.size() > MAX_ENTRY_BYTES {
        return Err(format!(
            "\"{path}\" is {} MB, past the {} MB per-file limit.",
            entry.size() / (1024 * 1024),
            MAX_ENTRY_BYTES / (1024 * 1024)
        ));
    }

    let mut buf = Vec::with_capacity(entry.size().min(MAX_ENTRY_BYTES) as usize);
    // `take` is a second belt on top of the declared size: the header can lie.
    entry
        .by_ref()
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("Could not read \"{path}\" from the extension archive: {e}"))?;
    if buf.len() as u64 > MAX_ENTRY_BYTES {
        return Err(format!(
            "\"{path}\" is larger than the {} MB per-file limit.",
            MAX_ENTRY_BYTES / (1024 * 1024)
        ));
    }

    String::from_utf8(buf).map_err(|_| format!("\"{path}\" is not valid UTF-8 text."))
}

/// Pull every dark colour theme out of an in-memory `.vsix`.
fn extract_dark_themes(bytes: Vec<u8>) -> Result<Vec<MarketplaceThemeVariant>, String> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| format!("That download is not a valid .vsix package: {e}"))?;

    let manifest = read_archive_text(&mut archive, "extension/package.json")?;
    let (name, entries) = parse_contributes_themes(&manifest)?;

    if entries.is_empty() {
        return Err(format!(
            "\"{name}\" contributes no dark colour theme. Codemux only imports dark themes."
        ));
    }

    let mut variants = Vec::new();
    for entry in entries {
        match read_archive_text(&mut archive, &entry.archive_path) {
            Ok(content) => variants.push(MarketplaceThemeVariant {
                label: entry.label,
                ui_theme: entry.ui_theme,
                content,
            }),
            // One broken file shouldn't sink an extension that ships several.
            Err(err) => log::warn!("vscode_marketplace: skipping theme file: {err}"),
        }
    }

    if variants.is_empty() {
        return Err(format!(
            "\"{name}\" lists dark themes but none of their files could be read from the package."
        ));
    }
    Ok(variants)
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Search the VS Code Marketplace for extensions matching `query`.
///
/// Restricted to the VS Code category; capped at [`MAX_RESULTS`] hits.
#[tauri::command]
pub async fn vscode_marketplace_search(query: String) -> Result<Vec<MarketplaceTheme>, String> {
    let query = query.trim().to_string();
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let body = serde_json::json!({
        "filters": [{
            "criteria": [
                // 8 = category, 10 = search text, 12 = flags (4096 excludes
                // unpublished extensions).
                { "filterType": 8, "value": "Microsoft.VisualStudio.Code" },
                { "filterType": 10, "value": query },
                { "filterType": 12, "value": "4096" }
            ],
            "pageNumber": 1,
            "pageSize": MAX_RESULTS,
            "sortBy": 0,
            "sortOrder": 0
        }],
        // 914 = IncludeFiles | IncludeVersionProperties | IncludeStatistics
        //       | IncludeLatestVersionOnly.
        "flags": 914
    });

    let client = reqwest::Client::new();
    let response = client
        .post(EXTENSION_QUERY_URL)
        .header("Accept", GALLERY_ACCEPT)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Could not reach the VS Code Marketplace: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "VS Code Marketplace search failed (HTTP {}).",
            status.as_u16()
        ));
    }

    let text = response
        .text()
        .await
        .map_err(|e| format!("Could not read the Marketplace response: {e}"))?;

    parse_extension_query(&text)
}

/// Download a `.vsix` and return its dark colour themes as raw JSONC.
#[tauri::command]
pub async fn vscode_marketplace_fetch_themes(
    vsix_url: String,
) -> Result<Vec<MarketplaceThemeVariant>, String> {
    let parsed = url::Url::parse(&vsix_url).map_err(|_| "Invalid extension download URL.")?;
    if parsed.scheme() != "https" {
        return Err("Extension downloads must use https.".to_string());
    }

    let client = reqwest::Client::new();
    let response = client
        .get(parsed)
        .send()
        .await
        .map_err(|e| format!("Could not download the extension: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "Extension download failed (HTTP {}).",
            status.as_u16()
        ));
    }

    // Cheap pre-check when the server declares a length; the streaming
    // guard below is what actually enforces the ceiling.
    if let Some(len) = response.content_length() {
        if len > MAX_VSIX_BYTES {
            return Err(too_large_error(len));
        }
    }

    let mut bytes: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Could not download the extension: {e}"))?;
        if bytes.len() as u64 + chunk.len() as u64 > MAX_VSIX_BYTES {
            // Drop the stream without draining it — the connection closes.
            return Err(too_large_error(MAX_VSIX_BYTES + 1));
        }
        bytes.extend_from_slice(&chunk);
    }

    if bytes.is_empty() {
        return Err("The extension download was empty.".to_string());
    }

    // `zip` is synchronous; keep it off the async runtime's worker threads.
    tokio::task::spawn_blocking(move || extract_dark_themes(bytes))
        .await
        .map_err(|e| format!("Theme extraction failed: {e}"))?
}

fn too_large_error(len: u64) -> String {
    format!(
        "That extension is larger than the {} MB limit ({} MB) and was not downloaded.",
        MAX_VSIX_BYTES / (1024 * 1024),
        len / (1024 * 1024)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed to the fields we read, but shaped exactly like a real
    /// `extensionquery` response (including the light/dark irrelevant
    /// extras and an entry with no VSIX asset).
    const SAMPLE_QUERY_RESPONSE: &str = r#"{
      "results": [{
        "extensions": [
          {
            "publisher": { "publisherName": "dracula-theme", "displayName": "Dracula Theme" },
            "extensionId": "8f45a9ee-3a72-4a1c-b1d4-3a2b1f4d5e6a",
            "extensionName": "theme-dracula",
            "displayName": "Dracula Official",
            "shortDescription": "Official Dracula Theme.",
            "versions": [{
              "version": "2.25.1",
              "flags": "validated, public",
              "files": [
                { "assetType": "Microsoft.VisualStudio.Services.Icons.Default", "source": "https://example.test/icon.png" },
                { "assetType": "Microsoft.VisualStudio.Services.VSIXPackage", "source": "https://example.test/dracula.vsix" }
              ]
            }],
            "statistics": [
              { "statisticName": "install", "value": 7654321.0 },
              { "statisticName": "averagerating", "value": 4.7 }
            ]
          },
          {
            "publisher": { "publisherName": "noassets", "displayName": "" },
            "extensionName": "broken",
            "displayName": "",
            "versions": [{ "version": "1.0.0", "files": [
              { "assetType": "Microsoft.VisualStudio.Services.Icons.Default", "source": "https://example.test/x.png" }
            ]}],
            "statistics": []
          },
          {
            "publisher": { "publisherName": "sdras", "displayName": "sdras" },
            "extensionName": "night-owl",
            "displayName": "Night Owl",
            "versions": [{ "version": "2.1.1", "files": [
              { "assetType": "Microsoft.VisualStudio.Services.VSIXPackage", "source": "https://example.test/night-owl.vsix" }
            ]}],
            "statistics": [{ "statisticName": "install", "value": 1234.0 }]
          }
        ],
        "resultMetadata": []
      }]
    }"#;

    #[test]
    fn parses_extension_query_response() {
        let themes = parse_extension_query(SAMPLE_QUERY_RESPONSE).expect("parses");
        // The middle entry has no VSIX asset and is skipped.
        assert_eq!(themes.len(), 2);

        assert_eq!(
            themes[0],
            MarketplaceTheme {
                extension_id: "dracula-theme.theme-dracula".to_string(),
                display_name: "Dracula Official".to_string(),
                publisher: "Dracula Theme".to_string(),
                install_count: 7_654_321,
                version: "2.25.1".to_string(),
                vsix_url: "https://example.test/dracula.vsix".to_string(),
            }
        );
        assert_eq!(themes[1].extension_id, "sdras.night-owl");
        assert_eq!(themes[1].install_count, 1234);
    }

    #[test]
    fn missing_statistics_default_to_zero_installs() {
        let body = r#"{"results":[{"extensions":[{
            "publisher":{"publisherName":"p","displayName":""},
            "extensionName":"n","displayName":"",
            "versions":[{"version":"0.1.0","files":[
              {"assetType":"Microsoft.VisualStudio.Services.VSIXPackage","source":"https://e.test/a.vsix"}
            ]}]
        }]}]}"#;
        let themes = parse_extension_query(body).expect("parses");
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].install_count, 0);
        // Empty display names fall back to the raw name / publisher id.
        assert_eq!(themes[0].display_name, "n");
        assert_eq!(themes[0].publisher, "p");
    }

    #[test]
    fn caps_results_at_twelve() {
        let one = r#"{"publisher":{"publisherName":"p","displayName":"P"},
            "extensionName":"n","displayName":"N",
            "versions":[{"version":"1.0.0","files":[
              {"assetType":"Microsoft.VisualStudio.Services.VSIXPackage","source":"https://e.test/a.vsix"}
            ]}],"statistics":[]}"#;
        let body = format!(
            r#"{{"results":[{{"extensions":[{}]}}]}}"#,
            vec![one; 30].join(",")
        );
        assert_eq!(parse_extension_query(&body).unwrap().len(), 12);
    }

    #[test]
    fn rejects_non_json_query_response() {
        let err = parse_extension_query("<html>gateway timeout</html>").unwrap_err();
        assert!(err.contains("unexpected response"), "{err}");
    }

    #[test]
    fn resolves_theme_paths_relative_to_extension_dir() {
        assert_eq!(
            resolve_theme_path("./themes/dracula.json").unwrap(),
            "extension/themes/dracula.json"
        );
        assert_eq!(
            resolve_theme_path("themes/dracula.json").unwrap(),
            "extension/themes/dracula.json"
        );
        assert_eq!(
            resolve_theme_path("theme.json").unwrap(),
            "extension/theme.json"
        );
        // Windows-style separators show up in a few published manifests.
        assert_eq!(
            resolve_theme_path(".\\themes\\a.json").unwrap(),
            "extension/themes/a.json"
        );
        // Redundant `./` segments collapse.
        assert_eq!(
            resolve_theme_path("./themes/./a.json").unwrap(),
            "extension/themes/a.json"
        );
    }

    #[test]
    fn rejects_zip_slip_theme_paths() {
        for bad in [
            "../../etc/passwd",
            "./../outside.json",
            "/absolute/theme.json",
            "C:/windows/theme.json",
            "",
            "./",
        ] {
            assert!(
                resolve_theme_path(bad).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn parses_dark_themes_only_from_package_json() {
        let manifest = r#"{
          "name": "theme-dracula",
          "displayName": "Dracula Official",
          "contributes": {
            "themes": [
              { "label": "Dracula", "uiTheme": "vs-dark", "path": "./theme/dracula.json" },
              { "label": "Dracula Soft Light", "uiTheme": "vs", "path": "./theme/light.json" },
              { "label": "Dracula Contrast", "uiTheme": "hc-black", "path": "theme/hc.json" },
              { "label": "Escapee", "uiTheme": "vs-dark", "path": "../../evil.json" }
            ]
          }
        }"#;
        let (name, entries) = parse_contributes_themes(manifest).expect("parses");
        assert_eq!(name, "Dracula Official");
        assert_eq!(
            entries,
            vec![
                ThemeEntry {
                    label: "Dracula".to_string(),
                    ui_theme: "vs-dark".to_string(),
                    archive_path: "extension/theme/dracula.json".to_string(),
                },
                ThemeEntry {
                    label: "Dracula Contrast".to_string(),
                    ui_theme: "hc-black".to_string(),
                    archive_path: "extension/theme/hc.json".to_string(),
                },
            ]
        );
    }

    #[test]
    fn package_json_without_themes_yields_no_entries() {
        let (name, entries) =
            parse_contributes_themes(r#"{"name":"some-ext","contributes":{}}"#).expect("parses");
        assert_eq!(name, "some-ext");
        assert!(entries.is_empty());

        let (_, entries) = parse_contributes_themes(
            r#"{"name":"light-only","contributes":{"themes":[
                {"label":"L","uiTheme":"vs","path":"./l.json"}]}}"#,
        )
        .expect("parses");
        assert!(entries.is_empty());
    }

    #[test]
    fn theme_label_falls_back_to_extension_name() {
        let (_, entries) = parse_contributes_themes(
            r#"{"displayName":"Fallback Ext","contributes":{"themes":[
                {"uiTheme":"vs-dark","path":"./t.json"}]}}"#,
        )
        .expect("parses");
        assert_eq!(entries[0].label, "Fallback Ext");
    }

    #[test]
    fn rejects_non_zip_download() {
        let err = extract_dark_themes(b"not a zip file at all".to_vec()).unwrap_err();
        assert!(err.contains("not a valid .vsix package"), "{err}");
    }
}
