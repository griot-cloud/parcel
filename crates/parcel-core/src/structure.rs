//! The structure pass: a contract document's shape, before it is deserialised.
//!
//! Deserialising stops at the first problem and describes it in serde's terms. This pass walks
//! the YAML as plain data instead and reports every structural problem at once, each with the
//! line it is on and what to write instead. Whatever it does not cover, deserialising still
//! catches.

use serde_json::Value;

use crate::diag::{Code, Diagnostic, did_you_mean};

const TOP_KEYS: [&str; 11] = [
    "contract",
    "version",
    "owner",
    "inherits",
    "binding",
    "residency",
    "expose",
    "rules",
    "extensions",
    "enrich",
    "dataset_other",
];

const OPS: [&str; 6] = [
    "decide",
    "admit",
    "assert",
    "transform",
    "guarantee",
    "shape",
];

/// The fields each operation takes beyond `id` and `op`, and which of them are required.
fn op_fields(op: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match op {
        "decide" | "admit" => (&["expr"], &["expr"]),
        "assert" | "guarantee" => (&["expr", "on_fail"], &["expr", "on_fail"]),
        "transform" => (&["column", "expr"], &["column", "expr"]),
        _ => (&["operator", "column", "params", "unless"], &["operator"]),
    }
}

fn on_fail_help(op: &str) -> &'static str {
    if op == "assert" {
        "on_fail: drop (exclude row) | deny (invalidate dataset) | report (count only)"
    } else {
        "on_fail: deny (invalidate dataset) | annotate (record only)"
    }
}

/// Every structural problem in `source`, or the document as plain data when there are none.
pub fn check(source: &str) -> Result<Value, Vec<Diagnostic>> {
    let value: Value = match yaml_serde::from_str(source) {
        Ok(v) => v,
        Err(e) => return Err(vec![yaml_error(source, &e)]),
    };
    let lines = Lines::new(source);
    let mut d = Vec::new();
    let Some(top) = value.as_object() else {
        return Err(vec![
            Diagnostic::new(
                Code::Document,
                None,
                "expected a mapping: contract, version, binding, expose, rules",
            )
            .with_help("see `parcel schema` for the fields"),
        ]);
    };

    for key in top.keys() {
        if !TOP_KEYS.contains(&key.as_str()) {
            d.push(unknown_key(
                key,
                &TOP_KEYS,
                "contract",
                None,
                lines.top(key),
            ));
        }
    }
    match top.get("contract") {
        None => d.push(
            Diagnostic::new(Code::Document, None, "missing `contract`")
                .with_help("add `contract: <area>/<dataset>`")
                .at(lines.first()),
        ),
        Some(Value::String(_)) => {}
        Some(_) => d.push(
            Diagnostic::new(Code::Document, None, "`contract` must be text")
                .with_help("quote it: `contract: \"purchasing/orders\"`")
                .at(lines.top("contract")),
        ),
    }
    match top.get("version") {
        None => d.push(
            Diagnostic::new(Code::Document, None, "missing `version`")
                .with_help("add `version: 1`")
                .at(lines.first()),
        ),
        Some(v) if v.as_u64().is_some_and(|n| n <= u32::MAX as u64) => {}
        Some(v) => d.push(
            Diagnostic::new(
                Code::Document,
                None,
                format!("`version` must be a whole number, not {v}"),
            )
            .with_help("e.g. `version: 2`")
            .at(lines.top("version")),
        ),
    }
    for key in ["owner", "inherits"] {
        if let Some(v) = top.get(key)
            && !v.is_string()
        {
            d.push(
                Diagnostic::new(Code::Document, None, format!("`{key}` must be text"))
                    .with_help(format!("quote it: `{key}: \"...\"`"))
                    .at(lines.top(key)),
            );
        }
    }
    if let Some(b) = top.get("binding") {
        binding(b, &lines, &mut d);
    }
    if let Some(Value::Null) = top.get("expose") {
        d.push(
            Diagnostic::new(Code::Document, None, "`expose:` is empty")
                .with_help("add items under `expose:`, or remove it")
                .at(lines.top("expose")),
        );
    } else if let Some(e) = top.get("expose") {
        expose(e, &lines, &mut d);
    }
    if let Some(Value::Null) = top.get("rules") {
        d.push(
            Diagnostic::new(Code::Document, None, "`rules:` is empty")
                .with_help("add items under `rules:`, or remove it")
                .at(lines.top("rules")),
        );
    } else if let Some(r) = top.get("rules") {
        rules(r, &lines, &mut d);
    }
    if d.is_empty() {
        Ok(value)
    } else {
        Err(collapse_colons(d))
    }
}

