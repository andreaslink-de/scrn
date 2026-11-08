//! The last uploads in `history.json` (see [`crate::paths`]), newest
//! first.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{self, ConfigError};

/// How many uploads the history keeps.
pub const LEN: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// UUID of the screenshot on the server.
    pub id: String,
    /// The short link of a public upload, otherwise the gallery page.
    pub url: String,
    pub public: bool,
    /// Unix seconds of the upload.
    pub created_at: i64,
}

pub fn path() -> Option<PathBuf> {
    crate::paths::history_file()
}

/// Loads the history; a missing or damaged file gives an empty one.
pub fn load() -> Vec<Entry> {
    path().map(|p| load_from(&p)).unwrap_or_default()
}

pub fn load_from(path: &Path) -> Vec<Entry> {
    std::fs::read(path)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

pub fn save(entries: &[Entry]) -> Result<(), ConfigError> {
    let path = path().ok_or_else(|| std::io::Error::other("no home directory"))?;
    save_to(&path, entries)
}

pub fn save_to(path: &Path, entries: &[Entry]) -> Result<(), ConfigError> {
    config::write_atomic(path, &serde_json::to_vec_pretty(entries)?)
}

/// Puts `entry` first, drops an older entry of the same screenshot and
/// keeps at most [`LEN`] entries.
pub fn push(entries: &mut Vec<Entry>, entry: Entry) {
    entries.retain(|e| e.id != entry.id);
    entries.insert(0, entry);
    entries.truncate(LEN);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: usize) -> Entry {
        Entry {
            id: format!("id-{n}"),
            url: format!("https://scrn.dev/{n}"),
            public: n.is_multiple_of(2),
            created_at: n as i64,
        }
    }

    #[test]
    fn push_keeps_the_newest_ten_without_duplicates() {
        let mut entries = Vec::new();
        for n in 0..12 {
            push(&mut entries, entry(n));
        }
        assert_eq!(entries.len(), LEN);
        assert_eq!(entries[0].id, "id-11");
        assert_eq!(entries[LEN - 1].id, "id-2");
        push(&mut entries, entry(5));
        assert_eq!(entries.len(), LEN);
        assert_eq!(entries[0].id, "id-5");
        assert_eq!(entries.iter().filter(|e| e.id == "id-5").count(), 1);
    }

    #[test]
    fn history_round_trips_and_ignores_damage() {
        let dir = config::tests::tempdir();
        let path = dir.join("history.json");
        assert!(load_from(&path).is_empty());
        let entries = vec![entry(1), entry(2)];
        save_to(&path, &entries).unwrap();
        assert_eq!(load_from(&path), entries);
        std::fs::write(&path, "{broken").unwrap();
        assert!(load_from(&path).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
