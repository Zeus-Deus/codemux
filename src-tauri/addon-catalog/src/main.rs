//! Static review tool: shares the exact archive and download guards with desktop.
use codemux_addon_protocol::catalog::{self, Capabilities, Catalog, Plugin, Release};
pub use codemux_addon_protocol::{ErrorCode, Manifest, ProtocolError};
pub type Result<T> = std::result::Result<T, ProtocolError>;
#[path = "../../src/addons/download.rs"]
mod download;
#[path = "../../src/addons/http.rs"]
mod http;
#[path = "../../src/addons/package.rs"]
mod package;
use std::{io::Read, path::Path, time::Duration};
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
/// Checks downloaded release bytes against the reviewed entry. The archive is
/// validated inertly; its code is never executed.
fn verify_release(plugin: &Plugin, release: &Release, bytes: Vec<u8>) -> Result<()> {
    if bytes.len() as u64 != release.compressed_bytes {
        return Err(ProtocolError::invalid("Release size mismatch"));
    }
    let package = package::Package::parse_for_platform(bytes, Some(&release.sha256), None)?;
    let m = &package.manifest;
    for (field, matches) in [
        ("ID", m.id == plugin.id),
        ("version", m.version == release.version),
        ("API range", m.api == release.api),
        ("platforms", m.platforms == release.platforms),
        ("repository", m.repository == plugin.repository),
        ("license", m.license == release.license),
        (
            "capabilities",
            Capabilities::from_manifest(m).normalized() == release.capabilities.normalized(),
        ),
    ] {
        if !matches {
            return Err(ProtocolError::invalid(format!(
                "Package {field} does not match the catalog entry"
            )));
        }
    }
    Ok(())
}
/// Accepted releases are immutable (`Catalog::check_successor`), so only releases
/// new since the previous catalog need their tag provenance resolved again.
fn needs_provenance(previous: Option<&Catalog>, plugin: &Plugin, release: &Release) -> bool {
    !previous.is_some_and(|previous| {
        previous.plugins.iter().any(|old| {
            old.id == plugin.id && old.releases.iter().any(|r| r.version == release.version)
        })
    })
}
fn github_token() -> Option<String> {
    ["GITHUB_TOKEN", "GH_TOKEN"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
}
/// Only these GitHub REST API requests carry the optional CI token, which raises
/// the anonymous rate limit. Release downloads and the desktop stay anonymous.
fn provenance_request(
    client: &reqwest::Client,
    repo: &str,
    reference: &str,
    token: Option<&str>,
) -> reqwest::RequestBuilder {
    let request = client
        .get(format!(
            "https://api.github.com/repos/{repo}/commits/{reference}"
        ))
        .header("User-Agent", "CodeMux-Addon-Catalog/1")
        .header("Accept", "application/vnd.github.sha")
        .header("X-GitHub-Api-Version", "2022-11-28");
    match token {
        Some(token) => request.bearer_auth(token),
        None => request,
    }
}
/// The commit a release tag resolves to in the declared repository. Tags are
/// per-repository, so a match also proves the source commit belongs to it.
async fn tag_commit(
    client: &reqwest::Client,
    repo: &str,
    tag: &str,
    token: Option<&str>,
) -> Result<String> {
    let unavailable = |detail: &str| {
        ProtocolError::new(
            ErrorCode::NetworkDenied,
            format!("Public source provenance unavailable: {detail}"),
        )
    };
    let mut response = provenance_request(client, repo, tag, token)
        .send()
        .await
        .map_err(|_| unavailable("request failed"))?;
    let status = response.status();
    if !status.is_success() {
        let hint = if token.is_none() && matches!(status.as_u16(), 403 | 429) {
            "; set GITHUB_TOKEN for the authenticated rate limit"
        } else {
            ""
        };
        return Err(unavailable(&format!("HTTP {}{hint}", status.as_u16())));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| unavailable("response failed"))?
    {
        if body.len() + chunk.len() > 64 {
            return Err(unavailable("unexpected response"));
        }
        body.extend_from_slice(&chunk);
    }
    let sha = std::str::from_utf8(&body).unwrap_or_default().trim();
    if !catalog::hex(sha, 40) {
        return Err(unavailable("unexpected response"));
    }
    Ok(sha.into())
}
#[tokio::main]
async fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    assert!(args.len()>=2,"Usage: codemux-addon-catalog <check|online|generate> <catalog.json> [previous.json|entries-directory]");
    let path = Path::new(&args[1]);
    let mut catalog =
        Catalog::parse(&read_bounded(path, 2 * 1024 * 1024)).expect("Invalid catalog");
    let mut previous = None;
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
        plugins.sort_by(|a: &Plugin, b| a.id.cmp(&b.id));
        catalog.plugins = plugins;
        catalog.validate().expect("Invalid entries");
        let mut bytes = serde_json::to_vec_pretty(&catalog).unwrap();
        bytes.push(b'\n');
        assert!(bytes.len() <= 2 * 1024 * 1024);
        std::fs::write(path, bytes).expect("write generated catalog");
    } else {
        assert!(args[0] == "check" || args[0] == "online", "Unknown command");
        if let Some(path) = args.get(2) {
            let accepted = Catalog::parse(&read_bounded(Path::new(path), 2 * 1024 * 1024))
                .expect("previous catalog");
            catalog
                .check_successor(&accepted)
                .expect("Catalog history changed");
            previous = Some(accepted);
        }
    }
    if args[0] == "online" {
        let token = github_token();
        let client = reqwest::Client::builder()
            .no_proxy()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .expect("HTTP client");
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
                verify_release(plugin, release, bytes).unwrap_or_else(|error| {
                    panic!("{} {}: {}", plugin.id, release.version, error.message)
                });
                // Verify that the release tag in the declared public repository
                // resolves to the immutable source commit.
                if needs_provenance(previous.as_ref(), plugin, release) {
                    let repo = plugin
                        .repository
                        .strip_prefix("https://github.com/")
                        .unwrap();
                    let asset = url::Url::parse(&release.download_url).unwrap();
                    let tag = asset.path_segments().unwrap().nth(4).unwrap();
                    let commit = tag_commit(&client, repo, tag, token.as_deref())
                        .await
                        .unwrap_or_else(|error| panic!("{}", error.message));
                    assert_eq!(
                        commit, release.source_commit,
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
#[cfg(test)]
mod tests {
    use super::*;
    use codemux_addon_protocol::catalog::Tier;
    use sha2::{Digest, Sha256};
    use std::io::Write;
    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(bytes).unwrap();
        gzip.finish().unwrap()
    }
    fn tar_of(archive: &[u8]) -> Vec<u8> {
        let mut tar = Vec::new();
        flate2::read::GzDecoder::new(archive)
            .read_to_end(&mut tar)
            .unwrap();
        tar
    }
    /// The reviewed entry for `bytes`, built from the fixture manifest.
    fn entry(bytes: &[u8]) -> (Plugin, Release) {
        let manifest: Manifest =
            serde_json::from_str(include_str!("../../addon-protocol/fixtures/hello.json")).unwrap();
        let release = Release {
            version: manifest.version.clone(),
            api: manifest.api.clone(),
            platforms: manifest.platforms.clone(),
            source_commit: "a".repeat(40),
            download_url:
                "https://github.com/example/hello/releases/download/v1.0.0/hello.cmxaddon".into(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            compressed_bytes: bytes.len() as u64,
            published_at: "2026-09-18T00:00:00Z".into(),
            license: manifest.license.clone(),
            capabilities: Capabilities::from_manifest(&manifest),
        };
        let plugin = Plugin {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            publisher: "Example".into(),
            tier: Tier::Community,
            repository: manifest.repository.clone(),
            description: manifest.description.clone(),
            readme: "Fixture".into(),
            releases: vec![release.clone()],
        };
        (plugin, release)
    }
    fn refusal(bytes: Vec<u8>) -> ProtocolError {
        let (plugin, release) = entry(&bytes);
        verify_release(&plugin, &release, bytes).unwrap_err()
    }
    #[test]
    fn reviewed_release_bytes_must_match_their_entry() {
        let archive = package::fixture_archive();
        let (plugin, release) = entry(&archive);
        verify_release(&plugin, &release, archive.clone()).unwrap();
        let mut tampered = archive.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert_eq!(
            verify_release(&plugin, &release, tampered)
                .unwrap_err()
                .message,
            "Package digest does not match the accepted release"
        );
        let mut longer = archive.clone();
        longer.push(0);
        assert_eq!(
            verify_release(&plugin, &release, longer)
                .unwrap_err()
                .message,
            "Release size mismatch"
        );
        let mut other = release.clone();
        other.version = "1.0.1".into();
        assert_eq!(
            verify_release(&plugin, &other, archive.clone())
                .unwrap_err()
                .message,
            "Package version does not match the catalog entry"
        );
        let mut other = release.clone();
        other.capabilities.permissions =
            vec![codemux_addon_protocol::manifest::Permission::GitRead];
        assert_eq!(
            verify_release(&plugin, &other, archive.clone())
                .unwrap_err()
                .message,
            "Package capabilities does not match the catalog entry"
        );
        let mut moved = plugin.clone();
        moved.repository = "https://github.com/other/hello".into();
        assert_eq!(
            verify_release(&moved, &release, archive)
                .unwrap_err()
                .message,
            "Package repository does not match the catalog entry"
        );
    }
    #[test]
    fn tampered_archives_fail_inertly_even_with_a_matching_digest() {
        let archive = package::fixture_archive();
        let mut trailing = archive.clone();
        trailing.extend_from_slice(b"trailing");
        let mut second_member = archive.clone();
        second_member.extend_from_slice(&gzip(b""));
        for bytes in [trailing, second_member] {
            assert_eq!(
                refusal(bytes).message,
                "Trailing compressed archive content"
            );
        }
        let mut tar = tar_of(&archive);
        tar.extend_from_slice(b"hidden");
        assert_eq!(
            refusal(gzip(&tar)).message,
            "Unexpected content after tar terminator"
        );
        let error = refusal(package::fixture_archive_with_source(&vec![
            b' ';
            codemux_addon_protocol::limits::BUNDLE
                + 1
        ]));
        assert_eq!(
            (error.data.code, error.message.as_str()),
            (ErrorCode::ResourceLimit, "Archive entry exceeds its limit")
        );
        let error = refusal(vec![0; 10 * 1024 * 1024 + 1]);
        assert_eq!(
            (error.data.code, error.message.as_str()),
            (ErrorCode::ResourceLimit, "Archive exceeds 10 MiB")
        );
    }
    #[test]
    fn only_new_releases_resolve_provenance() {
        let archive = package::fixture_archive();
        let (plugin, release) = entry(&archive);
        let mut previous = Catalog {
            schema_version: 1,
            revision: 1,
            generated_at: "2026-09-18T00:00:00Z".into(),
            plugins: vec![plugin.clone()],
            blocked: vec![],
        };
        previous.validate().unwrap();
        assert!(needs_provenance(None, &plugin, &release));
        assert!(!needs_provenance(Some(&previous), &plugin, &release));
        let mut next = release.clone();
        next.version = "1.1.0".into();
        assert!(needs_provenance(Some(&previous), &plugin, &next));
        previous.plugins[0].id = "example.other".into();
        assert!(needs_provenance(Some(&previous), &plugin, &release));
    }
    #[test]
    fn only_github_api_requests_carry_the_ci_token() {
        let client = reqwest::Client::new();
        let request = provenance_request(&client, "example/hello", "v1.0.0", Some("secret"))
            .build()
            .unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://api.github.com/repos/example/hello/commits/v1.0.0"
        );
        let authorization = &request.headers()["authorization"];
        assert_eq!(authorization, "Bearer secret");
        assert!(authorization.is_sensitive());
        assert_eq!(request.headers()["accept"], "application/vnd.github.sha");
        let anonymous = provenance_request(&client, "example/hello", "v1.0.0", None)
            .build()
            .unwrap();
        assert!(!anonymous.headers().contains_key("authorization"));
    }
}
