//! File-level persistence. This does not make playerdata and campaign writes one transaction.
use super::Registry;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_BYTES: u64 = 4 * 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Campaign exceeds 4 MiB",
        ));
    }
    Ok(bytes)
}

/// A missing file is a new campaign; malformed or unsupported files remain an error.
pub fn load(path: &Path) -> Result<Registry, String> {
    match read_bounded(path) {
        Ok(bytes) => Registry::restore(&bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Registry::default()),
        Err(error) => Err(error.to_string()),
    }
}

fn preserve_legacy(path: &Path) -> Result<(), String> {
    let bytes = match read_bounded(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    // Never overwrite an unreadable/future campaign, even through a direct save call.
    Registry::restore(&bytes)?;
    let version =
        serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| e.to_string())?["version"]
            .as_u64();
    if !matches!(version, Some(1 | 2 | 3 | 4 | 5 | 6 | 7)) {
        return Ok(());
    }
    let backup = path.with_extension(format!("json.v{}.bak", version.unwrap()));
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
    {
        Ok(mut file) => {
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|e| e.to_string())?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if read_bounded(&backup).map_err(|e| e.to_string())? != bytes {
                return Err("Legacy backup already exists with different contents".into());
            }
        }
        Err(error) => return Err(error.to_string()),
    }
    Ok(())
}

/// Flush a unique sibling before replacement. Failed writes never truncate the old file.
/// Keep a byte-for-byte v1 backup before the first migrated write.
pub fn save(path: &Path, registry: &Registry) -> Result<(), String> {
    let bytes = registry.save()?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Campaign exceeds 4 MiB".into());
    }
    preserve_legacy(path)?;
    let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("json.{}.{serial}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    let result = file.write_all(&bytes).and_then(|_| file.sync_all());
    drop(file); // Windows replacement requires our temporary handle to be closed.
    let result = result.and_then(|_| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::civilization::{Bell, VillageEvidence};
    use std::path::PathBuf;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "civilization-store-{}-{serial}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn campaign(&self) -> PathBuf {
            self.0.join("horizon-civilization.json")
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn founded() -> Registry {
        let mut state = Registry::default();
        state.begin_player("alice").unwrap();
        state
            .found(
                "alice",
                "A",
                Bell {
                    dimension: "minecraft:overworld".into(),
                    x: 0,
                    y: 64,
                    z: 0,
                },
                VillageEvidence {
                    beds: 2,
                    residents: 1,
                },
            )
            .unwrap();
        state
    }

    #[test]
    fn legacy_migration_preserves_original_bytes_and_supports_repeated_saves() {
        let dir = Scratch::new();
        let path = dir.campaign();
        let mut legacy = serde_json::to_value(founded()).unwrap();
        legacy["version"] = 1.into();
        legacy.as_object_mut().unwrap().remove("choices");
        legacy.as_object_mut().unwrap().remove("settlements");
        legacy.as_object_mut().unwrap().remove("next_settlement");
        let city = legacy["civilizations"]["civ-1"].as_object_mut().unwrap();
        city.remove("leader");
        city.remove("former_members");
        city.remove("settlement_id");
        city.remove("governance");
        city.remove("economy");
        city.remove("population");
        for quest in city["quests"].as_array_mut().unwrap() {
            quest["npc_key"] = format!("civ-1.{}", quest["id"].as_str().unwrap()).into();
        }
        let original = serde_json::to_vec_pretty(&legacy).unwrap();
        fs::write(&path, &original).unwrap();
        let mut state = load(&path).unwrap();
        assert_eq!(state.civilization("civ-1").unwrap().leader, "alice");
        state.complete_quest("alice", "granary").unwrap();
        save(&path, &state).unwrap();
        save(&path, &state).unwrap();
        assert_eq!(load(&path).unwrap(), state);
        assert_eq!(
            fs::read(path.with_extension("json.v1.bak")).unwrap(),
            original
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap()).unwrap()["version"],
            8
        );
    }

    #[test]
    fn invalid_or_future_campaign_is_never_replaced() {
        let dir = Scratch::new();
        let path = dir.campaign();
        for bytes in [b"{broken".as_slice(), b"{\"version\":999}".as_slice()] {
            fs::write(&path, bytes).unwrap();
            assert!(load(&path).is_err());
            assert!(save(&path, &founded()).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn write_failure_does_not_damage_previous_campaign() {
        let dir = Scratch::new();
        let path = dir.campaign();
        save(&path, &founded()).unwrap();
        let original = fs::read(&path).unwrap();
        let mut invalid = founded();
        invalid.civilizations.get_mut("civ-1").unwrap().recognition = 60;
        assert!(save(&path, &invalid).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        // Portable, deterministic I/O failure: the parent is a file, not a directory.
        assert!(save(&path.join("child.json"), &founded()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn failed_replacement_keeps_old_state_and_cleans_only_its_own_temporary() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = Scratch::new();
        let path = dir.campaign();
        let mut state = founded();
        save(&path, &state).unwrap();
        let original = fs::read(&path).unwrap();
        // Permit reading/writing but deny deletion/replacement while this handle is open.
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        state.complete_quest("alice", "granary").unwrap();
        assert!(save(&path, &state).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        drop(lock);
        save(&path, &state).unwrap();
        assert_eq!(load(&path).unwrap(), state);
    }
}