/// One missing space after a colon is usually many: report the first, and list the rest.
fn collapse_colons(d: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let is_colon = |x: &Diagnostic| {
        x.help
            .as_deref()
            .is_some_and(|h| h.starts_with("add a space after the colon"))
    };
    let more: Vec<String> = d
        .iter()
        .filter(|x| is_colon(x))
        .skip(1)
        .map(|x| x.line.map(|l| l.to_string()).unwrap_or_else(|| "?".into()))
        .collect();
    let mut first = true;
    d.into_iter()
        .filter_map(|mut x| {
            if !is_colon(&x) {
                return Some(x);
            }
            if !std::mem::take(&mut first) {
                return None;
            }
            if !more.is_empty() {
                let help = x.help.take().unwrap_or_default();
                x.help = Some(format!(
                    "{help}; also line{} {}",
                    if more.len() == 1 { "" } else { "s" },
                    more.join(", ")
                ));
            }
            Some(x)
        })
        .collect()
}

fn binding(b: &Value, lines: &Lines, d: &mut Vec<Diagnostic>) {
    let at = lines.top("binding");
    let Some(m) = b.as_object() else {
        d.push(
            Diagnostic::new(Code::Document, None, "`binding` must be a mapping")
                .with_help(
                    "`binding: {parquet: <path>}` or `binding: {iceberg: <namespace.table>}`",
                )
                .at(at),
        );
        return;
    };
    let keys = ["parquet", "iceberg", "partitioned_by"];
    for k in m.keys() {
        if !keys.contains(&k.as_str()) {
            d.push(unknown_key(k, &keys, "binding", None, at));
        }
    }
    if !m.contains_key("parquet") && !m.contains_key("iceberg") {
        d.push(
            Diagnostic::new(
                Code::Document,
                None,
                "a binding names exactly one of `parquet` or `iceberg`; this one names neither",
            )
            .with_help("add `parquet: <file or directory>` or `iceberg: <namespace.table>`")
            .at(at),
        );
    }
}

fn expose(e: &Value, lines: &Lines, d: &mut Vec<Diagnostic>) {
    let Some(items) = e.as_array() else {
        d.push(
            Diagnostic::new(Code::Document, None, "`expose` must be a list")
                .with_help("one `- {name: <column>, type: <type>}` per column")
                .at(lines.top("expose")),
        );
        return;
    };
    let starts = lines.items("expose");
    for (i, item) in items.iter().enumerate() {
        let at = starts.get(i).copied();
        let what = format!("expose[{i}]");
        let Some(m) = item.as_object() else {
            let name = item.as_str().unwrap_or("<column>");
            d.push(
                Diagnostic::new(
                    Code::Document,
                    None,
                    format!("{what}: expected `{{name: ..., type: ...}}`"),
                )
                .with_help(format!("`- {{name: {name}, type: <type>}}`"))
                .at(at),
            );
            continue;
        };
        let mut bad_key = false;
        for k in m.keys() {
            if k != "name" && k != "type" {
                bad_key = true;
                d.push(unknown_key(k, &["name", "type"], &what, None, at));
            }
        }
        if bad_key {
            continue; // the missing-field messages would only repeat the cause
        }
        for k in ["name", "type"] {
            match m.get(k) {
                None => d.push(
                    Diagnostic::new(Code::Document, None, format!("{what} has no `{k}`"))
                        .with_help("each exposed column is `{name: <column>, type: <type>}`")
                        .at(at),
                ),
                Some(Value::String(_)) => {}
                Some(v) => d.push(
                    Diagnostic::new(
                        Code::Document,
                        None,
                        format!("{what}: `{k}` must be text, not {v}"),
                    )
                    .with_help(format!(
                        "quote it: `{k}: \"{}\"`",
                        v.to_string().trim_matches('"')
                    ))
                    .at(at),
                ),
            }
        }
    }
}

