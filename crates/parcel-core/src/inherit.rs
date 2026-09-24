//! Inheritance (design 12): flatten a chain of contracts into one, restriction only.
//!
//! - `expose` is intersected: a child lists a subset of its parent's columns, with the same
//!   types, or omits the section to keep them all.
//! - `binding` is shared: a child omits it or repeats the parent's.
//! - `decide`, `admit`, `assert` and `guarantee` rules are unioned; `shape` operators accumulate.
//! - `transform` rules compose, parent first: a child's rules see the parent's transformed
//!   values, and a child transform may read only what the parent exposes.
//!
//! A child cannot remove or redefine a parent rule. peQL only ever sees flat contracts.

use std::collections::BTreeSet;

use crate::check::Layer;
use crate::diag::{Code, Diagnostic};
use crate::document::ContractDoc;
use crate::types::parse_type_name;

/// A flattened contract and the layers it came from, root first.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub doc: ContractDoc,
    /// Empty for a contract that inherits nothing.
    pub layers: Vec<Layer>,
}

/// Flatten `doc`'s inheritance chain, looking parents up by contract name.
pub fn resolve(
    doc: &ContractDoc,
    lookup: &dyn Fn(&str) -> Option<ContractDoc>,
) -> Result<Resolved, Vec<Diagnostic>> {
    resolve_inner(doc, lookup, &mut Vec::new())
}

fn err(msg: String) -> Vec<Diagnostic> {
    vec![Diagnostic::new(Code::Inheritance, None, msg)]
}

fn resolve_inner(
    doc: &ContractDoc,
    lookup: &dyn Fn(&str) -> Option<ContractDoc>,
    chain: &mut Vec<String>,
) -> Result<Resolved, Vec<Diagnostic>> {
    if chain.contains(&doc.contract) {
        chain.push(doc.contract.clone());
        return Err(err(format!("inheritance cycle: {}", chain.join(" -> "))));
    }
    chain.push(doc.contract.clone());
    let Some(parent_name) = &doc.inherits else {
        return Ok(Resolved {
            doc: doc.clone(),
            layers: Vec::new(),
        });
    };
    let parent_doc = lookup(parent_name).ok_or_else(|| {
        err(format!(
            "`{}` inherits `{parent_name}`, which is not registered",
            doc.contract
        ))
    })?;
    let parent = resolve_inner(&parent_doc, lookup, chain)?;
    let p = &parent.doc;

    let binding = match (&doc.binding, &p.binding) {
        (None, b) => b.clone(),
        (Some(c), Some(pb)) if c == pb => Some(c.clone()),
        (Some(_), _) => {
            return Err(err(format!(
                "`{}` binds different data from its parent `{parent_name}`; a child contract is a view of its parent's data",
                doc.contract
            )));
        }
    };

    let parent_expose = p.expose.clone().unwrap_or_default();
    let expose = match &doc.expose {
        None => parent_expose.clone(),
        Some(cols) => {
            let mut out = Vec::new();
            let mut errs = Vec::new();
            for c in cols {
                match parent_expose.iter().find(|pc| pc.name == c.name) {
                    None => errs.push(Diagnostic::new(
                        Code::Inheritance,
                        None,
                        format!("`{}` is not exposed by `{parent_name}`; a child can only narrow what its parent exposes", c.name),
                    )),
                    Some(pc) if parse_type_name(&pc.type_name).ok() != parse_type_name(&c.type_name).ok() => {
                        errs.push(Diagnostic::new(
                            Code::Inheritance,
                            None,
                            format!("`{}` is {} in `{parent_name}` and cannot change type to {}", c.name, pc.type_name, c.type_name),
                        ))
                    }
                    Some(_) => out.push(c.clone()),
                }
            }
            if !errs.is_empty() {
                return Err(errs);
            }
            out
        }
    };

    let parent_ids: BTreeSet<&str> = p.rules.iter().map(|r| r.id()).collect();
    let clashes: Vec<Diagnostic> = doc
        .rules
        .iter()
        .filter(|r| parent_ids.contains(r.id()))
        .map(|r| {
            Diagnostic::new(
                Code::Inheritance,
                Some(r.id()),
                format!("`{parent_name}` already has a rule `{}`; a child cannot redefine or remove its parent's rules", r.id()),
            )
        })
        .collect();
    if !clashes.is_empty() {
        return Err(clashes);
    }

    let mut rules = p.rules.clone();
    rules.extend(doc.rules.iter().cloned());
    let mut layers = parent.layers.clone();
    if layers.is_empty() {
        layers.push(Layer {
            contract: p.contract.clone(),
            rules: p.rules.iter().map(|r| r.id().to_owned()).collect(),
            visible: BTreeSet::new(),
            expose: parent_expose.clone(),
        });
    }
    layers.push(Layer {
        contract: doc.contract.clone(),
        rules: doc.rules.iter().map(|r| r.id().to_owned()).collect(),
        visible: parent_expose.iter().map(|c| c.name.clone()).collect(),
        expose: expose.clone(),
    });

    let flat = ContractDoc {
        contract: doc.contract.clone(),
        version: doc.version,
        inherits: None,
        binding,
        expose: Some(expose),
        rules,
        extensions: doc.extensions.clone().or_else(|| p.extensions.clone()),
        enrich: doc.enrich.clone().or_else(|| p.enrich.clone()),
        dataset_other: doc
            .dataset_other
            .clone()
            .or_else(|| p.dataset_other.clone()),
    };
    Ok(Resolved { doc: flat, layers })
}
