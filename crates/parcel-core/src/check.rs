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

use crate::checker::{Env, check_expr};
use crate::diag::{Code, Diagnostic};
use crate::document::*;
use crate::hash;
use crate::ir::{CtxField, NsSet, TExpr};
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
    },
    Suppress {
        k: u64,
    },
    Sample {
        fraction: f64,
        key: String,
    },
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
    pub binding: Binding,
    pub exposed: Vec<ExposedColumn>,
    pub rules: Vec<CheckedRule>,
    /// Every registry entry the contract is pinned to.
    pub functions: BTreeSet<FunctionPin>,
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
    let mut d: Vec<Diagnostic> = Vec::new();
    let e = |d: &mut Vec<Diagnostic>, code, rule: Option<&str>, msg: String| {
        d.push(Diagnostic::new(code, rule, msg))
    };

    for (field, present) in [
        ("inherits", doc.inherits.is_some()),
        ("extensions", doc.extensions.is_some()),
        ("enrich", doc.enrich.is_some()),
        ("dataset_other", doc.dataset_other.is_some()),
    ] {
        if present {
            e(
                &mut d,
                Code::Unsupported,
                None,
                format!("`{field}` is not supported in parcel v0"),
            );
        }
    }

    let columns: BTreeMap<String, Result<Type, String>> = schema
        .fields()
        .iter()
        .map(|f| (f.name().clone(), Type::from_arrow(f.data_type())))
        .collect();

    for p in &doc.binding.partitioned_by {
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
        if let Rule::Transform(t) = r
            && transforms.insert(t.column.as_str(), t).is_some()
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
    for col in &doc.expose {
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
    };

    let mut rules = Vec::new();
    for r in &doc.rules {
        match classify_rule(r, &env, &exposed) {
            Ok(c) => rules.push(c),
            Err(mut errs) => d.append(&mut errs),
        }
    }

    if !d.is_empty() {
        return Err(d);
    }
    let functions = rules
        .iter()
        .flat_map(|r| r.exprs())
        .flat_map(|x| x.expr.pins())
        .collect();
    Ok(CheckedContract {
        name: doc.contract.clone(),
        version: doc.version,
        contract_hash: contract_hash(doc),
        binding: doc.binding.clone(),
        exposed,
        rules,
        functions,
    })
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
            if x.expr.ctx_fields().iter().any(|f| *f != CtxField::Now) {
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
        "noise" => &["sensitivity", "epsilon", "budget"],
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
            ShapeOp::Noise {
                column,
                sensitivity,
                epsilon,
                budget: text("budget")?,
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
