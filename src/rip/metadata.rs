use std::collections::BTreeMap;
use std::io::Read;
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use crate::error::Error;

#[derive(Debug, Serialize, Deserialize)]
pub struct ChecksumManifest {
    pub version: u8,
    pub files: BTreeMap<String, FileChecksum>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileChecksum {
    pub sha256: String,
    pub size_bytes: u64,
}

/// Generate a SHA256 checksum manifest for all regular files in `dir`.
/// `relative_to` is the base path for storing relative keys in the manifest.
pub fn generate_checksums(dir: &str) -> Result<ChecksumManifest, Error> {
    let mut files: BTreeMap<String, FileChecksum> = BTreeMap::new();
    collect_files(dir, dir, &mut files)?;
    Ok(ChecksumManifest { version: 1, files })
}

fn collect_files(
    base: &str,
    dir: &str,
    out: &mut BTreeMap<String, FileChecksum>,
) -> Result<(), Error> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(base, path.to_str().unwrap_or(""), out)?;
        } else if path.is_file() {
            let rel = path.strip_prefix(base)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| path.to_string_lossy().to_string());

            // Skip the manifest itself
            if rel == "checksums.json" { continue; }

            let (sha256, size) = hash_file(path.to_str().unwrap_or(""))?;
            out.insert(rel, FileChecksum { sha256, size_bytes: size });
        }
    }
    Ok(())
}

fn hash_file(path: &str) -> Result<(String, u64), Error> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    let mut total = 0u64;

    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
        total += n as u64;
    }

    let hash = hex::encode(hasher.finalize());
    Ok((hash, total))
}

/// Write the manifest to `<dir>/checksums.json`.
pub fn write_checksums(manifest: &ChecksumManifest, dir: &str) -> Result<(), Error> {
    let path = format!("{}/checksums.json", dir);
    let json = serde_json::to_string_pretty(manifest)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Verify all files in `dir` against an existing `checksums.json`.
/// Returns (passed, failed) counts; prints failures to stderr.
pub fn verify_checksums(dir: &str) -> Result<VerifyResult, Error> {
    let manifest_path = format!("{}/checksums.json", dir);
    let json = std::fs::read_to_string(&manifest_path).map_err(|_| {
        Error::validation(format!("No checksums.json found in {}", dir))
    })?;

    let manifest: ChecksumManifest = serde_json::from_str(&json)?;

    let mut passed = 0usize;
    let mut failed: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();

    for (rel, expected) in &manifest.files {
        let full_path = format!("{}/{}", dir, rel);
        if !std::path::Path::new(&full_path).exists() {
            missing.push(rel.clone());
            continue;
        }
        match hash_file(&full_path) {
            Ok((hash, size)) => {
                if hash == expected.sha256 && size == expected.size_bytes {
                    passed += 1;
                } else {
                    failed.push(rel.clone());
                }
            }
            Err(_) => failed.push(rel.clone()),
        }
    }

    Ok(VerifyResult { passed, failed, missing })
}

#[derive(Debug)]
pub struct VerifyResult {
    pub passed: usize,
    pub failed: Vec<String>,
    pub missing: Vec<String>,
}

impl VerifyResult {
    pub fn is_ok(&self) -> bool {
        self.failed.is_empty() && self.missing.is_empty()
    }
}
