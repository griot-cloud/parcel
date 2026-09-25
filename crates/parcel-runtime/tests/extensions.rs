//! Extensions: row.other via enrichers, ctx.other from the caller, dataset.other at write.

use std::sync::Arc;

use datafusion::arrow::array::*;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::MemTable;
use parcel_core::{Code, ContractDoc, Registry, compile};
use parcel_runtime::Caller;
use parcel_runtime::bundle::Bundle;
use parcel_runtime::differential::differential;
use parcel_runtime::plan::{selectivity, validate};

fn table(b: RecordBatch) -> Arc<MemTable> {
    Arc::new(MemTable::try_new(schema(), vec![vec![b]]).unwrap())
}

const LOANS: &str = r#"
contract: credit/loans
version: 1
binding: {parquet: loans/}
expose:
  - {name: loan_id, type: int64}
  - {name: amount, type: float64}
  - {name: score, type: int64}
extensions:
  row: {risk_band: utf8, flagged: bool}
  ctx: {department: utf8, max_amount: float64}
  dataset: {source_system: utf8, median_amount: float64, borrowers: int64}
enrich:
  - {field: risk_band, expr: "row.score >= 700 ? 'low' : (row.score >= 500 ? 'medium' : 'high')"}
  - {field: flagged, expr: "row.other.risk_band == 'high' && row.amount > 5000.0"}
dataset_other:
  - {field: source_system, value: "mpesa-loans-v2"}
  - {field: median_amount, expr: "median(row.amount)"}
  - {field: borrowers, expr: "count_distinct(row.borrower)"}
rules:
  - id: risk_team_or_low_risk
    op: admit
    expr: "ctx.other.department == 'risk' || row.other.risk_band == 'low'"
  - id: within_limit
    op: admit
    expr: "row.amount <= ctx.other.max_amount"
  - id: banded
    op: assert
    expr: "has(row.other.risk_band)"
    on_fail: deny
  - id: few_flagged
    op: assert
    expr: "!row.other.flagged"
    on_fail: report
  - id: known_source
    op: guarantee
    expr: "dataset.other.source_system == 'mpesa-loans-v2'"
    on_fail: deny
  - id: sane_median
    op: guarantee
    expr: "dataset.other.median_amount > 100.0 && dataset.other.borrowers > 10"
    on_fail: annotate
"#;

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("loan_id", DataType::Int64, false),
        Field::new("borrower", DataType::Int64, false),
        Field::new("amount", DataType::Float64, true),
        Field::new("score", DataType::Int64, true),
    ]))
}

