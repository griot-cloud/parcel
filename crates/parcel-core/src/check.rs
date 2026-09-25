//! Contract-level check and classify passes.
//!
//! [`check_contract`] validates the document against the bound schema, checks
//! every rule's expression, and then classifies: confirms each rule's
//! namespaces fit its operation (design 4.8). The result is a
//! [`CheckedContract`], which is what translation and assembly consume.

use std::collections::{BTreeMap, BTreeSet};

use datafusion_common::arrow::datatypes::{DataType, Schema};
use serde::Serialize;
use serde_json::Value;

use crate::checker::{Env, OtherTypes, check_expr};
use crate::diag::{Code, Diagnostic};
use crate::document::*;
use crate::hash;
use crate::ir::{CtxField, ExprKind, NsSet, TExpr, Var};
use crate::registry::{FunctionPin, Registry};
use crate::types::{Type, exposable_as, parse_type_name};

/// A checked expression, with the source it came from.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CheckedExpr {
    pub source: String,
    pub expr: TExpr,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExposedColumn {
    pub name: String,
    /// The type as written in the contract.
    pub type_name: String,
    #[serde(skip)]
    pub data_type: DataType,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "operator")]
pub enum ShapeOp {
    Noise {
        column: String,
        sensitivity: f64,
        epsilon: f64,
        budget: String,
        /// Where the noise goes: on aggregates over the column, or on each value.
        at: NoiseAt,
    },
    Suppress {
        k: u64,
    },
    Sample {
        fraction: f64,
        key: String,
    },
}

/// Where `noise` adds Laplace noise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseAt {
    /// To every aggregate over the column; the column cannot be read outside an aggregate.
    Aggregate,
    /// To each value as it leaves the view, so any query over it sees noised values.
    Row,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "op")]
pub enum CheckedRule {
    Decide {
        id: String,
        expr: CheckedExpr,
    },
    Admit {
        id: String,
        expr: CheckedExpr,
    },
    Assert {
        id: String,
        expr: CheckedExpr,
        on_fail: AssertOnFail,
    },
    Transform {
        id: String,
        column: String,
        expr: CheckedExpr,
    },
    Guarantee {
        id: String,
        expr: CheckedExpr,
        on_fail: GuaranteeOnFail,
    },
    Shape {
        id: String,
        shape: ShapeOp,
        unless: Option<CheckedExpr>,
    },
}

impl CheckedRule {
    pub fn id(&self) -> &str {
        match self {
            CheckedRule::Decide { id, .. }
            | CheckedRule::Admit { id, .. }
            | CheckedRule::Assert { id, .. }
            | CheckedRule::Transform { id, .. }
            | CheckedRule::Guarantee { id, .. }
            | CheckedRule::Shape { id, .. } => id,
        }
    }

    pub fn exprs(&self) -> Vec<&CheckedExpr> {
        match self {
            CheckedRule::Decide { expr, .. }
            | CheckedRule::Admit { expr, .. }
            | CheckedRule::Assert { expr, .. }
            | CheckedRule::Transform { expr, .. }
            | CheckedRule::Guarantee { expr, .. } => vec![expr],
            CheckedRule::Shape { unless, .. } => unless.iter().collect(),
        }
    }
}

/// A contract that has passed check and classify.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CheckedContract {
    pub name: String,
    pub version: u32,
    /// Hash of the document's canonical form (design section 12).
    pub contract_hash: String,
    /// The tenant the contract belongs to, after inheritance.
    pub owner: Option<String>,
    pub binding: Binding,
    pub exposed: Vec<ExposedColumn>,
    pub rules: Vec<CheckedRule>,
    /// Every registry entry the contract is pinned to.
    pub functions: BTreeSet<FunctionPin>,
    /// Declared extension fields.
    pub other: OtherTypes,
    /// `row.other` producers, in order, each inlined to read only raw columns.
    pub enrichers: Vec<(String, CheckedExpr)>,
    /// `dataset.other` producers.
    pub producers: Vec<Producer>,
}

