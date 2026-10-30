//! The data file of a prepared resource folder, built from the files in it.
//! `horizon-prepare` and the game (when the file is missing or stale) call the
//! same [`write`], and the game's loaders list their source files through the
//! same [`json_files`], so the ids are the same by construction.
use crate::{DataFile, FILE_NAME, OwnedTable, Writer, invalid};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant, UNIX_EPOCH},
};

/// `data/<ns>/loot_table/**.json` of every namespace: the exported bytes by id.
pub const LOOT_TABLES: &str = "loot_tables";

/// Every `<data>/<ns>/<kind>/**.json` as (`ns:path`, file), namespaces in name
/// order.
pub fn json_files(data: &Path, kind: &str) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(data) else {
        return Vec::new();
    };
    let mut namespaces: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .collect();
    namespaces.sort();
    let mut out = Vec::new();
    for ns in namespaces {
        json_dir(&data.join(&ns).join(kind), &ns, "", &mut out);
    }
    out
}

/// The `.json` files under `dir` as (`namespace:prefix/path`, file).
pub fn json_dir(dir: &Path, namespace: &str, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.is_dir() {
            json_dir(&path, namespace, &format!("{prefix}{stem}/"), out);
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push((format!("{namespace}:{prefix}{stem}"), path));
        }
    }
}

/// The loot table files of `root`, one per id (the last listed, as a map
/// keeps it), in id order.
fn loot_files(root: &Path) -> Vec<(String, PathBuf)> {
    let files: HashMap<String, PathBuf> = json_files(&root.join("data"), "loot_table")
        .into_iter()
        .collect();
    let mut files: Vec<_> = files.into_iter().collect();
    files.sort();
    files
}

/// What a data file is built from: each source file's id, size and
/// modification time. A changed, added or removed file makes it stale.
fn key(files: &[(String, PathBuf)]) -> io::Result<[u8; 32]> {
    let mut hash = Sha256::new();
    hash.update(LOOT_TABLES);
    for (id, path) in files {
        let meta = fs::metadata(path)?;
        let modified = meta
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        hash.update([0]);
        hash.update(id);
        hash.update([0]);
        hash.update(meta.len().to_le_bytes());
        hash.update(modified.to_le_bytes());
    }
    Ok(hash.finalize().into())
}

/// Write `root`'s data file from the files in it and return how many loot
/// tables it holds. Every table must parse here, so a bad one still fails
/// at load and never at its first use. A root without loot tables gets no
/// file.
pub fn write(root: &Path) -> io::Result<usize> {
    let files = loot_files(root);
    if files.is_empty() {
        return Ok(0);
    }
    let key = key(&files)?;
    let mut entries = Vec::with_capacity(files.len());
    for (id, path) in files {
        let bytes = fs::read(&path)?;
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .map_err(|e| invalid(format!("{}: {e}", path.display())))?;
        entries.push((id.into_bytes(), bytes));
    }
    let count = entries.len();
    let mut writer = Writer::new(key);
    writer.table(LOOT_TABLES, entries)?;
    writer.write(&root.join(FILE_NAME))?;
    Ok(count)
}

/// `root`'s loot tables from its data file, if the file is current.
pub fn loot_tables(root: &Path) -> io::Result<OwnedTable> {
    let key = key(&loot_files(root))?;
    DataFile::open(&root.join(FILE_NAME), &key)?.into_table(LOOT_TABLES)
}

/// What [`verify`] found.
pub struct Verified {
    pub tables: usize,
    /// The slowest single table parse and its id: what a first use costs at most.
    pub slowest: (Duration, String),
}

/// Parse every table of `root`'s data file and compare it with its source
/// file's parse.
pub fn verify(root: &Path) -> io::Result<Verified> {
    let files = loot_files(root);
    let mapped = loot_tables(root)?;
    let table = mapped.table();
    if table.len() != files.len() {
        return Err(invalid(format!(
            "{} loot tables in {FILE_NAME}, {} files",
            table.len(),
            files.len()
        )));
    }
    let mut slowest = (Duration::ZERO, String::new());
    for (id, path) in &files {
        let bytes = table
            .get(id.as_bytes())
            .ok_or_else(|| invalid(format!("{id} missing from {FILE_NAME}")))?;
        let start = Instant::now();
        let value: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|e| invalid(format!("{id} in {FILE_NAME}: {e}")))?;
        let took = start.elapsed();
        let source: serde_json::Value = serde_json::from_slice(&fs::read(path)?)
            .map_err(|e| invalid(format!("{}: {e}", path.display())))?;
        if value != source {
            return Err(invalid(format!("{id} differs from {}", path.display())));
        }
        if took > slowest.0 {
            slowest = (took, id.clone());
        }
    }
    Ok(Verified {
        tables: files.len(),
        slowest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("horizon-prepared-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("data/minecraft/loot_table/blocks");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("stone.json"), br#"{"pools":[]}"#).unwrap();
        fs::write(root.join("data/minecraft/loot_table/empty.json"), b"{}").unwrap();
        root
    }

    #[test]
    fn writes_maps_verifies_and_goes_stale() {
        let root = root("stale");
        assert_eq!(write(&root).unwrap(), 2);
        let mapped = loot_tables(&root).unwrap();
        assert_eq!(
            mapped.table().get(b"minecraft:blocks/stone"),
            Some(&br#"{"pools":[]}"#[..])
        );
        assert_eq!(verify(&root).unwrap().tables, 2);
        // A new table makes the file stale until it is written again.
        fs::write(root.join("data/minecraft/loot_table/more.json"), b"[]").unwrap();
        assert!(loot_tables(&root).is_err());
        assert_eq!(write(&root).unwrap(), 3);
        assert!(loot_tables(&root).is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bad_table_fails_the_write() {
        let root = root("bad");
        fs::write(root.join("data/minecraft/loot_table/bad.json"), b"{").unwrap();
        assert!(write(&root).is_err());
        assert!(!root.join(FILE_NAME).exists());
        let _ = fs::remove_dir_all(&root);
    }
}
