//! The function registry as the compiler sees it (design section 7).
//!
//! parcel-core only needs what a function *is*: its signatures and what the
//! compiler may assume about it. Implementations live in `parcel-runtime`,
//! which registers them under the same names and hashes.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::json;

use crate::hash;
use crate::types::Type;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Signature {
    pub args: Vec<Type>,
    pub ret: Type,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Nulls {
    /// Null in any argument gives null out.
    Propagate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Cost {
    Cheap,
    Moderate,
    Expensive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Pushdown {
    None,
    Monotone,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FunctionEntry {
    pub name: String,
    pub version: u32,
    pub signatures: Vec<Signature>,
    pub deterministic: bool,
    pub nulls: Nulls,
    pub cost: Cost,
    pub pushdown: Pushdown,
    pub owner: String,
    /// Hash of implementation identity and manifest. Compiled artifacts pin this.
    pub hash: String,
}

/// A reference from a compiled artifact to the exact registry entry it used.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct FunctionPin {
    pub name: String,
    pub version: u32,
    pub hash: String,
}

impl FunctionEntry {
    pub fn pin(&self) -> FunctionPin {
        FunctionPin {
            name: self.name.clone(),
            version: self.version,
            hash: self.hash.clone(),
        }
    }

    fn builtin(name: &str, version: u32, signatures: Vec<Signature>, cost: Cost) -> Self {
        let manifest = json!({
            "impl": format!("core:{name}@{version}"),
            "name": name,
            "version": version,
            "signatures": signatures,
            "deterministic": true,
            "nulls": Nulls::Propagate,
            "cost": cost,
            "pushdown": Pushdown::None,
        });
        FunctionEntry {
            name: name.into(),
            version,
            signatures,
            deterministic: true,
            nulls: Nulls::Propagate,
            cost,
            pushdown: Pushdown::None,
            owner: "core".into(),
            hash: hash::sha256_hex(hash::canonical_json(&manifest).as_bytes()),
        }
    }
}

fn sig(args: &[Type], ret: Type) -> Signature {
    Signature {
        args: args.to_vec(),
        ret,
    }
}

/// The current entry for every function name a contract may call.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    entries: BTreeMap<String, FunctionEntry>,
}

impl Registry {
    /// The built-in entries shipped with this version of parcel.
    pub fn builtin() -> Self {
        use Type::*;
        let mut r = Registry::default();
        for e in [
            FunctionEntry::builtin(
                "hash_sha256",
                1,
                vec![sig(&[String], String), sig(&[Bytes], String)],
                Cost::Moderate,
            ),
            FunctionEntry::builtin("redact", 1, vec![sig(&[String], String)], Cost::Cheap),
            FunctionEntry::builtin("is_msisdn", 1, vec![sig(&[String], Bool)], Cost::Cheap),
            FunctionEntry::builtin("is_email", 1, vec![sig(&[String], Bool)], Cost::Cheap),
        ] {
            r.insert(e);
        }
        r
    }

    pub fn insert(&mut self, entry: FunctionEntry) {
        self.entries.insert(entry.name.clone(), entry);
    }

    pub fn get(&self, name: &str) -> Option<&FunctionEntry> {
        self.entries.get(name)
    }

    pub fn entries(&self) -> impl Iterator<Item = &FunctionEntry> {
        self.entries.values()
    }
}