/// An aggregate a `dataset.other` producer may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Aggregate {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    CountDistinct,
    Median,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProducerKind {
    Constant(crate::ir::Lit),
    Aggregate {
        func: Aggregate,
        arg: Option<CheckedExpr>,
    },
}

/// Produces one `dataset.other` field at write.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Producer {
    pub field: String,
    pub ty: Type,
    pub kind: ProducerKind,
}

/// The hash of a contract document's canonical form.
pub fn contract_hash(doc: &ContractDoc) -> String {
    let v = serde_json::to_value(doc).expect("contract documents always serialise");
    hash::sha256_hex(hash::canonical_json(&v).as_bytes())
}

/// Run the check and classify passes over a document and the binding's Arrow schema.
pub fn check_contract(
    doc: &ContractDoc,
    schema: &Schema,
    registry: &Registry,
) -> Result<CheckedContract, Vec<Diagnostic>> {
    check_layers(doc, schema, registry, &[])
}

/// One level of an inheritance chain, root first (see [`crate::inherit`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layer {
    pub contract: String,
    /// The rules this contract added.
    pub rules: BTreeSet<String>,
    /// Columns the layer above exposes: all a child's transforms may read. Empty for the root.
    pub visible: BTreeSet<String>,
    /// What this layer exposes, so its transforms can be checked even where a child hides them.
    pub expose: Vec<crate::document::ExposeColumn>,
}

