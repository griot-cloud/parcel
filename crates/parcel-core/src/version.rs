//! A contract's version: a semantic version, `MAJOR.MINOR.PATCH`, with an optional
//! pre-release (`-beta.1`) and build (`+2026.10`).
//!
//! A whole number is accepted for a contract written before semantic versions: `version: 2`
//! is `2.0.0`. A decimal is refused, because YAML reads `1.10` as the number 1.1.

use std::fmt;
use std::str::FromStr;

/// A semantic version, ordered by precedence.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(semver::Version);

impl Version {
    pub fn new(major: u64, minor: u64, patch: u64) -> Version {
        Version(semver::Version::new(major, minor, patch))
    }

    pub fn major(&self) -> u64 {
        self.0.major
    }

    pub fn minor(&self) -> u64 {
        self.0.minor
    }

    pub fn patch(&self) -> u64 {
        self.0.patch
    }

    /// The pre-release, e.g. `beta.1`; empty for a release.
    pub fn pre(&self) -> &str {
        self.0.pre.as_str()
    }

    /// The version as the `semver` crate represents it.
    pub fn semver(&self) -> &semver::Version {
        &self.0
    }
}

impl From<u32> for Version {
    fn from(major: u32) -> Version {
        Version::new(major.into(), 0, 0)
    }
}

impl Version {
    /// How to write `s` as a version, for a string that is not one.
    pub fn fix_for(s: &str) -> String {
        let t = s.trim();
        if let Some(rest) = t.strip_prefix(['v', 'V'])
            && semver::Version::parse(rest).is_ok()
        {
            return format!("drop the `{}`: `{rest}`", &t[..1]);
        }
        if t.split('.').count() == 2 && t.split('.').all(|p| p.parse::<u64>().is_ok()) {
            return format!("write `{t}.0`");
        }
        "use MAJOR.MINOR.PATCH, e.g. `1.0.0` or `2.1.0-beta.1`".to_owned()
    }
}

impl FromStr for Version {
    type Err = String;

    fn from_str(s: &str) -> Result<Version, String> {
        let t = s.trim();
        if let Ok(major) = t.parse::<u64>() {
            return Ok(Version::new(major, 0, 0));
        }
        semver::Version::parse(t)
            .map(Version)
            .map_err(|_| format!("`{t}` is not a version; {}", Version::fix_for(t)))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl serde::Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Version, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Version;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a version such as 1.0.0")
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Version, E> {
                Ok(Version::new(v, 0, 0))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Version, E> {
                u64::try_from(v)
                    .map(|v| Version::new(v, 0, 0))
                    .map_err(|_| E::custom(format!("`{v}` is not a version")))
            }

            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Version, E> {
                Err(E::custom(format!(
                    "`{v}` is a decimal, not a version; write MAJOR.MINOR.PATCH, e.g. `1.0.0`"
                )))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Version, E> {
                v.parse().map_err(E::custom)
            }
        }
        d.deserialize_any(Visitor)
    }
}

/// The pattern a version string matches, for the JSON Schema (semver 2.0.0).
const PATTERN: &str = r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$";

impl schemars::JsonSchema for Version {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Version".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "A semantic version, MAJOR.MINOR.PATCH, e.g. `1.0.0` or `2.1.0-beta.1`. A whole number N means N.0.0.",
            "oneOf": [
                {"type": "string", "pattern": PATTERN},
                {"type": "integer", "minimum": 0},
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_orders() {
        let v: Version = "1.2.3".parse().unwrap();
        assert_eq!((v.major(), v.minor(), v.patch()), (1, 2, 3));
        assert_eq!("2".parse::<Version>().unwrap(), Version::new(2, 0, 0));
        let beta: Version = "2.1.0-beta.1".parse().unwrap();
        assert_eq!(beta.pre(), "beta.1");
        assert!(beta < "2.1.0".parse().unwrap());
        assert!(Version::new(1, 10, 0) > Version::new(1, 9, 0));
        assert_eq!(beta.to_string(), "2.1.0-beta.1");
    }

    #[test]
    fn refuses_what_is_not_a_version() {
        assert!(
            "1.2"
                .parse::<Version>()
                .unwrap_err()
                .contains("write `1.2.0`")
        );
        assert!(
            "v1.0.0"
                .parse::<Version>()
                .unwrap_err()
                .contains("drop the `v`")
        );
        assert!("01.0.0".parse::<Version>().is_err());
    }

    #[test]
    fn yaml_forms() {
        let v: Version = yaml_serde::from_str("1.0.0").unwrap();
        assert_eq!(v, Version::new(1, 0, 0));
        let v: Version = yaml_serde::from_str("3").unwrap();
        assert_eq!(v, Version::new(3, 0, 0));
        assert!(yaml_serde::from_str::<Version>("1.10").is_err());
        assert_eq!(
            serde_json::to_value(Version::new(1, 0, 0)).unwrap(),
            serde_json::json!("1.0.0")
        );
    }
}