fn rules(r: &Value, lines: &Lines, d: &mut Vec<Diagnostic>) {
    let Some(items) = r.as_array() else {
        d.push(
            Diagnostic::new(Code::Document, None, "`rules` must be a list")
                .with_help("start each rule with `- id: <name>`")
                .at(lines.top("rules")),
        );
        return;
    };
    let starts = lines.items("rules");
    for (i, item) in items.iter().enumerate() {
        let start = starts.get(i).copied();
        let end = starts.get(i + 1).copied();
        let Some(m) = item.as_object() else {
            d.push(
                Diagnostic::new(Code::Document, None, format!("rules[{i}] is not a rule"))
                    .with_help("expected `- id: <name>` with `op:` and `expr:`")
                    .at(start),
            );
            continue;
        };
        let id = m.get("id").and_then(Value::as_str);
        let name = id
            .map(str::to_owned)
            .unwrap_or_else(|| format!("rules[{i}]"));
        let rule = id;
        let field_line = |k: &str| lines.key_between(k, start, end).or(start);
        match m.get("id") {
            None => d.push(
                Diagnostic::new(Code::Document, None, format!("rules[{i}] has no `id`"))
                    .with_help("add `- id: <name>`")
                    .at(start),
            ),
            Some(Value::String(_)) => {}
            Some(v) => d.push(
                Diagnostic::new(
                    Code::Document,
                    None,
                    format!("rules[{i}]: `id` must be text, not {v}"),
                )
                .with_help("lowercase and underscores, e.g. `positive_amount`")
                .at(field_line("id")),
            ),
        }
        let op = match m.get("op") {
            None => {
                d.push(
                    Diagnostic::new(Code::Document, rule, format!("`{name}` has no `op`"))
                        .with_help(format!("add `op:` with one of {}", OPS.join(", ")))
                        .at(start),
                );
                None
            }
            Some(Value::String(op)) if OPS.contains(&op.as_str()) => Some(op.as_str()),
            Some(v) => {
                let text = v.as_str().unwrap_or_default();
                let help = match did_you_mean(text, OPS) {
                    Some(o) => format!("did you mean `op: {o}`?"),
                    None => format!("`op` is one of {}", OPS.join(", ")),
                };
                d.push(
                    Diagnostic::new(Code::Document, rule, format!("`{name}`: unknown op {v}"))
                        .with_help(help)
                        .at(field_line("op")),
                );
                None
            }
        };
        let Some(op) = op else { continue };
        let (allowed, required) = op_fields(op);
        let mut bad_key = false;
        for k in m.keys() {
            if k == "id" || k == "op" || k == "message" || allowed.contains(&k.as_str()) {
                continue;
            }
            bad_key = true;
            let mut all = vec!["id", "op", "message"];
            all.extend_from_slice(allowed);
            let why = match k.as_str() {
                "on_fail" => Some("`on_fail` is for assert and guarantee rules".to_owned()),
                "expr" if op == "shape" => {
                    Some("shapes take `operator`, `params`, `unless`".into())
                }
                "column" => Some(
                    "`column` is for transform and shape rules; read columns as `row.<column>`"
                        .to_owned(),
                ),
                _ => None,
            };
            d.push(
                unknown_key(k, &all, &format!("`{name}` ({op})"), why, field_line(k))
                    .with_rule(rule),
            );
        }
        for k in required {
            if m.contains_key(*k) {
                continue;
            }
            if bad_key && *k == "expr" {
                continue; // most likely spelled wrongly, and already reported
            }
            let help = match *k {
                "on_fail" => on_fail_help(op).to_owned(),
                "expr" => "add `expr:`".to_owned(),
                "column" => "add `column: <exposed column>`".to_owned(),
                _ => "add `operator:` with one of suppress, sample, noise".to_owned(),
            };
            d.push(
                Diagnostic::new(Code::Document, rule, format!("`{name}` ({op}) needs `{k}`"))
                    .with_help(help)
                    .at(start),
            );
        }
        if let Some(v) = m.get("on_fail") {
            let options: &[&str] = if op == "assert" {
                &["drop", "deny", "report"]
            } else {
                &["deny", "annotate"]
            };
            if !v.as_str().is_some_and(|s| options.contains(&s)) {
                d.push(
                    Diagnostic::new(
                        Code::Document,
                        rule,
                        format!(
                            "`{name}`: `on_fail: {}` is not an option for {op} rules",
                            v.as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| v.to_string())
                        ),
                    )
                    .with_help(on_fail_help(op))
                    .at(field_line("on_fail")),
                );
            }
        }
        for k in ["expr", "unless", "column", "operator"] {
            if let Some(v) = m.get(k)
                && !v.is_string()
                && allowed.contains(&k)
            {
                d.push(
                    Diagnostic::new(
                        Code::Document,
                        rule,
                        format!("`{name}`: `{k}` must be text, not {v}"),
                    )
                    .with_help(format!("put it in double quotes: `{k}: \"{v}\"`"))
                    .at(field_line(k)),
                );
            }
        }
    }
}

