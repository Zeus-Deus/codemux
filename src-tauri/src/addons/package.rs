//! Inert package validation. No tar entry is ever executed or extracted by tar.
use super::{ErrorCode, Manifest, ProtocolError, Result};
use codemux_addon_protocol::{limits, manifest::Platform};
use flate2::bufread::GzDecoder;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    io::{Cursor, Read},
    path::Path,
};
pub struct Package {
    pub manifest: Manifest,
    pub digest: String,
    pub archive: Vec<u8>,
    pub files: BTreeMap<String, Vec<u8>>,
}
impl Package {
    pub fn read(path: &Path, expected: Option<&str>) -> Result<Self> {
        let file = std::fs::File::open(path)
            .map_err(|_| ProtocolError::invalid("Cannot read add-on package"))?;
        let mut bytes = Vec::new();
        file.take(10 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ProtocolError::invalid("Cannot read add-on package"))?;
        Self::parse(bytes, expected)
    }
    pub fn parse(archive: Vec<u8>, expected: Option<&str>) -> Result<Self> {
        let platform = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Platform::LinuxX64
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            Platform::WindowsX64
        } else {
            return Err(ProtocolError::new(
                ErrorCode::IncompatibleApi,
                "This platform is not supported by add-ons",
            ));
        };
        Self::parse_for_platform(archive, expected, Some(platform))
    }
    pub fn parse_for_platform(
        archive: Vec<u8>,
        expected: Option<&str>,
        platform: Option<Platform>,
    ) -> Result<Self> {
        if archive.len() > 10 * 1024 * 1024 {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "Archive exceeds 10 MiB",
            ));
        }
        let digest = format!("{:x}", Sha256::digest(&archive));
        if expected.is_some_and(|expected| expected != digest) {
            return Err(ProtocolError::invalid(
                "Package digest does not match the accepted release",
            ));
        }
        let mut decoder = GzDecoder::new(Cursor::new(&archive));
        let mut expanded = Vec::new();
        (&mut decoder)
            .take(30 * 1024 * 1024 + 1)
            .read_to_end(&mut expanded)
            .map_err(|_| ProtocolError::invalid("Invalid gzip archive"))?;
        if expanded.len() > 30 * 1024 * 1024 {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "Archive expands beyond 30 MiB",
            ));
        }
        if decoder.get_ref().position() as usize != archive.len() {
            return Err(ProtocolError::invalid(
                "Trailing compressed archive content",
            ));
        }
        let mut tar = tar::Archive::new(Cursor::new(&expanded));
        let mut files = BTreeMap::new();
        let mut names = HashSet::new();
        for entry in tar
            .entries()
            .map_err(|_| ProtocolError::invalid("Invalid tar archive"))?
            .raw(true)
        {
            let mut entry = entry.map_err(|_| ProtocolError::invalid("Invalid tar entry"))?;
            if !entry.header().entry_type().is_file() {
                return Err(ProtocolError::invalid(
                    "Links, directories, and special archive entries are unsupported",
                ));
            }
            let path = entry.path_bytes();
            let name = std::str::from_utf8(&path)
                .map_err(|_| ProtocolError::invalid("Invalid archive path"))?
                .to_string();
            if ![
                "manifest.json",
                "plugin.js",
                "README.md",
                "LICENSE",
                "NOTICE",
                "source.map",
            ]
            .contains(&name.as_str())
                || !names.insert(name.to_ascii_lowercase())
                || names.len() > 64
            {
                return Err(ProtocolError::invalid(
                    "Unexpected, duplicate, or unsafe archive path",
                ));
            }
            let max = match name.as_str() {
                "plugin.js" => limits::BUNDLE,
                "manifest.json" => limits::MANIFEST,
                _ => 30 * 1024 * 1024,
            };
            if entry.size() > max as u64 {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceLimit,
                    "Archive entry exceeds its limit",
                ));
            }
            let mut contents = Vec::new();
            entry
                .read_to_end(&mut contents)
                .map_err(|_| ProtocolError::invalid("Truncated archive entry"))?;
            files.insert(name, contents);
        }
        let consumed = tar.into_inner().position() as usize;
        if expanded[consumed..].iter().any(|b| *b != 0) {
            return Err(ProtocolError::invalid(
                "Unexpected content after tar terminator",
            ));
        }
        for name in ["manifest.json", "plugin.js", "README.md", "LICENSE"] {
            if !files.contains_key(name) {
                return Err(ProtocolError::invalid("Package is missing required files"));
            }
        }
        let manifest = Manifest::parse(&files["manifest.json"], platform)?;
        std::str::from_utf8(&files["plugin.js"])
            .map_err(|_| ProtocolError::invalid("Plugin source must be UTF-8 JavaScript"))?;
        Ok(Self {
            manifest,
            digest,
            archive,
            files,
        })
    }
    pub fn source(&self) -> String {
        String::from_utf8(self.files["plugin.js"].clone()).expect("validated UTF-8")
    }
}
#[cfg(test)]
pub(super) fn fixture_archive() -> Vec<u8> {
    tests::archive(None)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    pub(super) fn archive(extra: Option<(&str, tar::EntryType)>) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (name, contents) in [
            (
                "manifest.json",
                include_bytes!("../../addon-protocol/fixtures/hello.json").as_slice(),
            ),
            ("plugin.js", b"globalThis.__shouldNotRun = true;".as_slice()),
            ("README.md", b"readme".as_slice()),
            ("LICENSE", b"MIT".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o777);
            header.set_cksum();
            tar.append_data(&mut header, name, contents).unwrap();
        }
        if let Some((name, kind)) = extra {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(kind);
            header.set_size(0);
            header.set_mode(0o777);
            header.set_cksum();
            tar.append_data(&mut header, name, &[][..]).unwrap();
        }
        let bytes = tar.into_inner().unwrap();
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&bytes).unwrap();
        gzip.finish().unwrap()
    }
    #[test]
    fn package_validation_is_inert_and_checks_identity() {
        let package = Package::parse(archive(None), None).unwrap();
        assert!(package.source().contains("__shouldNotRun"));
        assert!(Package::parse(package.archive.clone(), Some("forged")).is_err());
        for (name, kind) in [
            ("CON", tar::EntryType::Regular),
            ("manifest.json", tar::EntryType::Regular),
            ("plugin.js", tar::EntryType::Symlink),
            ("unexpected.exe", tar::EntryType::Regular),
        ] {
            assert!(Package::parse(archive(Some((name, kind))), None).is_err());
        }
    }
    #[test]
    fn expansion_bombs_are_bounded() {
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        let block = vec![0u8; 1024 * 1024];
        for _ in 0..31 {
            gzip.write_all(&block).unwrap()
        }
        assert!(Package::parse(gzip.finish().unwrap(), None).is_err());
    }
}
