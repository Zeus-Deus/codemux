//! Helper programs Codemux installs for a plugin. A plugin names a tool; the
//! release, per-platform archive, size, SHA-256 and copied files are pinned
//! here, so a plugin cannot point the installer at anything else. An install
//! lands in `<app data>/tools/<id>/<version>-<platform>/`, is validated in a
//! staging folder and only then renamed into place. It never changes PATH,
//! never touches a copy the user installed, and never runs the tool beyond a
//! version check.
use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Archive {
    TarGz,
    Zip,
}

#[derive(Debug)]
struct Asset {
    os: &'static str,
    arch: &'static str,
    /// Platform label in the release and in the install folder name.
    label: &'static str,
    url: &'static str,
    sha256: &'static str,
    bytes: u64,
    archive: Archive,
    /// Files copied from the archive root; anything else is ignored.
    files: &'static [&'static str],
    executable: &'static str,
}

struct Tool {
    id: &'static str,
    version: &'static str,
    /// Exact stdout line of `<executable> --version`.
    version_line: &'static str,
    /// Environment for the version check.
    env: &'static [(&'static str, &'static str)],
    assets: &'static [Asset],
    /// Platforms with a release that cannot simply be unpacked.
    unsupported: &'static [(&'static str, &'static str)],
}

const CUA_UNIX_FILES: &[&str] = &[
    "cua-driver",
    "cua-cursor-theme",
    "wayland-helper/README.md",
    "wayland-helper/install.sh",
    "wayland-helper/winrects@cua/extension.js",
    "wayland-helper/winrects@cua/metadata.json",
];
const CUA_WINDOWS_FILES: &[&str] = &[
    "cua-driver.exe",
    "cua-cursor-theme.exe",
    "cua-driver-uia.exe",
];

// Values from the release's checksums.txt and release-manifest.json.
const TOOLS: &[Tool] = &[Tool {
    id: "cua-driver",
    version: "0.21.0",
    version_line: "cua-driver 0.21.0",
    env: &[("CUA_DRIVER_EMBEDDED", "1"), ("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")],
    assets: &[
        Asset {
            os: "linux",
            arch: "x86_64",
            label: "linux-x86_64",
            url: "https://github.com/trycua/cua/releases/download/cua-driver-rs-v0.21.0/cua-driver-rs-0.21.0-linux-x86_64-binary.tar.gz",
            sha256: "cf29bfaad16737e31e72f38dad2a6424f24b21a4ac2edea885e83cb9d5034135",
            bytes: 28_809_630,
            archive: Archive::TarGz,
            files: CUA_UNIX_FILES,
            executable: "cua-driver",
        },
        Asset {
            os: "linux",
            arch: "aarch64",
            label: "linux-arm64",
            url: "https://github.com/trycua/cua/releases/download/cua-driver-rs-v0.21.0/cua-driver-rs-0.21.0-linux-arm64-binary.tar.gz",
            sha256: "6b3a308c6741dd8291aad22a308bd2ec8eb52eab64687be007ac86ceaf7e3307",
            bytes: 28_777_061,
            archive: Archive::TarGz,
            files: CUA_UNIX_FILES,
            executable: "cua-driver",
        },
        Asset {
            os: "windows",
            arch: "x86_64",
            label: "windows-x86_64",
            url: "https://github.com/trycua/cua/releases/download/cua-driver-rs-v0.21.0/cua-driver-rs-0.21.0-windows-x86_64-binary.zip",
            sha256: "d63f6a78e65afc06524048f5557fed36cdf01f0a8a680236e93c9a2fb3587f44",
            bytes: 27_572_733,
            archive: Archive::Zip,
            files: CUA_WINDOWS_FILES,
            executable: "cua-driver.exe",
        },
        Asset {
            os: "windows",
            arch: "aarch64",
            label: "windows-arm64",
            url: "https://github.com/trycua/cua/releases/download/cua-driver-rs-v0.21.0/cua-driver-rs-0.21.0-windows-arm64-binary.zip",
            sha256: "79caf7e4f55cdab08533abf7ead4038132aa070040669238d806d057f648cc5f",
            bytes: 25_831_703,
            archive: Archive::Zip,
            files: CUA_WINDOWS_FILES,
            executable: "cua-driver.exe",
        },
    ],
    // macOS attributes Accessibility and Screen Recording grants to the
    // signed CuaDriver.app bundle in /Applications. A copy unpacked elsewhere
    // would not keep that identity, and replacing /Applications/CuaDriver.app
    // would overwrite the user's own install.
    unsupported: &[(
        "macos",
        "Codemux can’t install the driver on macOS. macOS permissions need the driver app in /Applications, which the official installer sets up.",
    )],
}];