/// A key the document does not know, with the likeliest fix.
fn unknown_key(
    key: &str,
    known: &[&str],
    within: &str,
    why: Option<String>,
    at: Option<usize>,
) -> Diagnostic {
    let help = if let Some((k, v)) = key.split_once(':') {
        format!("add a space after the colon: `{}: {}`", k.trim(), v.trim())
    } else if let Some(w) = why {
        w
    } else if let Some(k) = did_you_mean(key, known.iter().copied()) {
        format!("did you mean `{k}`?")
    } else {
        format!("{within} takes {}", known.join(", "))
    };
    Diagnostic::new(
        Code::Document,
        None,
        format!("{within}: unknown field `{key}`"),
    )
    .with_help(help)
    .at(at)
}

/// A YAML syntax error, located, with the usual causes.
fn yaml_error(source: &str, e: &yaml_serde::Error) -> Diagnostic {
    let text = e.to_string();
    let message = match text.find(" at line ") {
        Some(i) => text[..i].to_owned(),
        None => text.clone(),
    };
    let line = e.location().map(|l| l.line());
    let on_line = line
        .and_then(|l| source.lines().nth(l.saturating_sub(1)))
        .unwrap_or_default();
    let help = if source.contains('\t') {
        Some("replace tabs with spaces")
    } else if on_line.contains(": !") || text.contains("tag") {
        Some("quote the value: a leading `!` is a YAML tag")
    } else if text.contains("mapping values are not allowed") {
        Some("quote the value: it contains `: `")
    } else if text.contains("did not find expected") || text.contains("could not find expected") {
        Some("check indentation")
    } else {
        None
    };
    let mut d = Diagnostic::new(Code::Document, None, format!("invalid YAML: {message}")).at(line);
    if let Some(h) = help {
        d = d.with_help(h);
    }
    d
}

trait At {
    fn at(self, line: Option<usize>) -> Self;
    fn with_rule(self, rule: Option<&str>) -> Self;
}

impl At for Diagnostic {
    fn at(mut self, line: Option<usize>) -> Self {
        self.line = line;
        self
    }

    fn with_rule(mut self, rule: Option<&str>) -> Self {
        self.rule = rule.map(str::to_owned);
        self
    }
}

/// Where keys and list items are in the source, as 1-based line numbers. Block-style YAML only;
/// in JSON or flow style the lines are unknown and diagnostics go without them.
pub struct Lines<'a> {
    lines: Vec<&'a str>,
}

impl<'a> Lines<'a> {
    pub fn new(source: &'a str) -> Self {
        Lines {
            lines: source.lines().collect(),
        }
    }

    fn first(&self) -> Option<usize> {
        self.lines
            .iter()
            .position(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
            .map(|i| i + 1)
    }

    /// The line of a top-level key.
    pub fn top(&self, key: &str) -> Option<usize> {
        self.lines
            .iter()
            .position(|l| starts_key(l, key))
            .map(|i| i + 1)
    }

    /// The first line of each item of a top-level list.
    pub fn items(&self, key: &str) -> Vec<usize> {
        let Some(start) = self.top(key) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut indent = None;
        for (i, l) in self.lines.iter().enumerate().skip(start) {
            let t = l.trim_start();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let ind = l.len() - t.len();
            if ind == 0 && !t.starts_with('-') {
                break; // the next top-level key
            }
            if t == "-" || t.starts_with("- ") {
                match indent {
                    None => {
                        indent = Some(ind);
                        out.push(i + 1);
                    }
                    Some(n) if n == ind => out.push(i + 1),
                    _ => {}
                }
            }
        }
        out
    }

    /// The first line in `[from, to)` that holds `key`.
    pub fn key_between(&self, key: &str, from: Option<usize>, to: Option<usize>) -> Option<usize> {
        let from = from?;
        let to = to.unwrap_or(self.lines.len() + 1);
        (from..to).find(|&n| {
            let l = self.lines[n - 1];
            let t = l
                .trim_start()
                .trim_start_matches("- ")
                .trim_start_matches('{');
            starts_key(t, key)
                || l.contains(&format!(" {key}:"))
                || l.contains(&format!("{{{key}:"))
        })
    }

    /// The line where a rule with this id starts.
    pub fn rule(&self, id: &str) -> Option<usize> {
        self.items("rules").into_iter().find(|&n| {
            let end = self.items("rules").into_iter().find(|&m| m > n);
            self.key_between("id", Some(n), end).is_some_and(|k| {
                let l = self.lines[k - 1];
                let v = l.split_once("id:").map(|(_, v)| v).unwrap_or_default();
                let v = v.split([',', '}']).next().unwrap_or_default();
                v.trim().trim_matches(['"', '\'']) == id
            })
        })
    }

    /// The `expr:` line of a rule, or its first line when it has none.
    pub fn rule_expr(&self, id: &str) -> Option<usize> {
        let start = self.rule(id)?;
        let end = self.items("rules").into_iter().find(|&m| m > start);
        self.key_between("expr", Some(start), end)
            .or_else(|| self.key_between("unless", Some(start), end))
            .or(Some(start))
    }

    /// The line of the exposed column with this name.
    pub fn exposed(&self, name: &str) -> Option<usize> {
        self.items("expose").into_iter().find(|&n| {
            let l = self.lines[n - 1];
            l.split_once("name:").is_some_and(|(_, v)| {
                v.split([',', '}'])
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .trim_matches(['"', '\''])
                    == name
            }) || self.lines.get(n).is_some_and(|next| {
                next.trim_start()
                    .strip_prefix("name:")
                    .is_some_and(|v| v.trim().trim_matches(['"', '\'']) == name)
            })
        })
    }

    /// The text of a 1-based line.
    pub fn text(&self, line: usize) -> Option<&'a str> {
        self.lines.get(line.checked_sub(1)?).copied()
    }
}