fn batch() -> RecordBatch {
    let n = 300i64;
    RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(Int64Array::from((0..n).collect::<Vec<_>>())),
            Arc::new(Int64Array::from((0..n).map(|i| i % 50).collect::<Vec<_>>())),
            Arc::new(Float64Array::from(
                (0..n)
                    .map(|i| {
                        if i % 37 == 0 {
                            None
                        } else {
                            Some(200.0 + (i * 97 % 90) as f64 * 100.0)
                        }
                    })
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Int64Array::from(
                (0..n)
                    .map(|i| {
                        if i % 41 == 0 {
                            None
                        } else {
                            Some(300 + (i * 13 % 550))
                        }
                    })
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .unwrap()
}

fn caller(department: &str, max: f64) -> Caller {
    let mut c = Caller::new("u", "bank", "analytics");
    c.other
        .insert("department".into(), serde_json::json!(department));
    c.other.insert("max_amount".into(), serde_json::json!(max));
    c
}

#[tokio::test]
async fn extensions_end_to_end() {
    let doc = ContractDoc::parse(LOANS).unwrap();
    let comp = compile(&doc, &schema(), &Registry::builtin()).unwrap_or_else(|d| panic!("{d:#?}"));
    let v = &validate(&comp, comp.validation.plan.clone(), table(batch()))
        .await
        .unwrap();
    // Rows with a null score get no band: the deny assert fails, the data is unservable.
    assert!(!v.valid);
    assert_eq!(v.breached, ["banded"]);
    assert_eq!(
        v.failures["banded"],
        (0..300).filter(|i| i % 41 == 0).count() as i64
    );
    assert_eq!(
        v.stats["other__source_system"],
        serde_json::json!("mpesa-loans-v2")
    );
    assert_eq!(v.stats["other__borrowers"], serde_json::json!(50));
    assert!(v.stats["other__median_amount"].as_f64().unwrap() > 100.0);

    // Give every loan a score: now it is servable.
    let fixed = {
        let b = batch();
        let scores: Int64Array = (0..300i64).map(|i| Some(300 + (i * 13 % 550))).collect();
        RecordBatch::try_new(
            schema(),
            vec![
                b.column(0).clone(),
                b.column(1).clone(),
                b.column(2).clone(),
                Arc::new(scores),
            ],
        )
        .unwrap()
    };
    let fixed_verdict = validate(&comp, comp.validation.plan.clone(), table(fixed.clone()))
        .await
        .unwrap();
    assert!(fixed_verdict.valid, "{:?}", fixed_verdict.breached);

    // ctx.other decides what each caller sees.
    let n = |c: Caller| {
        let comp = comp.clone();
        let data = table(fixed.clone());
        async move { selectivity(&comp, data, &c).await.unwrap() as i64 }
    };
    let risk = n(caller("risk", 1e9)).await;
    let sales = n(caller("sales", 1e9)).await;
    let capped = n(caller("risk", 3000.0)).await;
    let with_amount = (0..300).filter(|i| i % 37 != 0).count() as i64;
    assert_eq!(
        risk, with_amount,
        "the risk team sees every loan with an amount"
    );
    assert!(
        sales < risk && sales > 0,
        "others see low-risk loans only: {sales}"
    );
    assert!(capped < risk, "max_amount caps what is visible: {capped}");

    // A caller that does not supply a declared ctx.other field sees nothing (fails closed).
    let bare = Caller::new("u", "bank", "analytics");
    assert!(
        selectivity(&comp, table(fixed.clone()), &bare)
            .await
            .is_err()
    );

    // Differential: enrichers and every rule agree between the interpreter and DataFusion.
    let diff = differential(
        &comp,
        &batch(),
        &[caller("risk", 1e9), caller("sales", 5000.0)],
    )
    .await
    .unwrap();
    assert!(
        diff.passed(),
        "{:#?}",
        &diff.mismatches[..diff.mismatches.len().min(8)]
    );
    assert!(diff.evaluations > 1000);

    // Bundles carry enrichers and verify.
    let b = Bundle::new(&doc, &[], &schema(), &comp).unwrap();
    Bundle::from_json(&b.to_json().unwrap())
        .unwrap()
        .verify()
        .unwrap();
}

#[test]
fn extension_errors() {
    let reg = Registry::builtin();
    let run = |src: &str| -> Vec<Code> {
        let doc = ContractDoc::parse(src).unwrap();
        compile(&doc, &schema(), &reg)
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|d| d.code)
            .collect()
    };
    let base =
        "contract: x\nversion: 1\nbinding: {parquet: x}\nexpose: [{name: loan_id, type: int64}]\n";
    // Undeclared extension field.
    assert!(
        run(&format!(
            "{base}rules:\n  - {{id: a, op: admit, expr: \"row.other.band == 'x'\"}}\n"
        ))
        .contains(&Code::UnknownField)
    );
    // Declared but never produced.
    assert!(
        run(&format!("{base}extensions: {{row: {{band: utf8}}}}\n")).contains(&Code::UnknownField)
    );
    // An enricher reading ctx.
    assert!(run(&format!("{base}extensions: {{row: {{band: utf8}}}}\nenrich:\n  - {{field: band, expr: \"ctx.tenant\"}}\n")).contains(&Code::Namespace));
    // An enricher reading a later one.
    assert!(run(&format!("{base}extensions: {{row: {{a: utf8, b: utf8}}}}\nenrich:\n  - {{field: a, expr: \"row.other.b\"}}\n  - {{field: b, expr: \"'x'\"}}\n")).contains(&Code::UnknownField));
    // Wrong type.
    assert!(
        run(&format!(
            "{base}extensions: {{row: {{a: int64}}}}\nenrich:\n  - {{field: a, expr: \"'x'\"}}\n"
        ))
        .contains(&Code::TypeMismatch)
    );
    // A guarantee reading ctx.other.
    assert!(run(&format!("{base}extensions: {{ctx: {{d: utf8}}}}\nrules:\n  - {{id: g, op: guarantee, expr: \"ctx.other.d == 'x'\", on_fail: deny}}\n")).contains(&Code::Namespace));
    // A dataset producer with a bad aggregate or value type.
    assert!(run(&format!("{base}extensions: {{dataset: {{m: float64}}}}\ndataset_other:\n  - {{field: m, expr: \"quantile(row.amount, 0.9)\"}}\n")).contains(&Code::UnknownFunction));
    assert!(run(&format!("{base}extensions: {{dataset: {{m: int64}}}}\ndataset_other:\n  - {{field: m, value: \"nope\"}}\n")).contains(&Code::TypeMismatch));
    assert!(run(&format!("{base}extensions: {{dataset: {{m: int64}}}}\ndataset_other:\n  - {{field: m, expr: \"avg(row.amount)\"}}\n")).contains(&Code::TypeMismatch));
}