/// Check a document flattened from an inheritance chain. Rules of each layer see the layers
/// above through their transforms, and a transform may only read what the layer above exposes.
pub fn check_layers(
    doc: &ContractDoc,
    schema: &Schema,
    registry: &Registry,
    layers: &[Layer],
) -> Result<CheckedContract, Vec<Diagnostic>> {
    let mut d: Vec<Diagnostic> = Vec::new();
    let layer_of = |id: &str| {
        layers
            .iter()
            .position(|l| l.rules.contains(id))
            .unwrap_or(0)
    };
    if doc.inherits.is_some() {
        return Err(vec![Diagnostic::new(
            Code::Inheritance,
            None,
            format!(
                "`{}` inherits; resolve the chain first (parcel_core::inherit::resolve)",
                doc.contract
            ),
        )]);
    }
    let e = |d: &mut Vec<Diagnostic>, code, rule: Option<&str>, msg: String| {
        d.push(Diagnostic::new(code, rule, msg))
    };

    // Declared extension fields (design 3.1).
    let mut other = OtherTypes::default();
    if let Some(x) = &doc.extensions {
        for (ns, decl, out) in [
            ("row", &x.row, &mut other.row),
            ("ctx", &x.ctx, &mut other.ctx),
            ("dataset", &x.dataset, &mut other.dataset),
        ] {
            for (field, type_name) in decl {
                match parse_type_name(type_name).and_then(|t| Type::from_arrow(&t)) {
                    Ok(t) => {
                        out.insert(field.clone(), t);
                    }
                    Err(msg) => e(
                        &mut d,
                        Code::BadType,
                        None,
                        format!("extensions.{ns}.{field}: {msg}"),
                    ),
                }
            }
        }
    }

    let columns: BTreeMap<String, Result<Type, String>> = schema
        .fields()
        .iter()
        .map(|f| (f.name().clone(), Type::from_arrow(f.data_type())))
        .collect();

    let Some(binding) = &doc.binding else {
        return Err(vec![Diagnostic::new(
            Code::Document,
            None,
            "the contract has no binding; only a contract that inherits may omit it",
        )]);
    };
    let Some(expose) = &doc.expose else {
        return Err(vec![Diagnostic::new(
            Code::Document,
            None,
            "the contract has no expose section; only a contract that inherits may omit it",
        )]);
    };
    for p in &binding.partitioned_by {
        if !columns.contains_key(p) {
            e(
                &mut d,
                Code::UnknownColumn,
                None,
                format!("partition column `{p}` is not in the bound schema"),
            );
        }
    }

    // Rule ids: unique, and usable as column-name suffixes (`_c_<id>`).
    let mut seen = BTreeSet::new();
    for r in &doc.rules {
        let id = r.id();
        if !is_valid_id(id) {
            e(
                &mut d,
                Code::InvalidRuleId,
                Some(id),
                "rule ids must match [a-z][a-z0-9_]*".into(),
            );
        }
        if !seen.insert(id) {
            e(
                &mut d,
                Code::DuplicateRuleId,
                Some(id),
                "rule id is used more than once".into(),
            );
        }
    }

    // Transforms, indexed by column, so expose can tell a raw column from a transformed one.
    let mut transforms: BTreeMap<&str, &TransformRule> = BTreeMap::new();
    for r in &doc.rules {
        // Across inheritance layers a column may be transformed again: the transforms compose.
        if let Rule::Transform(t) = r
            && let Some(prev) = transforms.insert(t.column.as_str(), t)
            && layer_of(&prev.id) == layer_of(&t.id)
        {
            e(
                &mut d,
                Code::DuplicateTransform,
                Some(&t.id),
                format!("column `{}` already has a transform", t.column),
            );
        }
    }

    // Expose: the caller's schema.
    let mut exposed = Vec::new();
    let mut exposed_names = BTreeSet::new();
    for col in expose {
        if !exposed_names.insert(col.name.as_str()) {
            e(
                &mut d,
                Code::DuplicateExpose,
                None,
                format!("column `{}` is exposed twice", col.name),
            );
            continue;
        }
        let declared = match parse_type_name(&col.type_name) {
            Ok(t) => t,
            Err(msg) => {
                e(
                    &mut d,
                    Code::BadType,
                    None,
                    format!("expose `{}`: {msg}", col.name),
                );
                continue;
            }
        };
        match schema.field_with_name(&col.name) {
            Err(_) => e(
                &mut d,
                Code::UnknownColumn,
                None,
                format!("exposed column `{}` is not in the bound schema", col.name),
            ),
            Ok(f)
                if !transforms.contains_key(col.name.as_str())
                    && !exposable_as(f.data_type(), &declared) =>
            {
                e(
                    &mut d,
                    Code::ExposeTypeMismatch,
                    None,
                    format!(
                        "column `{}` is {} in the data but exposed as {}",
                        col.name,
                        f.data_type(),
                        col.type_name
                    ),
                )
            }
            Ok(_) => {}
        }
        exposed.push(ExposedColumn {
            name: col.name.clone(),
            type_name: col.type_name.clone(),
            data_type: declared,
        });
    }

    let assertions: BTreeSet<String> = doc
        .rules
        .iter()
        .filter(|&r| matches!(r, Rule::Assert(_)))
        .map(|r| r.id().to_owned())
        .collect();
    let env = Env {
        columns: &columns,
        assertions: &assertions,
        registry,
        other: &other,
    };
    let enrichers = check_enrichers(doc, schema, &env, &mut d);
    let producers = check_producers(doc, &env, &mut d);

    // Transforms of columns a child hides are still checked against the type the layer exposed.
    let mut all_exposed = exposed.clone();
    for l in layers {
        for c in &l.expose {
            if !all_exposed.iter().any(|e| e.name == c.name)
                && let Ok(t) = parse_type_name(&c.type_name)
            {
                all_exposed.push(ExposedColumn {
                    name: c.name.clone(),
                    type_name: c.type_name.clone(),
                    data_type: t,
                });
            }
        }
    }
    let mut rules: Vec<CheckedRule> = Vec::new();
    // Transforms composed so far, and the snapshot the current layer sees.
    let mut composed: BTreeMap<String, TExpr> = BTreeMap::new();
    let mut seen_by_layer: BTreeMap<String, TExpr> = BTreeMap::new();
    let mut current = 0;
    for r in &doc.rules {
        let layer = layer_of(r.id());
        if layer != current {
            current = layer;
            seen_by_layer = composed.clone();
        }
        let mut checked = match classify_rule(r, &env, &all_exposed) {
            Ok(c) => c,
            Err(mut errs) => {
                d.append(&mut errs);
                continue;
            }
        };
        if layer > 0 {
            match &mut checked {
                CheckedRule::Transform { id, expr, .. } => {
                    let hidden: Vec<String> = expr
                        .expr
                        .row_columns()
                        .into_iter()
                        .filter(|c| !layers[layer].visible.contains(c))
                        .collect();
                    if !hidden.is_empty() {
                        d.push(Diagnostic::new(
                            Code::Inheritance,
                            Some(id),
                            format!(
                                "a child transform may read only what `{}` exposes; `{}` is hidden by the parent",
                                layers[layer - 1].contract,
                                hidden.join("`, `")
                            ),
                        ));
                        continue;
                    }
                    expr.expr = expr.expr.substitute_rows(&seen_by_layer);
                }
                CheckedRule::Admit { expr, .. } => {
                    expr.expr = expr.expr.substitute_rows(&seen_by_layer)
                }
                _ => {}
            }
        }
        if let CheckedRule::Transform { column, expr, .. } = &checked {
            composed.insert(column.clone(), expr.expr.clone());
        }
        rules.push(checked);
    }
    // A composed transform replaces the ones it was composed from.
    let mut last_transform: BTreeMap<String, usize> = BTreeMap::new();
    for (i, r) in rules.iter().enumerate() {
        if let CheckedRule::Transform { column, .. } = r {
            last_transform.insert(column.clone(), i);
        }
    }
    let rules: Vec<CheckedRule> = rules
        .into_iter()
        .enumerate()
        .filter(|(i, r)| match r {
            CheckedRule::Transform { column, .. } => {
                last_transform[column] == *i && exposed.iter().any(|e| &e.name == column)
            }
            _ => true,
        })
        .map(|(_, r)| r)
        .collect();

    if !d.is_empty() {
        return Err(d);
    }
    // Every function any expression calls: rules, enrichers and dataset producers.
    let mut functions: BTreeSet<FunctionPin> = rules
        .iter()
        .flat_map(|r| r.exprs())
        .flat_map(|x| x.expr.pins())
        .collect();
    functions.extend(enrichers.iter().flat_map(|(_, x)| x.expr.pins()));
    for p in &producers {
        if let ProducerKind::Aggregate { arg: Some(a), .. } = &p.kind {
            functions.extend(a.expr.pins());
        }
    }
    Ok(CheckedContract {
        name: doc.contract.clone(),
        version: doc.version,
        contract_hash: contract_hash(doc),
        owner: doc.owner.clone(),
        binding: binding.clone(),
        exposed,
        rules,
        functions,
        other,
        enrichers,
        producers,
    })
}

