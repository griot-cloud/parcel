//! Import an Open Data Contract Standard (ODCS v3) document as a parcel contract.
//!
//! ODCS describes what data should be; parcel makes it enforceable. The importer maps what has
//! a faithful parcel meaning and reports everything else as a note rather than guessing:
//!
//! | ODCS | parcel |
//! |---|---|
//! | `schema[].properties[]` (`name`, `logicalType`, `physicalType`) | `expose` |
//! | `required: true` or `primaryKey: true` | `assert has(row.x)`, `on_fail: deny` |
//! | `unique: true` | `guarantee` distinct count equals non-null count, `annotate` |
//! | `quality` with `engine: parcel` | its `implementation` as parcel rules, verbatim |
//! | `name`, `domain`, `version` | `contract` (`domain/name`), `version` (major) |
//!
//! Everything else in the document stays authoritative in ODCS (descriptions, owners,
//! SLAs, servers); the import notes say what was not carried over.

use serde_json::Value;

use crate::document::{
    AssertOnFail, AssertRule, Binding, ContractDoc, ExposeColumn, GuaranteeOnFail, GuaranteeRule,
    Rule,
};

/// The result of an import: a contract to review, and what did not carry over.
#[derive(Clone, Debug, PartialEq)]
pub struct Imported {
    pub doc: ContractDoc,
    pub notes: Vec<String>,
}

/// Import an ODCS v3 document (YAML or JSON). `object` picks a schema object when there are several.
pub fn import(source: &str, object: Option<&str>) -> Result<Imported, String> {
    let v: Value = yaml_serde::from_str(source).map_err(|e| format!("not YAML/JSON: {e}"))?;
    let mut notes = Vec::new();
    match v.get("kind").and_then(Value::as_str) {
        Some("DataContract") => {}
        other => notes.push(format!(
            "`kind` is {other:?}, expected \"DataContract\"; importing anyway"
        )),
    }
    if let Some(api) = v.get("apiVersion").and_then(Value::as_str)
        && !api.starts_with("v3")
    {
        notes.push(format!("apiVersion {api}: the importer targets ODCS v3"));
    }

    let objects = v
        .get("schema")
        .and_then(Value::as_array)
        .ok_or("the document has no `schema` list")?;
    let obj = match object {
        Some(name) => objects
            .iter()
            .find(|o| o.get("name").and_then(Value::as_str) == Some(name))
            .ok_or_else(|| format!("no schema object named `{name}`"))?,
        None => {
            if objects.len() > 1 {
                let names: Vec<&str> = objects
                    .iter()
                    .filter_map(|o| o.get("name").and_then(Value::as_str))
                    .collect();
                notes.push(format!(
                    "{} schema objects ({}); imported the first, choose another with --object",
                    objects.len(),
                    names.join(", ")
                ));
            }
            objects.first().ok_or("`schema` is empty")?
        }
    };
    let obj_name = obj.get("name").and_then(Value::as_str).unwrap_or("data");

    let name = {
        let base = v.get("name").and_then(Value::as_str).unwrap_or(obj_name);
        match v.get("domain").and_then(Value::as_str) {
            Some(d) => format!("{}/{}", slug(d), slug(base)),
            None => slug(base),
        }
    };
    let version = match v.get("version") {
        Some(Value::String(s)) => s
            .split('.')
            .next()
            .and_then(|m| m.parse().ok())
            .unwrap_or(1),
        Some(Value::Number(n)) => n.as_u64().unwrap_or(1) as u32,
        _ => 1,
    };

    let mut expose = Vec::new();
    let mut rules = Vec::new();
    for p in obj
        .get("properties")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(col) = p.get("name").and_then(Value::as_str) else {
            continue;
        };
        match column_type(p) {
            Ok(t) => expose.push(ExposeColumn {
                name: col.to_owned(),
                type_name: t,
            }),
            Err(why) => {
                notes.push(format!("`{col}`: {why}; not exposed"));
                continue;
            }
        }
        let flag = |k: &str| p.get(k).and_then(Value::as_bool).unwrap_or(false);
        let id = slug(col);
        if flag("required") || flag("primaryKey") {
            rules.push(Rule::Assert(AssertRule {
                id: format!("{id}_present"),
                expr: format!("has(row.{col})"),
                on_fail: AssertOnFail::Deny,
            }));
        }
        if flag("unique") || flag("primaryKey") {
            rules.push(Rule::Guarantee(GuaranteeRule {
                id: format!("{id}_unique"),
                expr: format!(
                    "dataset.{col}.distinct_count == dataset.row_count - dataset.{col}.null_count"
                ),
                on_fail: GuaranteeOnFail::Annotate,
            }));
        }
        for q in p
            .get("quality")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            import_quality(q, &format!("`{col}`"), &mut rules, &mut notes);
        }
        for key in [
            "classification",
            "logicalTypeOptions",
            "transformLogic",
            "examples",
        ] {
            if p.get(key).is_some() {
                notes.push(format!("`{col}.{key}` is kept in ODCS only"));
            }
        }
    }
    for q in obj
        .get("quality")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        import_quality(q, &format!("`{obj_name}`"), &mut rules, &mut notes);
    }
    for q in v
        .get("quality")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        import_quality(q, "the contract", &mut rules, &mut notes);
    }
    if v.get("servers").is_some() {
        notes.push("`servers` are not imported; set the binding to where the data lives".into());
    }
    for key in ["slaProperties", "support", "team", "roles", "price"] {
        if v.get(key).is_some() {
            notes.push(format!("`{key}` is kept in ODCS only"));
        }
    }

    let doc = ContractDoc {
        contract: name,
        version,
        owner: v.get("tenant").and_then(Value::as_str).map(slug),
        inherits: None,
        binding: Some(Binding {
            parquet: format!("data/{}/", slug(obj_name)),
            partitioned_by: Vec::new(),
        }),
        expose: Some(expose),
        rules,
        extensions: None,
        enrich: None,
        dataset_other: None,
    };
    Ok(Imported { doc, notes })
}

