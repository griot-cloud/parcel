//! The dataset manifest, written next to the data on every write (design 10, stage 6).

use std::collections::BTreeMap;
use std::path::Path;

use std::str::FromStr;

use chrono::{DateTime, Utc};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use serde::{Deserialize, Serialize};

/// Manifests live in this directory under the binding root, one per contract bound to the data.
pub const MANIFEST_DIR: &str = "_parcel";

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
    /// The row schema the contract was compiled against, so a restarted engine can recompile.
    #[serde(default)]
    pub row_schema: Vec<ColumnDef>,
    pub files: Vec<FileEntry>,
}

/// One column of a persisted Arrow schema. `type` is Arrow's display form, which parses back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnDef {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
    pub nullable: bool,
}

pub fn schema_to_defs(schema: &Schema) -> Vec<ColumnDef> {
    schema
        .fields()
        .iter()
        .map(|f| ColumnDef {
            name: f.name().clone(),
            data_type: f.data_type().to_string(),
            nullable: f.is_nullable(),
        })
        .collect()
}

pub fn schema_from_defs(defs: &[ColumnDef]) -> Result<Schema, String> {
    let fields = defs
        .iter()
        .map(|d| {
            let t = DataType::from_str(&d.data_type)
                .map_err(|e| format!("column `{}`: {e}", d.name))?;
            Ok(Field::new(&d.name, t, d.nullable))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Schema::new(fields))
}

impl Manifest {
    fn path(root: &Path, contract: &str) -> std::path::PathBuf {
        root.join(MANIFEST_DIR)
            .join(format!("{}.json", contract.replace('/', "__")))
    }

    /// The manifest one contract keeps for the data it binds.
    pub fn load(root: &Path, contract: &str) -> std::io::Result<Option<Manifest>> {
        let p = Self::path(root, contract);
        if !p.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(p)?;
        serde_json::from_str(&text)
            .map(Some)
            .map_err(std::io::Error::other)
    }

    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let p = Self::path(root, &self.contract);
        std::fs::create_dir_all(p.parent().expect("manifest dir"))?;
        let tmp = p.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_string_pretty(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(tmp, p)
    }

    /// The row schema recorded by any contract's manifest over this data.
    pub fn any_schema(root: &Path) -> std::io::Result<Option<Vec<ColumnDef>>> {
        let dir = root.join(MANIFEST_DIR);
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return Ok(None);
        };
        let mut paths: Vec<_> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        paths.sort();
        for p in paths {
            if let Ok(m) = serde_json::from_str::<Manifest>(&std::fs::read_to_string(&p)?)
                && !m.row_schema.is_empty()
            {
                return Ok(Some(m.row_schema));
            }
        }
        Ok(None)
    }

    /// True when every file was written under this contract hash, so stored flags can replace live rules.
    pub fn flags_current(&self, contract_hash: &str) -> bool {
        !self.files.is_empty() && self.files.iter().all(|f| f.contract_hash == contract_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_round_trip() {
        let schema = Schema::new(vec![
            Field::new("a", DataType::Int32, false),
            Field::new(
                "b",
                DataType::List(Field::new_list_field(DataType::Utf8, true).into()),
                true,
            ),
            Field::new("c", DataType::Decimal128(18, 2), true),
            Field::new(
                "d",
                DataType::Timestamp(
                    datafusion::arrow::datatypes::TimeUnit::Microsecond,
                    Some("UTC".into()),
                ),
                true,
            ),
            Field::new("e", DataType::Date32, false),
        ]);
        assert_eq!(schema_from_defs(&schema_to_defs(&schema)).unwrap(), schema);
    }
}