/// Enrichers: each produces one declared `row.other` field from the row, in order.
fn check_enrichers(
    doc: &ContractDoc,
    schema: &Schema,
    env: &Env,
    d: &mut Vec<Diagnostic>,
) -> Vec<(String, CheckedExpr)> {
    let list = doc.enrich.clone().unwrap_or_default();
    let data_has_other = schema.field_with_name(crate::ir::OTHER_COLUMN).is_ok();
    if data_has_other && !list.is_empty() {
        d.push(Diagnostic::new(
            Code::Document,
            None,
            "the data already carries `_other`; enrichers would replace it",
        ));
        return Vec::new();
    }
    let mut done: BTreeMap<String, TExpr> = BTreeMap::new();
    let mut out = Vec::new();
    for en in &list {
        let rule = format!("enrich.{}", en.field);
        let err = |d: &mut Vec<Diagnostic>, code, msg: String| {
            d.push(Diagnostic::new(code, Some(&rule), msg))
        };
        let Some(declared) = env.other.row.get(&en.field) else {
            err(
                d,
                Code::UnknownField,
                format!("`{}` is not declared under `extensions.row`", en.field),
            );
            continue;
        };
        if done.contains_key(&format!("other.{}", en.field)) {
            err(
                d,
                Code::DuplicateRuleId,
                format!("`{}` is enriched twice", en.field),
            );
            continue;
        }
        let x = match check_expr(env, &rule, &en.expr) {
            Ok(x) => x,
            Err(mut errs) => {
                d.append(&mut errs);
                continue;
            }
        };
        if x.ns.ctx || x.ns.dataset {
            err(
                d,
                Code::Namespace,
                "enrichers compute stored values and may read only row".into(),
            );
            continue;
        }
        let mut early = Vec::new();
        x.walk(&mut |n| {
            if let ExprKind::Var(Var::RowOther(f)) | ExprKind::HasOther(f) = &n.kind
                && !done.contains_key(&format!("other.{f}"))
            {
                early.push(f.clone());
            }
        });
        if !early.is_empty() {
            err(
                d,
                Code::UnknownField,
                format!(
                    "reads `row.other.{}` before it is enriched; order enrichers so producers come first",
                    early.join("`, `row.other.")
                ),
            );
            continue;
        }
        if &x.ty != declared {
            err(
                d,
                Code::TypeMismatch,
                format!("gives {}, but `{}` is declared {declared}", x.ty, en.field),
            );
            continue;
        }
        for pin in x.pins() {
            if env
                .registry
                .get(&pin.name)
                .is_some_and(|f| !f.deterministic)
            {
                err(
                    d,
                    Code::NonDeterministic,
                    format!(
                        "`{}` is not deterministic; stored values must be reproducible",
                        pin.name
                    ),
                );
            }
        }
        let inlined = x.substitute_rows(&done);
        done.insert(format!("other.{}", en.field), inlined.clone());
        out.push((
            en.field.clone(),
            CheckedExpr {
                source: en.expr.clone(),
                expr: inlined,
            },
        ));
    }
    if !data_has_other {
        for field in env.other.row.keys() {
            if !done.contains_key(&format!("other.{field}")) {
                d.push(Diagnostic::new(
                    Code::UnknownField,
                    None,
                    format!("`row.other.{field}` is declared but nothing produces it: add an enricher, or supply an `_other` column"),
                ));
            }
        }
    }
    out
}

