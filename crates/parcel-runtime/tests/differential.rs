//! The differential test across the whole CEL profile, and proof that it catches drift.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use datafusion::arrow::array::*;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use datafusion::logical_expr::lit;
use parcel_core::{ContractDoc, Registry, compile};
use parcel_runtime::Caller;
use parcel_runtime::differential::differential;

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("phone", DataType::Utf8, true),
        Field::new("score", DataType::Float64, true),
        Field::new("n", DataType::Int32, true),
        Field::new("u", DataType::UInt32, true),
        Field::new("tenant", DataType::Utf8, true),
        Field::new("at", DataType::Timestamp(TimeUnit::Nanosecond, None), true),
        Field::new(
            "tags",
            DataType::List(Field::new_list_field(DataType::Utf8, true).into()),
            true,
        ),
        Field::new(
            "nums",
            DataType::List(Field::new_list_field(DataType::Int64, true).into()),
            true,
        ),
        Field::new("blob", DataType::Binary, true),
    ]))
}

fn data() -> RecordBatch {
    let names = [
        Some("Amina"),
        Some("héllo wörld"),
        None,
        Some(""),
        Some("Otieno Ochieng"),
        Some("x@y.io"),
        Some("ÜBER"),
        Some("a\"b\\c"),
    ];
    let phones = [
        Some("254712345678"),
        Some("0712345678"),
        Some("254112345678"),
        None,
        Some("25471234567"),
        Some("254712345678"),
        Some("+254712345678"),
        Some("254799999999"),
    ];
    let scores = [
        Some(0.5),
        Some(-1.25),
        None,
        Some(1e10),
        Some(0.0),
        Some(3.75),
        Some(-0.0),
        Some(99.9),
    ];
    let ns = [
        Some(1),
        Some(-7),
        Some(0),
        None,
        Some(i32::MAX),
        Some(42),
        Some(-1),
        Some(5),
    ];
    let us = [
        Some(1u32),
        Some(0),
        Some(7),
        Some(100),
        None,
        Some(3),
        Some(9),
        Some(u32::MAX),
    ];
    let tenants = [
        Some("acme"),
        Some("globex"),
        Some("acme"),
        None,
        Some("initech"),
        Some("globex"),
        Some("acme"),
        Some("globex"),
    ];
    let ats = [
        Some(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()),
        Some(Utc.with_ymd_and_hms(2024, 2, 29, 23, 59, 59).unwrap()),
        Some(Utc.with_ymd_and_hms(1999, 12, 31, 12, 30, 5).unwrap()),
        None,
        Some(Utc.with_ymd_and_hms(2030, 7, 4, 6, 7, 8).unwrap()),
        Some(Utc.with_ymd_and_hms(2024, 12, 31, 1, 2, 3).unwrap()),
        Some(Utc.with_ymd_and_hms(2024, 3, 1, 10, 0, 0).unwrap()),
        Some(Utc.with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap()),
    ];
    let mut tags = ListBuilder::new(StringBuilder::new());
    let tag_rows: [Option<&[&str]>; 8] = [
        Some(&["a", "b"]),
        Some(&[]),
        None,
        Some(&["acme"]),
        Some(&["x", "globex", "y"]),
        Some(&["b"]),
        Some(&["a", "a"]),
        Some(&["zz"]),
    ];
    for t in tag_rows {
        match t {
            Some(t) => {
                for v in t {
                    tags.values().append_value(v);
                }
                tags.append(true)
            }
            None => tags.append(false),
        }
    }
    let mut nums = ListBuilder::new(Int64Builder::new());
    let num_rows: [&[i64]; 8] = [
        &[1, 2, 3],
        &[],
        &[-5],
        &[10, 20],
        &[0],
        &[7, 7, 7],
        &[100],
        &[2, 4],
    ];
    for r in num_rows {
        for v in r {
            nums.values().append_value(*v);
        }
        nums.append(true);
    }
    let blobs: Vec<Option<&[u8]>> = vec![
        Some(b"abc"),
        Some(b""),
        None,
        Some(&[0, 255]),
        Some(b"hello"),
        Some(b"x"),
        Some(b"yy"),
        Some(b"z"),
    ];
    RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(Int64Array::from((0..8).collect::<Vec<i64>>())),
            Arc::new(StringArray::from(names.to_vec())),
            Arc::new(StringArray::from(phones.to_vec())),
            Arc::new(Float64Array::from(scores.to_vec())),
            Arc::new(Int32Array::from(ns.to_vec())),
            Arc::new(UInt32Array::from(us.to_vec())),
            Arc::new(StringArray::from(tenants.to_vec())),
            Arc::new(TimestampNanosecondArray::from(
                ats.iter()
                    .map(|t| t.map(|t| t.timestamp_nanos_opt().unwrap()))
                    .collect::<Vec<_>>(),
            )),
            Arc::new(tags.finish()),
            Arc::new(nums.finish()),
            Arc::new(BinaryArray::from(blobs)),
        ],
    )
    .unwrap()
}

