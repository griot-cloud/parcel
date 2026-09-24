//! parcel-runtime: what an engine needs to execute parcel artifacts.
//!
//! - [`Caller`]: the authenticated caller, which becomes the `ctx` namespace.
//! - [`reference`]: the reference CEL interpreter. It evaluates `decide`,
//!   `guarantee` and `unless` rules and `ctx` parameters at plan time, and it
//!   is the oracle the differential test compares DataFusion against.
//! - [`verify_pins`]: confirms an artifact's pinned functions are the ones this
//!   runtime implements, byte for byte by hash.
//!
//! It contains no compiler.

pub mod reference;

use chrono::{DateTime, Utc};
use parcel_core::registry::{FunctionPin, Registry};
use serde::{Deserialize, Serialize};

/// The caller of a query, as the embedding application authenticated them (design 4.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Caller {
    pub id: String,
    pub tenant: String,
    pub purpose: String,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub clearance: i64,
    #[serde(default)]
    pub roles: Vec<String>,
    /// Fixed once per query. Defaults to the time the caller is created.
    #[serde(default = "Utc::now")]
    pub now: DateTime<Utc>,
    /// Anything else the embedding application knows about the caller: `ctx.other.<field>`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub other: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Caller {
    pub fn new(id: &str, tenant: &str, purpose: &str) -> Caller {
        Caller {
            id: id.into(),
            tenant: tenant.into(),
            purpose: purpose.into(),
            tier: String::new(),
            clearance: 0,
            roles: Vec::new(),
            now: Utc::now(),
            other: Default::default(),
        }
    }

    pub fn with_roles(mut self, roles: &[&str]) -> Caller {
        self.roles = roles.iter().map(|r| r.to_string()).collect();
        self
    }

    pub fn at(mut self, now: DateTime<Utc>) -> Caller {
        self.now = now;
        self
    }
}

/// Every function an artifact is pinned to must be implemented here, with the same hash.
pub fn verify_pins<'a>(pins: impl IntoIterator<Item = &'a FunctionPin>) -> Result<(), String> {
    let builtin = Registry::builtin();
    for pin in pins {
        match builtin.get(&pin.name) {
            Some(e) if e.hash == pin.hash => {}
            Some(_) => {
                return Err(format!(
                    "`{}` v{} is pinned to hash {}, which this runtime does not implement",
                    pin.name, pin.version, pin.hash
                ));
            }
            None => return Err(format!("`{}` is not available in this runtime", pin.name)),
        }
    }
    Ok(())
}