/// `dataset.other` producers: a constant `value`, or an aggregate `expr` such as `avg(row.amount)`.
fn check_producers(doc: &ContractDoc, env: &Env, d: &mut Vec<Diagnostic>) -> Vec<Producer> {
    let mut out = Vec::new();
    for p in doc.dataset_other.clone().unwrap_or_default() {
        let rule = format!("dataset_other.{}", p.field);
        let err = |d: &mut Vec<Diagnostic>, code, msg: String| {
            d.push(Diagnostic::new(code, Some(&rule), msg))
        };
        let Some(declared) = env.other.dataset.get(&p.field).cloned() else {
            err(
                d,
                Code::UnknownField,
                format!("`{}` is not declared under `extensions.dataset`", p.field),
            );
            continue;
        };
        let kind = match (&p.value, &p.expr) {
            (Some(v), None) => {
                use crate::ir::Lit;
                let lit = match (&declared, v) {
                    (Type::String, Value::String(s)) => Lit::String(s.clone()),
                    (Type::Bool, Value::Bool(b)) => Lit::Bool(*b),
                    (Type::Int, Value::Number(n)) if n.is_i64() => {
                        Lit::Int(n.as_i64().unwrap_or_default())
                    }
                    (Type::Uint, Value::Number(n)) if n.is_u64() => {
                        Lit::Uint(n.as_u64().unwrap_or_default())
                    }
                    (Type::Double, Value::Number(n)) => Lit::Double(n.as_f64().unwrap_or_default()),
                    _ => {
                        err(
                            d,
                            Code::TypeMismatch,
                            format!("value {v} is not a {declared}"),
                        );
                        continue;
                    }
                };
                ProducerKind::Constant(lit)
            }
            (None, Some(src)) => {
                let src = src.trim();
                let (Some(open), true) = (src.find('('), src.ends_with(')')) else {
                    err(
                        d,
                        Code::OutsideProfile,
                        "expected an aggregate such as `avg(row.amount)`".into(),
                    );
                    continue;
                };
                let func = match &src[..open] {
                    "count" => Aggregate::Count,
                    "sum" => Aggregate::Sum,
                    "avg" => Aggregate::Avg,
                    "min" => Aggregate::Min,
                    "max" => Aggregate::Max,
                    "count_distinct" => Aggregate::CountDistinct,
                    "median" => Aggregate::Median,
                    other => {
                        err(
                            d,
                            Code::UnknownFunction,
                            format!(
                                "`{other}` is not an aggregate; use count, sum, avg, min, max, count_distinct or median"
                            ),
                        );
                        continue;
                    }
                };
                let inner = src[open + 1..src.len() - 1].trim();
                let arg = if func == Aggregate::Count {
                    if !inner.is_empty() {
                        err(
                            d,
                            Code::NoMatchingOverload,
                            "count() takes no argument".into(),
                        );
                        continue;
                    }
                    None
                } else {
                    match check_expr(env, &rule, inner) {
                        Ok(x) if x.ns.ctx || x.ns.dataset => {
                            err(
                                d,
                                Code::Namespace,
                                "aggregates describe data and may read only row".into(),
                            );
                            continue;
                        }
                        Ok(x) => Some(CheckedExpr {
                            source: inner.to_owned(),
                            expr: x,
                        }),
                        Err(mut errs) => {
                            d.append(&mut errs);
                            continue;
                        }
                    }
                };
                let arg_ty = arg.as_ref().map(|a| a.expr.ty.clone());
                let result = match (func, &arg_ty) {
                    (Aggregate::Count | Aggregate::CountDistinct, _) => Some(Type::Int),
                    (Aggregate::Sum, Some(t))
                        if t.is_numeric() || matches!(t, Type::Decimal(_)) =>
                    {
                        Some(t.clone())
                    }
                    (Aggregate::Avg, Some(t))
                        if t.is_numeric() || matches!(t, Type::Decimal(_)) =>
                    {
                        Some(Type::Double)
                    }
                    (Aggregate::Median, Some(t)) if t.is_numeric() => Some(Type::Double),
                    (Aggregate::Min | Aggregate::Max, Some(t)) if t.is_ordered() => Some(t.clone()),
                    _ => None,
                };
                match result {
                    Some(t) if t == declared => ProducerKind::Aggregate { func, arg },
                    Some(t) => {
                        err(
                            d,
                            Code::TypeMismatch,
                            format!(
                                "`{src}` gives {t}, but `{}` is declared {declared}",
                                p.field
                            ),
                        );
                        continue;
                    }
                    None => {
                        err(
                            d,
                            Code::NoMatchingOverload,
                            format!(
                                "`{src}` does not aggregate {}",
                                arg_ty.map(|t| t.to_string()).unwrap_or_default()
                            ),
                        );
                        continue;
                    }
                }
            }
            _ => {
                err(
                    d,
                    Code::Document,
                    "give exactly one of `value` or `expr`".into(),
                );
                continue;
            }
        };
        out.push(Producer {
            field: p.field.clone(),
            ty: declared,
            kind,
        });
    }
    let produced: BTreeSet<&str> = out.iter().map(|p| p.field.as_str()).collect();
    for field in env.other.dataset.keys() {
        if !produced.contains(field.as_str()) {
            d.push(Diagnostic::new(
                Code::UnknownField,
                None,
                format!(
                    "`dataset.other.{field}` is declared but no `dataset_other` entry produces it"
                ),
            ));
        }
    }
    out
}