fn starts_key(line: &str, key: &str) -> bool {
    let rest = line
        .strip_prefix(key)
        .or_else(|| line.strip_prefix(&format!("\"{key}\"")));
    rest.is_some_and(|r| r.trim_start().starts_with(':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problems(src: &str) -> Vec<Diagnostic> {
        check(src).expect_err("expected problems")
    }

    const HEAD: &str = "contract: t/x\nversion: 1\nbinding: {parquet: x/}\n";

    #[test]
    fn a_missing_space_after_a_colon_is_named() {
        let src = format!("{HEAD}expose:\n  - {{name:order_id, type: int64}}\n");
        let d = problems(&src);
        assert_eq!(d.len(), 1, "{d:#?}");
        assert_eq!(d[0].line, Some(5));
        assert!(d[0].help.as_deref().unwrap().contains("`name: order_id`"));
    }

    #[test]
    fn every_problem_is_reported_with_its_rule_and_line() {
        let src = format!(
            "{HEAD}expose:\n  - {{name: a, type: int64}}\nrules:\n  - id: a\n    op: admit\n    expr: \"row.a > 0\"\n\n  - id: b\n    op: assert\n    expr: \"row.a > 0\"\n  - id: c\n    op: admitt\n    expr: \"true\"\n  - id: d\n    op: decide\n    expr: \"true\"\n    on_fail: deny\n"
        );
        let d = problems(&src);
        assert_eq!(d.len(), 3, "{d:#?}");
        assert_eq!((d[0].rule.as_deref(), d[0].line), (Some("b"), Some(11)));
        assert!(d[0].help.as_deref().unwrap().contains("drop"));
        assert_eq!((d[1].rule.as_deref(), d[1].line), (Some("c"), Some(15)));
        assert!(d[1].help.as_deref().unwrap().contains("`op: admit`"));
        assert_eq!((d[2].rule.as_deref(), d[2].line), (Some("d"), Some(20)));
    }

    #[test]
    fn locates_rules_and_columns() {
        let src = format!(
            "{HEAD}expose:\n  - {{name: a, type: int64}}\n  - name: b\n    type: utf8\nrules:\n- id: first\n  op: admit\n  expr: \"true\"\n- {{id: second, op: admit, expr: \"true\"}}\n"
        );
        let lines = Lines::new(&src);
        assert_eq!(lines.exposed("a"), Some(5));
        assert_eq!(lines.exposed("b"), Some(6));
        assert_eq!(lines.rule("first"), Some(9));
        assert_eq!(lines.rule("second"), Some(12));
    }

    #[test]
    fn yaml_syntax_errors_say_what_to_quote() {
        let src = format!(
            "{HEAD}rules:\n  - id: a\n    op: transform\n    column: x\n    expr: ctx.tier == 'a' ? row.x : null\n"
        );
        let d = problems(&src);
        assert!(
            d[0].help.as_deref().unwrap().contains("quote the value"),
            "{d:#?}"
        );
    }
}
