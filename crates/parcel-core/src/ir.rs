//! parcel's typed expression IR.
//!
//! The CEL parser produces an untyped AST in which macros are already expanded
//! into raw comprehensions. The checker lowers that AST into this IR: every
//! node is typed, every variable is resolved to its namespace, every function
//! call is pinned to a registry entry, and macros are named again. Everything
//! after the check (classify, split, translate, hashing) works on this IR only.

use std::collections::BTreeSet;
use std::fmt;

use serde::Serialize;

use crate::registry::FunctionPin;
use crate::types::Type;

/// Which namespaces an expression reads (design section 3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct NsSet {
    pub row: bool,
    pub ctx: bool,
    pub dataset: bool,
}

impl NsSet {
    pub const ROW: NsSet = NsSet {
        row: true,
        ctx: false,
        dataset: false,
    };
    pub const CTX: NsSet = NsSet {
        row: false,
        ctx: true,
        dataset: false,
    };
    pub const DATASET: NsSet = NsSet {
        row: false,
        ctx: false,
        dataset: true,
    };

    pub fn union(self, o: NsSet) -> NsSet {
        NsSet {
            row: self.row || o.row,
            ctx: self.ctx || o.ctx,
            dataset: self.dataset || o.dataset,
        }
    }

    pub fn is_empty(self) -> bool {
        !(self.row || self.ctx || self.dataset)
    }
}

impl fmt::Display for NsSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = [
            (self.row, "row"),
            (self.ctx, "ctx"),
            (self.dataset, "dataset"),
        ]
        .into_iter()
        .filter_map(|(on, n)| on.then_some(n))
        .collect();
        if names.is_empty() {
            f.write_str("none")
        } else {
            f.write_str(&names.join(", "))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CtxField {
    Id,
    Tenant,
    Purpose,
    Tier,
    Clearance,
    Roles,
    Now,
}

impl CtxField {
    pub const ALL: [CtxField; 7] = [
        CtxField::Id,
        CtxField::Tenant,
        CtxField::Purpose,
        CtxField::Tier,
        CtxField::Clearance,
        CtxField::Roles,
        CtxField::Now,
    ];

    pub fn name(self) -> &'static str {
        match self {
            CtxField::Id => "id",
            CtxField::Tenant => "tenant",
            CtxField::Purpose => "purpose",
            CtxField::Tier => "tier",
            CtxField::Clearance => "clearance",
            CtxField::Roles => "roles",
            CtxField::Now => "now",
        }
    }

    pub fn from_name(s: &str) -> Option<CtxField> {
        CtxField::ALL.into_iter().find(|f| f.name() == s)
    }

    pub fn ty(self) -> Type {
        match self {
            CtxField::Clearance => Type::Int,
            CtxField::Roles => Type::list(Type::String),
            CtxField::Now => Type::Timestamp,
            _ => Type::String,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnStat {
    NullCount,
    NullRate,
    DistinctCount,
    Min,
    Max,
}

impl ColumnStat {
    pub fn from_name(s: &str) -> Option<ColumnStat> {
        Some(match s {
            "null_count" => ColumnStat::NullCount,
            "null_rate" => ColumnStat::NullRate,
            "distinct_count" => ColumnStat::DistinctCount,
            "min" => ColumnStat::Min,
            "max" => ColumnStat::Max,
            _ => return None,
        })
    }
}

/// A value from the `dataset` namespace: one number in the manifest.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetField {
    RowCount,
    WrittenAt,
    ContractHash,
    Column { column: String, stat: ColumnStat },
    AssertionPassRate { assertion: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Var {
    Row(String),
    Ctx(CtxField),
    Dataset(DatasetField),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Lit {
    Bool(bool),
    Int(i64),
    Uint(u64),
    Double(f64),
    String(String),
    Bytes(Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BinOp {
    And,
    Or,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    In,
}

/// Functions that are part of the CEL profile itself (design section 6),
/// as opposed to registry functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Builtin {
    Size,
    StartsWith,
    EndsWith,
    Contains,
    Matches,
    ToInt,
    ToUint,
    ToDouble,
    ToString,
    ToTimestamp,
    ToDuration,
    GetFullYear,
    GetMonth,
    GetDayOfMonth,
    GetDayOfWeek,
    GetDayOfYear,
    GetHours,
    GetMinutes,
    GetSeconds,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MacroKind {
    Exists,
    All,
    Filter,
    Map,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExprKind {
    Lit(Lit),
    Var(Var),
    /// A comprehension's iteration variable.
    Local(String),
    List(Vec<TExpr>),
    Not(Box<TExpr>),
    Neg(Box<TExpr>),
    Binary(BinOp, Box<TExpr>, Box<TExpr>),
    Cond(Box<TExpr>, Box<TExpr>, Box<TExpr>),
    /// `has(row.col)`: the column is not null.
    Has(String),
    Builtin(Builtin, Vec<TExpr>),
    Call(FunctionPin, Vec<TExpr>),
    Macro {
        kind: MacroKind,
        var: String,
        range: Box<TExpr>,
        body: Box<TExpr>,
    },
}

/// A typed, resolved expression node.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TExpr {
    pub kind: ExprKind,
    pub ty: Type,
    /// Namespaces read by this node and everything below it.
    pub ns: NsSet,
}

impl TExpr {
    pub fn new(kind: ExprKind, ty: Type) -> TExpr {
        let ns = match &kind {
            ExprKind::Var(Var::Row(_)) | ExprKind::Has(_) => NsSet::ROW,
            ExprKind::Var(Var::Ctx(_)) => NsSet::CTX,
            ExprKind::Var(Var::Dataset(_)) => NsSet::DATASET,
            _ => NsSet::default(),
        };
        let mut e = TExpr { kind, ty, ns };
        let mut ns = e.ns;
        e.for_each_child(|c| ns = ns.union(c.ns));
        e.ns = ns;
        e
    }

    pub fn for_each_child(&self, mut f: impl FnMut(&TExpr)) {
        match &self.kind {
            ExprKind::Lit(_) | ExprKind::Var(_) | ExprKind::Local(_) | ExprKind::Has(_) => {}
            ExprKind::List(xs) | ExprKind::Builtin(_, xs) | ExprKind::Call(_, xs) => {
                xs.iter().for_each(f)
            }
            ExprKind::Not(x) | ExprKind::Neg(x) => f(x),
            ExprKind::Binary(_, a, b) => {
                f(a);
                f(b)
            }
            ExprKind::Cond(a, b, c) => {
                f(a);
                f(b);
                f(c)
            }
            ExprKind::Macro { range, body, .. } => {
                f(range);
                f(body)
            }
        }
    }

    /// Visit this node and every descendant, parents first.
    pub fn walk(&self, f: &mut impl FnMut(&TExpr)) {
        f(self);
        self.for_each_child(|c| c.walk(f));
    }

    pub fn ctx_fields(&self) -> BTreeSet<CtxField> {
        let mut out = BTreeSet::new();
        self.walk(&mut |e| {
            if let ExprKind::Var(Var::Ctx(f)) = &e.kind {
                out.insert(*f);
            }
        });
        out
    }

    pub fn row_columns(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        self.walk(&mut |e| match &e.kind {
            ExprKind::Var(Var::Row(c)) | ExprKind::Has(c) => {
                out.insert(c.clone());
            }
            _ => {}
        });
        out
    }

    pub fn pins(&self) -> BTreeSet<FunctionPin> {
        let mut out = BTreeSet::new();
        self.walk(&mut |e| {
            if let ExprKind::Call(p, _) = &e.kind {
                out.insert(p.clone());
            }
        });
        out
    }
}
