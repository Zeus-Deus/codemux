//! Static review tool: shares the exact archive and download guards with desktop.
use codemux_addon_protocol::catalog::{Capabilities, Catalog};
pub use codemux_addon_protocol::{ErrorCode, Manifest, ProtocolError};
pub type Result<T> = std::result::Result<T, ProtocolError>;
#[path = "../../src/addons/download.rs"]
mod download;
#[path = "../../src/addons/http.rs"]
mod http;
#[path = "../../src/addons/package.rs"]
mod package;
use std::{io::Read, path::Path};
fn read_bounded(path: &Path, max: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .expect("read input")
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .expect("read input");
    assert!(bytes.len() <= max, "Input exceeds limit");
    bytes
}
#[tokio::main]
async fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    assert!(args.len()>=2,"Usage: codemux-addon-catalog <check|online|generate> <catalog.json> [previous.json|entries-directory]");
    let path = Path::new(&args[1]);
    let mut catalog =
        Catalog::parse(&read_bounded(path, 2 * 1024 * 1024)).expect("Invalid catalog");
    if args[0] == "generate" {
        let entries = std::fs::read_dir(args.get(2).expect("entries directory required"))
            .expect("entries directory");
        let mut plugins = Vec::new();
        for entry in entries {
            let entry = entry.expect("entry");
            if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            assert!(
                entry.file_type().unwrap().is_file(),
                "Only regular entry files are supported"
            );
            let plugin = serde_json::from_slice(&read_bounded(&entry.path(), 1024 * 1024))
                .expect("Invalid entry");
            plugins.push(plugin);
        }
        plugins.sort_by(|a: &codemux_addon_protocol::catalog::Plugin, b| a.id.cmp(&b.id));
        catalog.plugins = plugins;
        catalog.validate().expect("Invalid entries");
        let mut bytes = serde_json::to_vec_pretty(&catalog).unwrap();
        bytes.push(b'\n');
        assert!(bytes.len() <= 2 * 1024 * 1024);
        std::fs::write(path, bytes).expect("write generated catalog");
    } else {
        assert!(args[0] == "check" || args[0] == "online", "Unknown command");
        if let Some(previous) = args.get(2) {
            catalog
                .check_successor(
                    &Catalog::parse(&read_bounded(Path::new(previous), 2 * 1024 * 1024))
                        .expect("previous catalog"),
                )
                .expect("Catalog history changed");
        }
    }
    if args[0] == "online" {
        for plugin in &catalog.plugins {
            for release in &plugin.releases {
                if catalog
                    .blocked_reason(&plugin.id, &release.sha256)
                    .is_some()
                {
                    continue;
                }
                let bytes = download::download(
                    &release.download_url,
                    release.compressed_bytes as usize,
                    true,
                )
                .await
                .expect("Release download failed");
                assert_eq!(
                    bytes.len() as u64,
                    release.compressed_bytes,
                    "Release size mismatch"
                );
                let package =
                    package::Package::parse_for_platform(bytes, Some(&release.sha256), None)
                        .expect("Static package validation failed");
                let m = &package.manifest;
                assert_eq!(m.id, plugin.id);
                assert_eq!(m.version, release.version);
                assert_eq!(m.api, release.api);
                assert_eq!(m.platforms, release.platforms);
                assert_eq!(m.repository, plugin.repository);
                assert_eq!(m.license, release.license);
                assert_eq!(
                    Capabilities::from_manifest(m).normalized(),
                    release.capabilities.normalized()
                );
                // Verify that the immutable source commit belongs to the declared
                // public repo and that the release tag resolves to that exact commit.
                let repo = plugin
                    .repository
                    .strip_prefix("https://github.com/")
                    .unwrap();
                let asset = url::Url::parse(&release.download_url).unwrap();
                let tag = asset.path_segments().unwrap().nth(4).unwrap();
                for reference in [&release.source_commit, tag] {
                    let url = format!("https://api.github.com/repos/{repo}/commits/{reference}");
                    let bytes = download::download(&url, 2 * 1024 * 1024, false)
                        .await
                        .expect("Public source provenance unavailable");
                    let value: serde_json::Value =
                        serde_json::from_slice(&bytes).expect("Source metadata");
                    assert_eq!(
                        value["sha"].as_str(),
                        Some(release.source_commit.as_str()),
                        "Release tag/source commit mismatch"
                    );
                }
                println!(
                    "Verified {} {} {} without executing plugin code",
                    plugin.id, release.version, release.sha256
                );
            }
        }
    }
    println!(
        "Catalog revision {} valid ({} plugins)",
        catalog.revision,
        catalog.plugins.len()
    );
}