fn is_valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'))
}

/// Check one rule's expression, then confirm its namespaces and result type fit its operation.
fn classify_rule(
    rule: &Rule,
    env: &Env,
    exposed: &[ExposedColumn],
) -> Result<CheckedRule, Vec<Diagnostic>> {
    let id = rule.id();
    let err = |code, msg: String| vec![Diagnostic::new(code, Some(id), msg)];
    let check = |src: &str| {
        check_expr(env, id, src).map(|expr| CheckedExpr {
            source: src.to_owned(),
            expr,
        })
    };

    // What each operation may read, and the message that explains why not.
    let fits = |x: &CheckedExpr, allowed: NsSet, why: &str| -> Result<(), Vec<Diagnostic>> {
        let ns = x.expr.ns;
        let bad = (ns.row && !allowed.row)
            || (ns.ctx && !allowed.ctx)
            || (ns.dataset && !allowed.dataset);
        if bad {
            Err(err(
                Code::Namespace,
                format!("{why} (`{}` reads {ns})", x.source),
            ))
        } else {
            Ok(())
        }
    };
    let boolean = |x: &CheckedExpr| -> Result<(), Vec<Diagnostic>> {
        if x.expr.ty == Type::Bool {
            Ok(())
        } else {
            Err(err(
                Code::TypeMismatch,
                format!(
                    "{} rules must be bool, `{}` is {}",
                    rule.op_name(),
                    x.source,
                    x.expr.ty
                ),
            ))
        }
    };
    let deterministic = |x: &CheckedExpr| -> Result<(), Vec<Diagnostic>> {
        for pin in x.expr.pins() {
            if env
                .registry
                .get(&pin.name)
                .is_some_and(|f| !f.deterministic)
            {
                return Err(err(
                    Code::NonDeterministic,
                    format!(
                        "`{}` is not deterministic; {} rules must be reproducible",
                        pin.name,
                        rule.op_name()
                    ),
                ));
            }
        }
        Ok(())
    };

    Ok(match rule {
        Rule::Decide(r) => {
            let x = check(&r.expr)?;
            fits(
                &x,
                NsSet::CTX,
                "decide rules are about the caller and may read only ctx",
            )?;
            boolean(&x)?;
            CheckedRule::Decide {
                id: r.id.clone(),
                expr: x,
            }
        }
        Rule::Admit(r) => {
            let x = check(&r.expr)?;
            fits(
                &x,
                NsSet::ROW.union(NsSet::CTX),
                "admit rules filter rows and may read only row and ctx",
            )?;
            boolean(&x)?;
            CheckedRule::Admit {
                id: r.id.clone(),
                expr: x,
            }
        }
        Rule::Assert(r) => {
            let x = check(&r.expr)?;
            if x.expr.ns.ctx {
                return Err(err(
                    Code::Namespace,
                    "assertions describe data, not callers; use admit".into(),
                ));
            }
            fits(&x, NsSet::ROW, "assert rules may read only row")?;
            boolean(&x)?;
            deterministic(&x)?;
            CheckedRule::Assert {
                id: r.id.clone(),
                expr: x,
                on_fail: r.on_fail,
            }
        }
        Rule::Transform(r) => {
            let Some(target) = exposed.iter().find(|c| c.name == r.column) else {
                return Err(err(
                    Code::TransformTarget,
                    format!("transform target `{}` is not an exposed column", r.column),
                ));
            };
            let x = check(&r.expr)?;
            fits(
                &x,
                NsSet::ROW.union(NsSet::CTX),
                "transform rules may read only row and ctx",
            )?;
            let declared = Type::from_arrow(&target.data_type)
                .map_err(|why| err(Code::UnreadableColumn, why))?;
            if x.expr.ty != declared {
                return Err(err(
                    Code::TypeMismatch,
                    format!(
                        "transform of `{}` gives {}, but the column is exposed as {declared}",
                        r.column, x.expr.ty
                    ),
                ));
            }
            CheckedRule::Transform {
                id: r.id.clone(),
                column: r.column.clone(),
                expr: x,
            }
        }
        Rule::Guarantee(r) => {
            let x = check(&r.expr)?;
            if x.expr.ns.row {
                return Err(err(
                    Code::Namespace,
                    "guarantees describe the whole dataset; use assert for per-row checks".into(),
                ));
            }
            // ctx.now is the one caller field a guarantee may read (freshness, design 4.6).
            let mut ctx_other = false;
            x.expr
                .walk(&mut |n| ctx_other |= matches!(n.kind, ExprKind::Var(Var::CtxOther(_))));
            if ctx_other || x.expr.ctx_fields().iter().any(|f| *f != CtxField::Now) {
                return Err(err(
                    Code::Namespace,
                    "guarantee rules may read dataset and ctx.now only".into(),
                ));
            }
            boolean(&x)?;
            deterministic(&x)?;
            CheckedRule::Guarantee {
                id: r.id.clone(),
                expr: x,
                on_fail: r.on_fail,
            }
        }
        Rule::Shape(r) => {
            let unless = match &r.unless {
                Some(src) => {
                    let x = check(src)?;
                    fits(
                        &x,
                        NsSet::CTX,
                        "`unless` is decided per caller and may read only ctx",
                    )?;
                    boolean(&x)?;
                    Some(x)
                }
                None => None,
            };
            let shape = shape_op(r, exposed).map_err(|msg| err(Code::ShapeParams, msg))?;
            CheckedRule::Shape {
                id: r.id.clone(),
                shape,
                unless,
            }
        }
    })
}

