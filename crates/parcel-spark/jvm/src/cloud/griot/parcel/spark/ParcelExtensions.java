package cloud.griot.parcel.spark;

import org.apache.spark.sql.SparkSessionExtensions;
import scala.Function1;
import scala.runtime.BoxedUnit;

/**
 * The parcel Spark extension: {@code spark.sql.extensions=cloud.griot.parcel.spark.ParcelExtensions}.
 *
 * <p>It adds one analyser rule, {@link ParcelResolution}, which resolves a compiled contract's
 * name ({@code `sales/customers@1.0.0`}) to its governed query.
 */
public final class ParcelExtensions implements Function1<SparkSessionExtensions, BoxedUnit> {
  @Override
  public BoxedUnit apply(SparkSessionExtensions extensions) {
    extensions.injectResolutionRule(ParcelResolution::new);
    return BoxedUnit.UNIT;
  }
}
