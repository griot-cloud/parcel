#!/usr/bin/env bash
# Build parcel-spark.jar against the Spark jars of SPARK_HOME, or of the pyspark package.
#   crates/parcel-spark/jvm/build.sh [out.jar]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-$here/parcel-spark.jar}
jars=${SPARK_HOME:+$SPARK_HOME/jars}
if [ -z "$jars" ]; then
  jars=$(python3 -c 'import os, pyspark; print(os.path.join(os.path.dirname(pyspark.__file__), "jars"))')
fi
classes=$(mktemp -d)
trap 'rm -rf "$classes"' EXIT
javac --release 17 -cp "$jars/*" -d "$classes" $(find "$here/src" -name '*.java')
jar cf "$out" -C "$classes" .
echo "$out"