const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
const VERSION_TIMEOUT: Duration = Duration::from_secs(15);
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    id: &'static str,
    version: &'static str,
    supported: bool,
    unsupported_reason: Option<String>,
    /// The installed managed executable, when present.
    executable: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "stage", rename_all = "camelCase")]
pub enum InstallProgress {
    Downloading { received: u64, total: u64 },
    Verifying,
    Validating,
}

fn tool(id: &str) -> Result<&'static Tool, String> {
    TOOLS
        .iter()
        .find(|tool| tool.id == id)
        .ok_or_else(|| format!("Unknown tool \"{id}\""))
}

fn asset_for(tool: &'static Tool, os: &str, arch: &str) -> Result<&'static Asset, String> {
    if let Some((_, reason)) = tool.unsupported.iter().find(|(name, _)| *name == os) {
        return Err((*reason).into());
    }
    tool.assets
        .iter()
        .find(|asset| asset.os == os && asset.arch == arch)
        .ok_or_else(|| {
            format!(
                "Codemux can’t install {} on this platform ({os} {arch}).",
                tool.id
            )
        })
}

fn tools_root() -> Result<PathBuf, String> {
    Ok(super::super::app_data_dir()
        .ok_or("App data directory is unavailable")?
        .join("tools"))
}

fn install_dir(root: &Path, tool: &Tool, asset: &Asset) -> PathBuf {
    root.join(tool.id)
        .join(format!("{}-{}", tool.version, asset.label))
}

fn status_at(root: &Path, tool: &'static Tool, os: &str, arch: &str) -> ToolStatus {
    let (supported, unsupported_reason, executable) = match asset_for(tool, os, arch) {
        Ok(asset) => {
            let path = install_dir(root, tool, asset).join(asset.executable);
            let present = fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file());
            (
                true,
                None,
                present.then(|| path.to_string_lossy().into_owned()),
            )
        }
        Err(reason) => (false, Some(reason), None),
    };
    ToolStatus {
        id: tool.id,
        version: tool.version,
        supported,
        unsupported_reason,
        executable,
    }
}

pub(super) fn status(id: &str) -> Result<ToolStatus, String> {
    Ok(status_at(
        &tools_root()?,
        tool(id)?,
        std::env::consts::OS,
        std::env::consts::ARCH,
    ))
}

/// One install per tool at a time in this process.
static INSTALLING: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
struct InstallLock(&'static str);
impl InstallLock {
    fn take(id: &'static str) -> Result<Self, String> {
        let mut busy = INSTALLING.lock().unwrap();
        if !busy.get_or_insert_with(HashSet::new).insert(id) {
            return Err("An installation is already running.".into());
        }
        Ok(Self(id))
    }
}
impl Drop for InstallLock {
    fn drop(&mut self) {
        if let Some(busy) = INSTALLING.lock().unwrap().as_mut() {
            busy.remove(self.0);
        }
    }
}

fn private_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("Could not create {}: {e}", path.display()))?;
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_dir() {
        return Err(format!("{} is not a folder", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn temp_name(kind: &str) -> String {
    format!(".{kind}-{}", uuid::Uuid::new_v4().simple())
}

/// Remove leftovers of an interrupted install: dot-entries older than a day.
fn sweep(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !(name.starts_with(".download-")
            || name.starts_with(".staging-")
            || name.starts_with(".old-"))
        {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if meta
            .modified()
            .ok()
            .and_then(|time| time.elapsed().ok())
            .is_some_and(|age| age > STALE_AFTER)
        {
            let _ = if meta.is_dir() {
                fs::remove_dir_all(entry.path())
            } else {
                fs::remove_file(entry.path())
            };
        }
    }
}

fn verify(path: &Path, sha256: &str, bytes: u64) -> Result<(), String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let copied = io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
    if copied != bytes || format!("{:x}", hasher.finalize()) != sha256 {
        return Err("The download didn’t match the published checksum. Try again.".into());
    }
    Ok(())
}

/// A pinned file name inside the archive, or `None` for anything else.
fn wanted<'a>(asset: &'a Asset, name: &str) -> Option<&'a str> {
    let name = name.trim_start_matches("./");
    let path = Path::new(name);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    asset.files.iter().copied().find(|file| *file == name)
}

