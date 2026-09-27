use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Placement terms, independent of the six rule operations. Missing terms deny placement.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Residency {
    pub jurisdictions: BTreeSet<String>,
    /// Permission to release approved output outside the processing jurisdictions.
    #[serde(default)]
    pub approved_output_may_leave: bool,
}

impl Residency {
    pub fn permits(&self, jurisdiction: &str) -> bool {
        known_jurisdiction(jurisdiction) && self.jurisdictions.contains(jurisdiction)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.jurisdictions.is_empty() {
            return Err("residency jurisdictions must not be empty".into());
        }
        for code in &self.jurisdictions {
            if !known_jurisdiction(code) {
                return Err(format!("unknown residency jurisdiction `{code}`"));
            }
        }
        Ok(())
    }

    pub(crate) fn narrows(&self, parent: &Self) -> bool {
        self.jurisdictions.is_subset(&parent.jurisdictions)
            && (!self.approved_output_may_leave || parent.approved_output_may_leave)
    }
}

fn known_jurisdiction(code: &str) -> bool {
    JURISDICTIONS.contains(&code)
}

// ISO 3166-1 alpha-2 country codes and EU, the European Union residency zone.
// Wildcards are not contract terms: a contract must name its permitted jurisdictions.
const JURISDICTIONS: &[&str] = &[
    "AD", "AE", "AF", "AG", "AI", "AL", "AM", "AO", "AQ", "AR", "AS", "AT", "AU", "AW", "AX", "AZ",
    "BA", "BB", "BD", "BE", "BF", "BG", "BH", "BI", "BJ", "BL", "BM", "BN", "BO", "BQ", "BR", "BS",
    "BT", "BV", "BW", "BY", "BZ", "CA", "CC", "CD", "CF", "CG", "CH", "CI", "CK", "CL", "CM", "CN",
    "CO", "CR", "CU", "CV", "CW", "CX", "CY", "CZ", "DE", "DJ", "DK", "DM", "DO", "DZ", "EC", "EE",
    "EG", "EH", "ER", "ES", "ET", "EU", "FI", "FJ", "FK", "FM", "FO", "FR", "GA", "GB", "GD", "GE",
    "GF", "GG", "GH", "GI", "GL", "GM", "GN", "GP", "GQ", "GR", "GS", "GT", "GU", "GW", "GY", "HK",
    "HM", "HN", "HR", "HT", "HU", "ID", "IE", "IL", "IM", "IN", "IO", "IQ", "IR", "IS", "IT", "JE",
    "JM", "JO", "JP", "KE", "KG", "KH", "KI", "KM", "KN", "KP", "KR", "KW", "KY", "KZ", "LA", "LB",
    "LC", "LI", "LK", "LR", "LS", "LT", "LU", "LV", "LY", "MA", "MC", "MD", "ME", "MF", "MG", "MH",
    "MK", "ML", "MM", "MN", "MO", "MP", "MQ", "MR", "MS", "MT", "MU", "MV", "MW", "MX", "MY", "MZ",
    "NA", "NC", "NE", "NF", "NG", "NI", "NL", "NO", "NP", "NR", "NU", "NZ", "OM", "PA", "PE", "PF",
    "PG", "PH", "PK", "PL", "PM", "PN", "PR", "PS", "PT", "PW", "PY", "QA", "RE", "RO", "RS", "RU",
    "RW", "SA", "SB", "SC", "SD", "SE", "SG", "SH", "SI", "SJ", "SK", "SL", "SM", "SN", "SO", "SR",
    "SS", "ST", "SV", "SX", "SY", "SZ", "TC", "TD", "TF", "TG", "TH", "TJ", "TK", "TL", "TM", "TN",
    "TO", "TR", "TT", "TV", "TW", "TZ", "UA", "UG", "UM", "US", "UY", "UZ", "VA", "VC", "VE", "VG",
    "VI", "VN", "VU", "WF", "WS", "YE", "YT", "ZA", "ZM", "ZW",
];
