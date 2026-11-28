use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// What the one-time Margin -> Milkrice data migration did on this launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Migration {
    /// Nothing to do (new install, already migrated, or MILKRICE_HOME in use).
    NotNeeded,
    /// The old directory was renamed (and its files renamed inside it).
    Migrated,
    /// The directory rename failed; the old directory is untouched and a fresh one is used.
    Failed(String),
}

static MIGRATION: OnceLock<Migration> = OnceLock::new();

/// Result of the migration run by the first `data_dir()` call (for logging once logging is up).
pub fn migration_outcome() -> Migration {
    MIGRATION.get().cloned().unwrap_or(Migration::NotNeeded)
}

/// One-time rename of `<base>/Margin` to `<base>/Milkrice`, plus margin.sqlite(-wal/-shm) and
/// margin.log inside it. Only runs when Milkrice does not exist and Margin does. The directory
/// rename is atomic (same volume); if it fails nothing is touched. User data is never deleted.
pub fn migrate_legacy_data(base: &Path) -> Migration {
    let old = base.join("Margin");
    let new = base.join("Milkrice");
    if new.exists() || !old.is_dir() {
        return Migration::NotNeeded;
    }
    if let Err(e) = std::fs::rename(&old, &new) {
        return Migration::Failed(e.to_string());
    }
    for (from, to) in [
        ("margin.sqlite", "milkrice.sqlite"),
        ("margin.sqlite-wal", "milkrice.sqlite-wal"),
        ("margin.sqlite-shm", "milkrice.sqlite-shm"),
        ("margin.log", "milkrice.log"),
        ("margin.log.1", "milkrice.log.1"),
    ] {
        let (f, t) = (new.join(from), new.join(to));
        // Never overwrite an existing file.
        if f.exists() && !t.exists() {
            if let Err(e) = std::fs::rename(&f, &t) {
                eprintln!("could not rename {} to {}: {e}", f.display(), t.display());
            }
        }
    }
    Migration::Migrated
}

/// ~/Library/Application Support/Milkrice (or MILKRICE_HOME for tests).
pub fn data_dir() -> PathBuf {
    let dir = match std::env::var_os("MILKRICE_HOME") {
        Some(d) => PathBuf::from(d),
        None => {
            let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
            let outcome = MIGRATION.get_or_init(|| {
                let m = migrate_legacy_data(&base);
                if let Migration::Failed(e) = &m {
                    eprintln!("could not migrate the old Margin data folder, leaving it untouched: {e}");
                }
                m
            });
            let _ = outcome;
            let dir = base.join("Milkrice");
            let _ = std::fs::create_dir_all(&dir);
            restrict_to_owner(&dir);
            return dir;
        }
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// The data folder holds transcripts, notes, recordings and the log: only its owner may open
/// it, whatever the umask or the permissions of the folders above it.
fn restrict_to_owner(dir: &Path) {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| owner_only(dir));
}

fn owner_only(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(m) = std::fs::metadata(dir) {
        if m.permissions().mode() & 0o077 != 0 {
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
}

pub fn models_dir() -> PathBuf {
    let d = data_dir().join("models");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn audio_dir() -> PathBuf {
    let d = data_dir().join("audio");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn db_path() -> PathBuf {
    data_dir().join("milkrice.sqlite")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("milkrice-paths-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn migrates_old_dir_and_files() {
        let base = tmp();
        let old = base.join("Margin");
        std::fs::create_dir_all(old.join("models")).unwrap();
        for (n, c) in [("margin.sqlite", "db"), ("margin.sqlite-wal", "wal"), ("margin.sqlite-shm", "shm"), ("margin.log", "log")] {
            std::fs::write(old.join(n), c).unwrap();
        }
        assert_eq!(migrate_legacy_data(&base), Migration::Migrated);
        let new = base.join("Milkrice");
        assert!(!old.exists());
        assert_eq!(std::fs::read_to_string(new.join("milkrice.sqlite")).unwrap(), "db");
        assert_eq!(std::fs::read_to_string(new.join("milkrice.sqlite-wal")).unwrap(), "wal");
        assert_eq!(std::fs::read_to_string(new.join("milkrice.sqlite-shm")).unwrap(), "shm");
        assert_eq!(std::fs::read_to_string(new.join("milkrice.log")).unwrap(), "log");
        assert!(new.join("models").is_dir());
        assert!(!new.join("margin.sqlite").exists());
        // Second run is a no-op.
        assert_eq!(migrate_legacy_data(&base), Migration::NotNeeded);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn leaves_everything_when_milkrice_exists() {
        let base = tmp();
        std::fs::create_dir_all(base.join("Margin")).unwrap();
        std::fs::write(base.join("Margin/margin.sqlite"), "old").unwrap();
        std::fs::create_dir_all(base.join("Milkrice")).unwrap();
        assert_eq!(migrate_legacy_data(&base), Migration::NotNeeded);
        assert!(base.join("Margin/margin.sqlite").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn data_folder_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let base = tmp();
        let dir = base.join("Milkrice");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        owner_only(&dir);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn nothing_to_do_on_fresh_install() {
        let base = tmp();
        assert_eq!(migrate_legacy_data(&base), Migration::NotNeeded);
        assert!(!base.join("Milkrice").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn failed_rename_keeps_old_dir() {
        let base = tmp();
        // A regular file named "Margin" is not a directory, so no migration is attempted.
        std::fs::write(base.join("Margin"), "x").unwrap();
        assert_eq!(migrate_legacy_data(&base), Migration::NotNeeded);
        assert!(base.join("Margin").exists());
        // Read-only base: the rename fails and the old dir is left in place.
        let base2 = tmp();
        std::fs::create_dir_all(base2.join("Margin")).unwrap();
        std::fs::write(base2.join("Margin/margin.sqlite"), "old").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&base2, std::fs::Permissions::from_mode(0o555)).unwrap();
        let r = migrate_legacy_data(&base2);
        std::fs::set_permissions(&base2, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(r, Migration::Failed(_)), "{r:?}");
        assert!(base2.join("Margin/margin.sqlite").exists());
        assert!(!base2.join("Milkrice").exists());
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base2);
    }
}