fn write_file(
    staging: &Path,
    name: &str,
    reader: &mut dyn Read,
    executable: bool,
) -> Result<(), String> {
    let path = staging.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let copied =
        io::copy(&mut reader.take(MAX_FILE_BYTES + 1), &mut file).map_err(|e| e.to_string())?;
    if copied > MAX_FILE_BYTES {
        return Err(format!("{name} in the download is unexpectedly large"));
    }
    file.flush().map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o755 } else { 0o644 };
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    Ok(())
}

/// Copy the pinned regular files out of the archive. Links, devices and
/// unlisted entries are skipped; every pinned file must be present.
fn extract(archive: &Path, asset: &Asset, staging: &Path) -> Result<(), String> {
    let mut found = HashSet::new();
    let damaged = |e: &dyn std::fmt::Display| format!("The download could not be unpacked: {e}");
    match asset.archive {
        Archive::TarGz => {
            let file = fs::File::open(archive).map_err(|e| e.to_string())?;
            let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
            for entry in tar.entries().map_err(|e| damaged(&e))? {
                let mut entry = entry.map_err(|e| damaged(&e))?;
                if !entry.header().entry_type().is_file() {
                    continue;
                }
                let path = entry
                    .path()
                    .map_err(|e| damaged(&e))?
                    .to_string_lossy()
                    .into_owned();
                let Some(name) = wanted(asset, &path) else {
                    continue;
                };
                if !found.insert(name) {
                    return Err(damaged(&format!("{name} appears twice")));
                }
                let executable = entry.header().mode().is_ok_and(|mode| mode & 0o111 != 0);
                write_file(staging, name, &mut entry, executable)?;
            }
        }
        Archive::Zip => {
            let file = fs::File::open(archive).map_err(|e| e.to_string())?;
            let mut zip = zip::ZipArchive::new(file).map_err(|e| damaged(&e))?;
            for index in 0..zip.len() {
                let mut entry = zip.by_index(index).map_err(|e| damaged(&e))?;
                if !entry.is_file() || entry.is_symlink() {
                    continue;
                }
                let path = entry.name().to_owned();
                let Some(name) = wanted(asset, &path) else {
                    continue;
                };
                if !found.insert(name) {
                    return Err(damaged(&format!("{name} appears twice")));
                }
                write_file(staging, name, &mut entry, true)?;
            }
        }
    }
    if let Some(missing) = asset.files.iter().find(|file| !found.contains(*file)) {
        return Err(damaged(&format!("{missing} is missing")));
    }
    Ok(())
}

/// Rename the validated staging folder over the install folder. A previous
/// managed copy moves aside first and is restored if the swap fails.
fn activate(staging: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or("Install folder has no parent")?;
    let old = parent.join(temp_name("old"));
    let had_old = match fs::rename(target, &old) {
        Ok(()) => true,
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(format!("Could not replace the previous install: {e}")),
    };
    if let Err(e) = fs::rename(staging, target) {
        if had_old {
            let _ = fs::rename(&old, target);
        }
        return Err(format!("Could not finish the install: {e}"));
    }
    if had_old {
        let _ = fs::remove_dir_all(&old);
    }
    Ok(())
}

