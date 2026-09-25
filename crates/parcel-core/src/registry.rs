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

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
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
    /// sha256 of the WebAssembly module, for user-defined functions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
}

/// What a tenant submits with a WebAssembly module, one per function (design 7.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FunctionManifest {
    pub name: String,
    pub version: u32,
    /// Typed overloads such as `(string) -> bool`. v1 modules export one overload per name.
    pub signatures: Vec<String>,
    #[serde(default = "yes")]
    pub deterministic: bool,
    #[serde(default = "moderate")]
    pub cost: Cost,
}

fn yes() -> bool {
    true
}

fn moderate() -> Cost {
    Cost::Moderate
}

/// Parse `(string, int) -> bool`. User functions take and return scalars.
pub fn parse_signature(s: &str) -> Result<Signature, String> {
    let bad = || format!("expected a signature like `(string, int) -> bool`, got `{s}`");
    let (args, ret) = s.split_once("->").ok_or_else(bad)?;
    let args = args
        .trim()
        .strip_prefix('(')
        .and_then(|a| a.strip_suffix(')'))
        .ok_or_else(bad)?;
    let scalar = |t: &str| -> Result<Type, String> {
        Ok(match t.trim() {
            "bool" => Type::Bool,
            "int" => Type::Int,
            "uint" => Type::Uint,
            "double" => Type::Double,
            "string" => Type::String,
            "bytes" => Type::Bytes,
            other => {
                return Err(format!(
                    "`{other}` is not a user function type; use bool, int, uint, double, string or bytes"
                ));
            }
        })
    };
    let args = if args.trim().is_empty() {
        Vec::new()
    } else {
        args.split(',').map(scalar).collect::<Result<_, _>>()?
    };
    Ok(Signature {
        args,
        ret: scalar(ret)?,
    })
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
            module: None,
        }
    }

    /// A tenant's function, implemented by a WebAssembly module with this sha256.
    pub fn user(
        manifest: &FunctionManifest,
        owner: &str,
        module_sha256: &str,
    ) -> Result<FunctionEntry, String> {
        if manifest.signatures.len() != 1 {
            return Err("a v1 function module exports exactly one signature per function".into());
        }
        if !manifest
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(format!("`{}` is not a valid function name", manifest.name));
        }
        let signatures = manifest
            .signatures
            .iter()
            .map(|s| parse_signature(s))
            .collect::<Result<Vec<_>, _>>()?;
        let identity = json!({
            "impl": format!("wasm:{module_sha256}"),
            "owner": owner,
            "manifest": manifest,
        });
        Ok(FunctionEntry {
            name: manifest.name.clone(),
            version: manifest.version,
            signatures,
            deterministic: manifest.deterministic,
            nulls: Nulls::Propagate,
            cost: manifest.cost,
            pushdown: Pushdown::None,
            owner: owner.to_owned(),
            hash: hash::sha256_hex(hash::canonical_json(&identity).as_bytes()),
            module: Some(module_sha256.to_owned()),
        })
    }

    pub fn is_builtin(&self) -> bool {
        self.module.is_none()
    }
}

/// How a pinned user function is executed: registered by the runtime that loaded it.
pub type Implementation = std::sync::Arc<
    dyn Fn(
            &[datafusion_common::arrow::array::ArrayRef],
            usize,
        ) -> Result<datafusion_common::arrow::array::ArrayRef, String>
        + Send
        + Sync,
>;

fn implementations() -> &'static std::sync::RwLock<std::collections::HashMap<String, Implementation>>
{
    static IMPLS: std::sync::OnceLock<
        std::sync::RwLock<std::collections::HashMap<String, Implementation>>,
    > = std::sync::OnceLock::new();
    IMPLS.get_or_init(Default::default)
}

/// Make an implementation available to plans pinned to `hash`.
pub fn provide_implementation(hash: &str, imp: Implementation) {
    implementations()
        .write()
        .expect("registry lock")
        .insert(hash.to_owned(), imp);
}

pub fn implementation(hash: &str) -> Option<Implementation> {
    implementations()
        .read()
        .expect("registry lock")
        .get(hash)
        .cloned()
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
            FunctionEntry::builtin("partial", 1, vec![sig(&[String, Int], String)], Cost::Cheap),
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

    /// The built-ins plus every function `owner` registered: what one contract may call.
    pub fn visible_to(&self, owner: Option<&str>) -> Registry {
        Registry {
            entries: self
                .entries
                .iter()
                .filter(|(_, e)| e.owner == "core" || Some(e.owner.as_str()) == owner)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    pub fn entries(&self) -> impl Iterator<Item = &FunctionEntry> {
        self.entries.values()
    }
}