/// A quality entry: parcel-engine rules come across verbatim; anything else is noted.
fn import_quality(q: &Value, whose: &str, rules: &mut Vec<Rule>, notes: &mut Vec<String>) {
    let engine = q.get("engine").and_then(Value::as_str);
    let label = q
        .get("name")
        .or_else(|| q.get("rule"))
        .or_else(|| q.get("metric"))
        .and_then(Value::as_str)
        .unwrap_or("unnamed");
    if engine == Some("parcel") {
        let imp = match q.get("implementation") {
            Some(Value::String(s)) => yaml_serde::from_str::<Value>(s).ok(),
            Some(other) => Some(other.clone()),
            None => None,
        };
        let list = match imp {
            Some(Value::Array(a)) => a,
            Some(one @ Value::Object(_)) => vec![one],
            _ => {
                notes.push(format!("{whose} quality `{label}` names engine parcel but has no rules in `implementation`"));
                return;
            }
        };
        for r in list {
            match serde_json::from_value::<Rule>(r) {
                Ok(rule) => rules.push(rule),
                Err(e) => notes.push(format!("{whose} quality `{label}`: not a parcel rule: {e}")),
            }
        }
    } else {
        let kind = q.get("type").and_then(Value::as_str).unwrap_or("library");
        notes.push(format!("{whose} quality `{label}` ({kind}) is not imported; express it as a parcel rule or run it in its own engine"));
    }
}

fn column_type(p: &Value) -> Result<String, String> {
    let physical = p
        .get("physicalType")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_lowercase());
    if let Some(ph) = &physical {
        if let Some(args) = ph
            .strip_prefix("decimal")
            .or_else(|| ph.strip_prefix("numeric"))
        {
            let args = args.trim();
            if args.starts_with('(') {
                return Ok(format!("decimal{}", args.replace(' ', "")));
            }
        }
        let mapped = match ph.split('(').next().unwrap_or("") {
            "varchar" | "text" | "string" | "char" | "nvarchar" | "uuid" => Some("utf8"),
            "bigint" | "int8" | "long" => Some("int64"),
            "int" | "integer" | "int4" => Some("int32"),
            "smallint" | "int2" => Some("int16"),
            "double" | "float8" | "double precision" => Some("float64"),
            "real" | "float" | "float4" => Some("float32"),
            "boolean" | "bool" => Some("bool"),
            "date" => Some("date32"),
            "timestamp" | "timestamptz" | "timestamp_ntz" | "timestamp_ltz" | "datetime" => {
                Some("timestamp")
            }
            "bytea" | "binary" | "varbinary" | "blob" => Some("binary"),
            _ => None,
        };
        if let Some(m) = mapped {
            return Ok(m.to_owned());
        }
    }
    let logical = p
        .get("logicalType")
        .and_then(Value::as_str)
        .unwrap_or("string");
    Ok(match logical {
        "string" => "utf8".into(),
        "integer" => "int64".into(),
        "number" => "float64".into(),
        "boolean" => "bool".into(),
        "date" => "date32".into(),
        "timestamp" => "timestamp".into(),
        "array" => {
            let items = p
                .get("items")
                .map(column_type)
                .transpose()?
                .unwrap_or_else(|| "utf8".into());
            format!("list<{items}>")
        }
        other => return Err(format!("logicalType `{other}` has no flat parcel type")),
    })
}

fn slug(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Seller Payments v1"), "seller_payments_v1");
        assert_eq!(slug("txn-ref.dt"), "txn_ref_dt");
    }
}
