//! Scalar functions that compiled contracts call and every engine registers: the ones
//! DataFusion has no equivalent for. [`parcel_udfs`] is the list an engine installs.

use std::sync::Arc;

use datafusion_common::arrow::array::{Array, ArrayRef, Float64Array, StringArray, UInt64Array};
use datafusion_common::arrow::datatypes::DataType;
use datafusion_common::{DataFusionError, Result, ScalarValue};
use datafusion_expr::{
    ColumnarValue, Expr, Operator, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature,
    Volatility, binary_expr, cast, lit,
};

/// Buckets a sampled key falls into; a `fraction` keeps the buckets below `fraction * SAMPLE_BUCKETS`.
pub const SAMPLE_BUCKETS: u64 = 1_000_000;

/// Every function compiled contracts may call besides DataFusion's own.
pub fn parcel_udfs() -> Vec<ScalarUDF> {
    vec![
        crate::translate::bytes_len_udf(),
        laplace_udf(),
        sample_bucket_udf(),
    ]
}

/// `parcel_laplace(value, scale)`: `value` plus a draw from Laplace(0, scale).
pub fn laplace_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(Laplace {
        signature: Signature::exact(
            vec![DataType::Float64, DataType::Float64],
            Volatility::Volatile,
        ),
    })
}

/// `parcel_sample_bucket(value)`: a stable bucket in `[0, SAMPLE_BUCKETS)` from the value's text.
pub fn sample_bucket_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(SampleBucket {
        signature: Signature::any(1, Volatility::Immutable),
    })
}

/// `value` with Laplace noise of `scale`, in the column's own type. Integers are rounded.
pub fn noised(value: Expr, scale: f64, ty: &DataType) -> Expr {
    let noisy = laplace_udf().call(vec![cast(value, DataType::Float64), lit(scale)]);
    if ty.is_integer() {
        cast(
            datafusion_functions::expr_fn::round(vec![noisy]),
            ty.clone(),
        )
    } else {
        cast(noisy, ty.clone())
    }
}

/// True for a stable `fraction` of rows, keyed on `key`: repeated queries see the same sample.
pub fn sample_predicate(key: Expr, fraction: f64) -> Expr {
    binary_expr(
        sample_bucket_udf().call(vec![key]),
        Operator::Lt,
        lit((fraction * SAMPLE_BUCKETS as f64).round() as u64),
    )
}

/// One draw from Laplace(0, scale) by inverting its CDF over a uniform draw from the
/// thread's cryptographically seeded generator.
pub fn laplace_sample(scale: f64) -> f64 {
    loop {
        let u: f64 = rand::random::<f64>() - 0.5;
        if u.abs() < 0.5 {
            return -scale * u.signum() * (1.0 - 2.0 * u.abs()).ln();
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct Laplace {
    signature: Signature,
}

impl ScalarUDFImpl for Laplace {
    fn name(&self) -> &str {
        "parcel_laplace"
    }
    fn signature(&self) -> &Signature {
        &self.signature
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Float64)
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let scale = match &args.args[1] {
            ColumnarValue::Scalar(ScalarValue::Float64(Some(s))) if s.is_finite() && *s > 0.0 => *s,
            _ => {
                return Err(DataFusionError::Execution(
                    "parcel_laplace needs a positive, finite constant scale".into(),
                ));
            }
        };
        let v = args.args[0].to_array(args.number_rows)?;
        let v = v
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| DataFusionError::Internal("parcel_laplace expects float64".into()))?;
        let out: Float64Array = v
            .iter()
            .map(|x| x.map(|x| x + laplace_sample(scale)))
            .collect();
        Ok(ColumnarValue::Array(Arc::new(out) as ArrayRef))
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct SampleBucket {
    signature: Signature,
}

impl ScalarUDFImpl for SampleBucket {
    fn name(&self) -> &str {
        "parcel_sample_bucket"
    }
    fn signature(&self) -> &Signature {
        &self.signature
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::UInt64)
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let arr = args.args[0].to_array(args.number_rows)?;
        let text = datafusion_common::arrow::compute::cast(&arr, &DataType::Utf8)?;
        let text = text
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| DataFusionError::Internal("utf8 cast".into()))?;
        // FNV-1a: stable across processes and platforms, unlike the standard hasher.
        let out: UInt64Array = (0..text.len())
            .map(|i| {
                (!text.is_null(i)).then(|| {
                    let mut h: u64 = 0xcbf29ce484222325;
                    for b in text.value(i).bytes() {
                        h ^= b as u64;
                        h = h.wrapping_mul(0x100000001b3);
                    }
                    h % SAMPLE_BUCKETS
                })
            })
            .collect();
        Ok(ColumnarValue::Array(Arc::new(out) as ArrayRef))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn laplace_draws_are_centred_with_the_right_spread() {
        let n = 200_000;
        let draws: Vec<f64> = (0..n).map(|_| laplace_sample(2.0)).collect();
        let mean = draws.iter().sum::<f64>() / n as f64;
        let mad = draws.iter().map(|d| d.abs()).sum::<f64>() / n as f64;
        // E[x] = 0 and E|x| = scale for Laplace(0, scale).
        assert!(mean.abs() < 0.05, "mean {mean}");
        assert!((mad - 2.0).abs() < 0.05, "mean absolute deviation {mad}");
    }
}
