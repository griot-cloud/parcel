//! The dataset manifest, written next to the data on every write (design 10, stage 6).

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const MANIFEST_FILE: &str = "_parcel_manifest.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlagStatus {
    AllPass,
    AllFail,
    Mixed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Path relative to the binding root.
    pub path: String,
    pub rows: i64,
    pub bytes: u64,
    /// The contract hash the file was written under.
    pub contract_hash: String,
    /// Per assert flag column, whether every row passes, fails, or both.
    pub flags: BTreeMap<String, FlagStatus>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub contract: String,
    pub contract_hash: String,
    pub compilation_hash: String,
    pub written_at: DateTime<Utc>,
    pub row_count: i64,
    /// Whether the data satisfies every deny-level rule: the validation verdict.
    pub valid: bool,
    pub breached: Vec<String>,
    /// Every statistic the validation plan computed, keyed by its column name.
    pub stats: BTreeMap<String, serde_json::Value>,
    /// Hash over the bytes of every file, in path order: what a certificate signs.
    pub data_hash: String,
    pub files: Vec<FileEntry>,
}

impl Manifest {
    pub fn load(root: &Path) -> std::io::Result<Option<Manifest>> {
        let p = root.join(MANIFEST_FILE);
        if !p.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(p)?;
        serde_json::from_str(&text)
            .map(Some)
            .map_err(std::io::Error::other)
    }

    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let tmp = root.join(format!("{MANIFEST_FILE}.tmp"));
        std::fs::write(
            &tmp,
            serde_json::to_string_pretty(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(tmp, root.join(MANIFEST_FILE))
    }

    /// True when every file was written under this contract hash, so stored flags can replace live rules.
    pub fn flags_current(&self, contract_hash: &str) -> bool {
        !self.files.is_empty() && self.files.iter().all(|f| f.contract_hash == contract_hash)
    }
}