const PROFILE: &str = r#"
contract: test/profile
version: 1
binding: {parquet: profile/}
expose:
  - {name: id, type: int64}
  - {name: name, type: utf8}
  - {name: phone, type: utf8}
  - {name: score, type: float64}
  - {name: n, type: utf8}
  - {name: tenant, type: utf8}
  - {name: u, type: uint32}
rules:
  # admits mixing row and ctx
  - {id: a_tenant, op: admit, expr: "row.tenant == ctx.tenant || 'admin' in ctx.roles"}
  - {id: a_in_list, op: admit, expr: "row.tenant in ['acme', 'globex']"}
  - {id: a_tags_ctx, op: admit, expr: "row.tags.exists(t, t == ctx.tenant)"}
  - {id: a_roles_all, op: admit, expr: "ctx.roles.all(r, r != 'banned') && row.n > 0"}
  - {id: a_clearance, op: admit, expr: "ctx.clearance >= 2 || row.score < 1.0"}
  - {id: a_time, op: admit, expr: "row.at < ctx.now - duration('24h')"}
  - {id: a_not_has, op: admit, expr: "!has(row.name) || row.name != ''"}
  # asserts across the profile
  - {id: s_len, op: assert, expr: "size(row.name) > 4", on_fail: report}
  - {id: s_prefix, op: assert, expr: "row.phone.startsWith('254') && row.phone.endsWith('8')", on_fail: report}
  - {id: s_contains, op: assert, expr: "row.name.contains('o')", on_fail: report}
  - {id: s_matches, op: assert, expr: "row.phone.matches('^254[17][0-9]{8}$')", on_fail: report}
  - {id: s_msisdn, op: assert, expr: "is_msisdn(row.phone)", on_fail: report}
  - {id: s_email, op: assert, expr: "is_email(row.name)", on_fail: report}
  - {id: s_concat, op: assert, expr: "row.name + '!' == 'Amina!'", on_fail: report}
  - {id: n_arith, op: assert, expr: "(row.n * 3 + 1) % 2 == 1", on_fail: report}
  - {id: n_div, op: assert, expr: "row.n / 2 >= 0", on_fail: report}
  - {id: n_neg, op: assert, expr: "-row.n < 5", on_fail: report}
  - {id: n_uint, op: assert, expr: "row.u > 5u", on_fail: report}
  - {id: n_double, op: assert, expr: "row.score * 2.0 > 1.0", on_fail: report}
  - {id: n_conv, op: assert, expr: "double(row.n) < row.score", on_fail: report}
  - {id: n_int, op: assert, expr: "int(row.score) == 0", on_fail: report}
  - {id: n_str, op: assert, expr: "string(row.n) == '42'", on_fail: report}
  - {id: t_year, op: assert, expr: "row.at.getFullYear() == 2024", on_fail: report}
  - {id: t_month, op: assert, expr: "row.at.getMonth() == 1", on_fail: report}
  - {id: t_dom, op: assert, expr: "row.at.getDayOfMonth() == 28", on_fail: report}
  - {id: t_dow, op: assert, expr: "row.at.getDayOfWeek() == 1", on_fail: report}
  - {id: t_doy, op: assert, expr: "row.at.getDayOfYear() == 59", on_fail: report}
  - {id: t_hms, op: assert, expr: "row.at.getHours() * 3600 + row.at.getMinutes() * 60 + row.at.getSeconds() > 3600", on_fail: report}
  - {id: t_lit, op: assert, expr: "row.at >= timestamp('2024-01-01T00:00:00Z')", on_fail: report}
  - {id: l_size, op: assert, expr: "size(row.tags) == 2", on_fail: report}
  - {id: l_exists, op: assert, expr: "row.nums.exists(x, x > 5)", on_fail: report}
  - {id: l_all, op: assert, expr: "row.nums.all(x, x % 2 == 0)", on_fail: report}
  - {id: l_filter, op: assert, expr: "size(row.nums.filter(x, x > 1)) >= 2", on_fail: report}
  - {id: l_map, op: assert, expr: "20 in row.nums.map(x, x * 2)", on_fail: report}
  - {id: l_in, op: assert, expr: "'a' in row.tags", on_fail: report}
  - {id: b_size, op: assert, expr: "size(row.blob) == 3", on_fail: report}
  - {id: c_cond, op: assert, expr: "(row.n > 0 ? row.name : row.phone) != ''", on_fail: report}
  # transforms
  - {id: x_mask, op: transform, column: name, expr: "ctx.tenant == 'acme' ? row.name : hash_sha256(row.name)"}
  - {id: x_redact, op: transform, column: phone, expr: "redact(row.phone)"}
  - {id: x_score, op: transform, column: score, expr: "row.score * 100.0"}
  # a number exposed as text: in clear for cleared callers, partly masked for others
  - {id: x_retype, op: transform, column: n, expr: "ctx.clearance >= 2 ? string(row.n) : partial(string(row.n), 2)"}
  # null for callers without the classification
  - {id: x_null, op: transform, column: tenant, expr: "ctx.classification == 'restricted' ? row.tenant : null"}
  - {id: x_null_left, op: transform, column: u, expr: "'admin' in ctx.roles ? null : row.u"}
  - {id: s_partial, op: assert, expr: "partial(row.phone, 4).endsWith('5678')", on_fail: report}