fn shape_op(r: &ShapeRule, exposed: &[ExposedColumn]) -> Result<ShapeOp, String> {
    let p = &r.params;
    let allowed: &[&str] = match r.operator.as_str() {
        "noise" => &["sensitivity", "epsilon", "budget", "at"],
        "suppress" => &["k"],
        "sample" => &["fraction", "key"],
        other => {
            return Err(format!(
                "unknown shape operator `{other}`; known: noise, suppress, sample"
            ));
        }
    };
    if let Some(k) = p.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!("`{}` does not take a `{k}` parameter", r.operator));
    }
    let num = |k: &str| -> Result<f64, String> {
        p.get(k)
            .and_then(Value::as_f64)
            .ok_or_else(|| format!("`{}` needs a numeric `{k}`", r.operator))
    };
    let text = |k: &str| -> Result<String, String> {
        p.get(k)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("`{}` needs a string `{k}`", r.operator))
    };
    let exposed_col = |c: &str| -> Result<(), String> {
        if exposed.iter().any(|e| e.name == c) {
            Ok(())
        } else {
            Err(format!("`{c}` is not an exposed column"))
        }
    };
    Ok(match r.operator.as_str() {
        "noise" => {
            let column = r.column.clone().ok_or("`noise` needs a `column`")?;
            exposed_col(&column)?;
            let (sensitivity, epsilon) = (num("sensitivity")?, num("epsilon")?);
            if !(sensitivity > 0.0 && epsilon > 0.0) {
                return Err("`noise` needs sensitivity > 0 and epsilon > 0".into());
            }
            let at = match p.get("at").map(|v| v.as_str()) {
                None | Some(Some("aggregate")) => NoiseAt::Aggregate,
                Some(Some("row")) => NoiseAt::Row,
                _ => return Err("`noise` takes `at: aggregate` (the default) or `at: row`".into()),
            };
            let numeric = exposed.iter().any(|e| {
                e.name == column
                    && (e.data_type.is_numeric()
                        || matches!(
                            e.data_type,
                            datafusion_common::arrow::datatypes::DataType::Decimal128(..)
                        ))
            });
            if !numeric {
                return Err(format!("`noise` needs a numeric column; `{column}` is not"));
            }
            ShapeOp::Noise {
                column,
                sensitivity,
                epsilon,
                budget: text("budget")?,
                at,
            }
        }
        "suppress" => {
            let k = p
                .get("k")
                .and_then(Value::as_u64)
                .filter(|k| *k >= 1)
                .ok_or("`suppress` needs an integer k >= 1")?;
            if r.column.is_some() {
                return Err("`suppress` applies to groups and takes no `column`".into());
            }
            ShapeOp::Suppress { k }
        }
        "sample" => {
            let fraction = num("fraction")?;
            if !(fraction > 0.0 && fraction <= 1.0) {
                return Err("`sample` needs 0 < fraction <= 1".into());
            }
            let key = text("key")?;
            exposed_col(&key)?;
            ShapeOp::Sample { fraction, key }
        }
        _ => unreachable!("operator validated above"),
    })
}