/// Unpack a verified archive, check the staged executable with `validate`,
/// then activate it. Returns the installed executable.
fn install_archive(
    root: &Path,
    tool: &Tool,
    asset: &Asset,
    archive: &Path,
    validate: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let dir = root.join(tool.id);
    let staging = dir.join(temp_name("staging"));
    let result = (|| {
        fs::create_dir(&staging).map_err(|e| e.to_string())?;
        extract(archive, asset, &staging)?;
        validate(&staging.join(asset.executable))?;
        let target = install_dir(root, tool, asset);
        activate(&staging, &target)?;
        Ok(target.join(asset.executable))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn version_matches(stdout: &str, line: &str) -> bool {
    stdout.lines().any(|candidate| candidate.trim() == line)
}

/// Run `<executable> --version` and require the pinned line. The child is
/// killed if it outlives the timeout.
fn check_version(tool: &Tool, executable: &Path) -> Result<(), String> {
    let mut command = std::process::Command::new(executable);
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    for (key, value) in tool.env {
        command.env(key, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("The downloaded driver couldn’t start on this computer: {e}"))?;
    let deadline = Instant::now() + VERSION_TIMEOUT;
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("The downloaded driver didn’t respond to a version check.".into());
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    let mut stdout = String::new();
    if let Some(pipe) = child.stdout.take() {
        let _ = pipe.take(16 * 1024).read_to_string(&mut stdout);
    }
    if !status.success() || !version_matches(&stdout, tool.version_line) {
        return Err(format!(
            "The downloaded driver didn’t report version {}. Nothing was changed.",
            tool.version
        ));
    }
    Ok(())
}

async fn download(
    asset: &Asset,
    path: &Path,
    cancelled: &watch::Receiver<bool>,
    progress: &(dyn Fn(InstallProgress) + Sync),
) -> Result<(), String> {
    let failed = |e: &dyn std::fmt::Display| {
        format!("The download failed: {e}. Check your connection and try again.")
    };
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("Codemux/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| failed(&e))?;
    let response = client.get(asset.url).send().await.map_err(|e| failed(&e))?;
    if !response.status().is_success() {
        return Err(failed(&format!("HTTP {}", response.status().as_u16())));
    }
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| e.to_string())?;
    let mut stream = response.bytes_stream();
    let mut received = 0u64;
    let mut reported = Instant::now();
    progress(InstallProgress::Downloading {
        received,
        total: asset.bytes,
    });
    while let Some(chunk) = stream.next().await {
        if *cancelled.borrow() {
            return Err("Installation was cancelled.".into());
        }
        let chunk = chunk.map_err(|e| failed(&e))?;
        received += chunk.len() as u64;
        if received > asset.bytes {
            return Err("The download was larger than the published release. Try again.".into());
        }
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|e| e.to_string())?;
        if reported.elapsed() >= Duration::from_millis(100) {
            reported = Instant::now();
            progress(InstallProgress::Downloading {
                received,
                total: asset.bytes,
            });
        }
    }
    tokio::io::AsyncWriteExt::flush(&mut file)
        .await
        .map_err(|e| e.to_string())?;
    progress(InstallProgress::Downloading {
        received,
        total: asset.bytes,
    });
    Ok(())
}

pub(super) async fn install(
    id: &str,
    cancelled: watch::Receiver<bool>,
    progress: impl Fn(InstallProgress) + Sync,
) -> Result<String, String> {
    let tool = tool(id)?;
    let asset = asset_for(tool, std::env::consts::OS, std::env::consts::ARCH)?;
    let _lock = InstallLock::take(tool.id)?;
    let root = tools_root()?;
    let dir = root.join(tool.id);
    private_dir(&root)?;
    private_dir(&dir)?;
    sweep(&dir);
    let archive = dir.join(temp_name("download"));
    let result = async {
        download(asset, &archive, &cancelled, &progress).await?;
        progress(InstallProgress::Verifying);
        let path = archive.clone();
        tokio::task::spawn_blocking(move || verify(&path, asset.sha256, asset.bytes))
            .await
            .map_err(|e| e.to_string())??;
        if *cancelled.borrow() {
            return Err("Installation was cancelled.".to_string());
        }
        progress(InstallProgress::Validating);
        let (root, path) = (root.clone(), archive.clone());
        let executable = tokio::task::spawn_blocking(move || {
            install_archive(&root, tool, asset, &path, |staged| {
                check_version(tool, staged)
            })
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(executable.to_string_lossy().into_owned())
    }
    .await;
    let _ = fs::remove_file(&archive);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: Asset = Asset {
        os: "linux",
        arch: "x86_64",
        label: "test",
        url: "https://example.invalid/fixture",
        sha256: "",
        bytes: 0,
        archive: Archive::TarGz,
        files: &["tool", "helper/readme.md"],
        executable: "tool",
    };
    const FIXTURE_TOOL: Tool = Tool {
        id: "fixture",
        version: "1.0.0",
        version_line: "fixture 1.0.0",
        env: &[],
        assets: &[],
        unsupported: &[],
    };

    fn tarball(dir: &Path, entries: &[(&str, &[u8], u32)]) -> PathBuf {
        let path = dir.join("fixture.tar.gz");
        let gz = flate2::write::GzEncoder::new(
            fs::File::create(&path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut tar = tar::Builder::new(gz);
        for (name, data, mode) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(*mode);
            header.set_cksum();
            tar.append_data(&mut header, name, *data).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        path
    }

    #[test]
    fn plugin_tools_pin_every_cua_driver_platform_and_refuse_macos_honestly() {
        let cua = tool("cua-driver").unwrap();
        for (os, arch, label) in [
            ("linux", "x86_64", "linux-x86_64"),
            ("linux", "aarch64", "linux-arm64"),
            ("windows", "x86_64", "windows-x86_64"),
            ("windows", "aarch64", "windows-arm64"),
        ] {
            let asset = asset_for(cua, os, arch).unwrap();
            assert_eq!(asset.label, label);
            assert!(asset.url.starts_with(
                "https://github.com/trycua/cua/releases/download/cua-driver-rs-v0.21.0/"
            ));
            assert!(asset.url.contains(label) && asset.url.contains("-binary."));
            assert_eq!(asset.sha256.len(), 64);
            assert!(asset.files.contains(&asset.executable));
        }
        for arch in ["x86_64", "aarch64"] {
            let reason = asset_for(cua, "macos", arch).unwrap_err();
            assert!(reason.contains("/Applications"), "{reason}");
        }
        assert!(tool("anything-else").is_err());
    }

    #[test]
    fn plugin_tools_status_reports_only_an_installed_managed_copy() {
        let root = tempfile::tempdir().unwrap();
        let cua = tool("cua-driver").unwrap();
        let missing = status_at(root.path(), cua, "linux", "x86_64");
        assert!(missing.supported && missing.executable.is_none());
        let dir = root.path().join("cua-driver/0.21.0-linux-x86_64");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("cua-driver"), "bin").unwrap();
        let installed = status_at(root.path(), cua, "linux", "x86_64");
        assert_eq!(
            installed.executable,
            Some(dir.join("cua-driver").to_string_lossy().into_owned())
        );
        let mac = status_at(root.path(), cua, "macos", "aarch64");
        assert!(!mac.supported && mac.executable.is_none() && mac.unsupported_reason.is_some());
    }

    #[test]
    fn plugin_tools_verify_rejects_wrong_size_or_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("download");
        fs::write(&path, b"release").unwrap();
        let sha = format!("{:x}", Sha256::digest(b"release"));
        assert!(verify(&path, &sha, 7).is_ok());
        assert!(verify(&path, &sha, 8).is_err());
        assert!(verify(&path, &"0".repeat(64), 7).is_err());
    }

    #[test]
    fn plugin_tools_install_copies_only_pinned_files_and_activates_atomically() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("fixture")).unwrap();
        let archive = tarball(
            root.path(),
            &[
                ("tool", b"#!/bin/sh\n", 0o755),
                ("helper/readme.md", b"notes", 0o644),
                ("libextra.so", b"skip", 0o755),
            ],
        );
        // A previous managed copy is replaced; nothing outside the tool folder changes.
        let target = root.path().join("fixture/1.0.0-test");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("stale"), "old").unwrap();
        let mut validated = None;
        let executable = install_archive(root.path(), &FIXTURE_TOOL, &FIXTURE, &archive, |path| {
            validated = Some(path.to_path_buf());
            Ok(())
        })
        .unwrap();
        assert_eq!(executable, target.join("tool"));
        assert!(validated
            .unwrap()
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".staging-"));
        assert_eq!(
            fs::read_to_string(target.join("helper/readme.md")).unwrap(),
            "notes"
        );
        assert!(!target.join("stale").exists() && !target.join("libextra.so").exists());
        // The tar builder refuses to write traversal names, so check the filter directly.
        for name in ["../tool", "/tool", "helper/../tool", "./helper/../../tool"] {
            assert_eq!(wanted(&FIXTURE, name), None, "{name}");
        }
        assert_eq!(wanted(&FIXTURE, "./tool"), Some("tool"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(target.join("tool"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
            assert_eq!(
                fs::metadata(target.join("helper/readme.md"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o644
            );
        }
        let leftovers: Vec<_> = fs::read_dir(root.path().join("fixture"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, vec!["1.0.0-test".to_string()]);
    }

    #[test]
    fn plugin_tools_failed_validation_keeps_the_previous_install() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("fixture")).unwrap();
        let archive = tarball(
            root.path(),
            &[("tool", b"new", 0o755), ("helper/readme.md", b"n", 0o644)],
        );
        let target = root.path().join("fixture/1.0.0-test");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("tool"), "old").unwrap();
        let error = install_archive(root.path(), &FIXTURE_TOOL, &FIXTURE, &archive, |_| {
            Err("wrong version".into())
        })
        .unwrap_err();
        assert_eq!(error, "wrong version");
        assert_eq!(fs::read_to_string(target.join("tool")).unwrap(), "old");
        assert_eq!(
            fs::read_dir(root.path().join("fixture")).unwrap().count(),
            1
        );
    }

    #[test]
    fn plugin_tools_install_requires_every_pinned_file_and_skips_links() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("fixture")).unwrap();
        let path = root.path().join("linked.tar.gz");
        let gz = flate2::write::GzEncoder::new(
            fs::File::create(&path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut tar = tar::Builder::new(gz);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        tar.append_link(&mut header, "tool", "/bin/sh").unwrap();
        let mut file = tar::Header::new_gnu();
        file.set_size(1);
        file.set_mode(0o644);
        file.set_cksum();
        tar.append_data(&mut file, "helper/readme.md", &b"n"[..])
            .unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        let error =
            install_archive(root.path(), &FIXTURE_TOOL, &FIXTURE, &path, |_| Ok(())).unwrap_err();
        assert!(error.contains("tool is missing"), "{error}");
        assert!(!root.path().join("fixture/1.0.0-test").exists());
        assert_eq!(
            fs::read_dir(root.path().join("fixture")).unwrap().count(),
            0
        );
    }

    #[test]
    fn plugin_tools_zip_extraction_uses_the_same_allow_list() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("fixture")).unwrap();
        let path = root.path().join("fixture.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in [
            ("tool", "exe"),
            ("helper/readme.md", "n"),
            ("extra.dll", "x"),
        ] {
            zip.start_file(name, options).unwrap();
            zip.write_all(data.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let asset = Asset {
            archive: Archive::Zip,
            ..FIXTURE
        };
        let executable =
            install_archive(root.path(), &FIXTURE_TOOL, &asset, &path, |_| Ok(())).unwrap();
        assert_eq!(fs::read_to_string(executable).unwrap(), "exe");
        assert!(!root.path().join("fixture/1.0.0-test/extra.dll").exists());
    }

    #[test]
    fn plugin_tools_version_check_needs_the_exact_line() {
        assert!(version_matches("cua-driver 0.21.0\n", "cua-driver 0.21.0"));
        assert!(!version_matches("cua-driver 0.21.1\n", "cua-driver 0.21.0"));
        assert!(!version_matches(
            "cua-driver 0.21.0-beta\n",
            "cua-driver 0.21.0"
        ));
    }

    #[test]
    fn plugin_tools_sweep_removes_only_old_install_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(2 * 86400);
        // Leftovers are files here; sweep treats folders the same way.
        for name in [
            ".download-a",
            ".staging-b",
            ".old-c",
            "0.21.0-linux-x86_64",
            ".download-new",
        ] {
            let path = dir.path().join(name);
            fs::write(&path, "x").unwrap();
            if name != ".download-new" {
                let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
                file.set_modified(old).unwrap();
            }
        }
        sweep(dir.path());
        let mut left: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            vec![
                ".download-new".to_string(),
                "0.21.0-linux-x86_64".to_string()
            ]
        );
    }

    /// Live check against the real release: `cargo test plugin_tools_live -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn plugin_tools_live_cua_driver_release_installs_on_this_platform() {
        let cua = tool("cua-driver").unwrap();
        let asset = asset_for(cua, std::env::consts::OS, std::env::consts::ARCH).unwrap();
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(cua.id)).unwrap();
        let archive = root.path().join("download");
        let (_tx, cancelled) = watch::channel(false);
        download(asset, &archive, &cancelled, &|_| {})
            .await
            .unwrap();
        verify(&archive, asset.sha256, asset.bytes).unwrap();
        let staging = root.path().join(cua.id).join(temp_name("staging"));
        fs::create_dir(&staging).unwrap();
        extract(&archive, asset, &staging).unwrap();
        check_version(cua, &staging.join(asset.executable)).unwrap();
        let target = install_dir(root.path(), cua, asset);
        activate(&staging, &target).unwrap();
        assert!(target.join(asset.executable).is_file());
    }
}