"#;

fn callers() -> Vec<Caller> {
    let now = Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();
    let mut high = Caller::new("h", "globex", "analytics")
        .with_roles(&["admin"])
        .at(now);
    high.clearance = 3;
    high.classification = "restricted".into();
    vec![
        Caller::new("a", "acme", "analytics").at(now),
        high,
        Caller::new("b", "initech", "analytics")
            .with_roles(&["banned"])
            .at(now),
    ]
}

#[tokio::test]
async fn interpreter_and_datafusion_agree_across_the_profile() {
    let doc = ContractDoc::parse(PROFILE).unwrap();
    let c = compile(&doc, &schema(), &Registry::builtin()).unwrap_or_else(|d| panic!("{d:#?}"));
    let diff = differential(&c, &data(), &callers()).await.unwrap();
    let shown: Vec<String> = diff
        .mismatches
        .iter()
        .map(|m| {
            format!(
                "{} row {} ({}): cel={} df={}",
                m.rule, m.row, m.caller, m.reference, m.datafusion
            )
        })
        .collect();
    assert!(
        diff.passed(),
        "{} mismatches:\n{}",
        shown.len(),
        shown.join("\n")
    );
    assert!(diff.evaluations >= 8 * 40);
    assert_eq!(
        diff.both_errored, 0,
        "every evaluation produced a value on both sides"
    );
}

#[tokio::test]
async fn a_drifted_translation_is_caught() {
    let doc = ContractDoc::parse(PROFILE).unwrap();
    let mut c = compile(&doc, &schema(), &Registry::builtin()).unwrap();
    let flag = c
        .contract
        .flags
        .iter_mut()
        .find(|f| f.assert_id == "s_len")
        .unwrap();
    flag.expr = lit(true);
    let diff = differential(&c, &data(), &callers()).await.unwrap();
    assert!(!diff.passed());
    assert!(diff.mismatches.iter().all(|m| m.rule == "s_len"));
}

#[tokio::test]
async fn bundles_round_trip_and_verify() {
    use parcel_runtime::bundle::Bundle;
    let doc = ContractDoc::parse(PROFILE).unwrap();
    let c = compile(&doc, &schema(), &Registry::builtin()).unwrap();
    let json = Bundle::new(&doc, &[], &schema(), &c)
        .unwrap()
        .to_json()
        .unwrap();
    let back = Bundle::from_json(&json).unwrap();
    let verified = back.verify().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        verified.contract.compilation_hash,
        c.contract.compilation_hash
    );

    // Tampering with the document or an expression is detected.
    let mut bad = Bundle::from_json(&json).unwrap();
    bad.document.version += 1;
    assert!(bad.verify().is_err());
    let mut bad = Bundle::from_json(&json).unwrap();
    let other = bad.exprs["flag/s_len"].clone();
    bad.exprs.insert("flag/s_prefix".into(), other);
    assert!(bad.verify().unwrap_err().contains("flag/s_prefix"));
}
