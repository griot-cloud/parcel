package cloud.griot.parcel.spark;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import java.io.IOException;
import java.nio.file.DirectoryStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.attribute.FileTime;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.apache.spark.sql.SparkSession;
import org.apache.spark.sql.catalyst.analysis.UnresolvedRelation;
import org.apache.spark.sql.catalyst.plans.logical.LogicalPlan;
import org.apache.spark.sql.catalyst.plans.logical.SubqueryAlias$;
import org.apache.spark.sql.catalyst.rules.Rule;
import scala.runtime.AbstractPartialFunction;

/**
 * Resolves a compiled contract's name to its governed query while Spark analyses a query.
 *
 * <p>Spark asks every resolution rule about the plan. This one replaces a relation Spark could not
 * resolve, whose name is a contract's ({@code <contract>@<version>}), by the contract's query
 * with the caller's fields filled in, then lets Spark resolve and optimise it like any other
 * subquery. Contracts are the {@code *.spark.json} files {@code parcel compile --target spark}
 * writes, in the directories {@code spark.parcel.contracts} lists (comma-separated; default
 * {@code contracts}). The caller is {@code spark.parcel.caller.<field>}: {@code tenant},
 * {@code purpose}, {@code other.region}.
 */
public final class ParcelResolution extends Rule<LogicalPlan> {
  private static final ObjectMapper JSON = new ObjectMapper();
  private static final Pattern TOKEN = Pattern.compile("__parcel_caller\\(([A-Za-z0-9_.]+)\\)__");
  private static final Map<Path, Loaded> CACHE = new ConcurrentHashMap<>();

  private final SparkSession session;

  public ParcelResolution(SparkSession session) {
    this.session = session;
  }

  /** A compiled contract's Spark artifact, read from disk. */
  private record Loaded(FileTime modified, JsonNode artifact) {}

  @Override
  public LogicalPlan apply(LogicalPlan plan) {
    return plan.resolveOperatorsUp(
        new AbstractPartialFunction<LogicalPlan, LogicalPlan>() {
          @Override
          public boolean isDefinedAt(LogicalPlan p) {
            return p instanceof UnresolvedRelation u && name(u).contains("@");
          }

          @Override
          public LogicalPlan apply(LogicalPlan p) {
            String name = name((UnresolvedRelation) p);
            JsonNode artifact = find(name);
            if (artifact == null) {
              return p; // not a contract: Spark reports the unknown table itself
            }
            String sql = bind(artifact);
            LogicalPlan query;
            try {
              query = session.sessionState().sqlParser().parsePlan(sql);
            } catch (Exception e) {
              throw new IllegalStateException(
                  "parcel: the governed query of " + name + " does not parse: " + e.getMessage(), e);
            }
            return SubqueryAlias$.MODULE$.apply(name, query);
          }
        });
  }

  private static String name(UnresolvedRelation u) {
    return String.join(
        ".", scala.jdk.javaapi.CollectionConverters.asJava(u.multipartIdentifier()));
  }

  private List<Path> directories() {
    String dirs = session.conf().get("spark.parcel.contracts", "contracts");
    List<Path> out = new ArrayList<>();
    for (String d : dirs.split(",")) {
      if (!d.isBlank()) {
        out.add(Paths.get(d.trim()));
      }
    }
    return out;
  }

  /** The artifact whose name is {@code name}, or null. */
  private JsonNode find(String name) {
    for (Path dir : directories()) {
      if (!Files.isDirectory(dir)) {
        continue;
      }
      try (DirectoryStream<Path> files = Files.newDirectoryStream(dir, "*.spark.json")) {
        for (Path f : files) {
          JsonNode a = load(f);
          if (a != null && name.equals(a.path("name").asText())) {
            return a;
          }
        }
      } catch (IOException e) {
        throw new IllegalStateException("parcel: cannot read " + dir + ": " + e.getMessage(), e);
      }
    }
    return null;
  }

  private static JsonNode load(Path f) throws IOException {
    FileTime modified = Files.getLastModifiedTime(f);
    Loaded cached = CACHE.get(f);
    if (cached != null && cached.modified().equals(modified)) {
      return cached.artifact();
    }
    JsonNode a = JSON.readTree(f.toFile());
    if (!"parcel-target/1".equals(a.path("format").asText())
        || !"spark".equals(a.path("engine").asText())) {
      return null;
    }
    CACHE.put(f, new Loaded(modified, a));
    return a;
  }

  /** The artifact's query with every caller token replaced by a typed literal. */
  private String bind(JsonNode artifact) {
    Map<String, String> types = new ConcurrentHashMap<>();
    for (JsonNode c : artifact.path("caller")) {
      types.put(c.path("name").asText(), c.path("type").asText());
    }
    Matcher m = TOKEN.matcher(artifact.path("query").asText());
    StringBuilder out = new StringBuilder();
    while (m.find()) {
      String field = m.group(1);
      String value = literal(field, types.getOrDefault(field, "string"));
      m.appendReplacement(out, Matcher.quoteReplacement(value));
    }
    m.appendTail(out);
    return out.toString();
  }

  /** Caller field {@code field} of parcel type {@code type}, as a Spark SQL literal. */
  private String literal(String field, String type) {
    String key = "spark.parcel.caller." + field;
    String v = session.conf().contains(key) ? session.conf().get(key) : null;
    if (v == null || v.isEmpty()) {
      return field.equals("now") ? "current_timestamp()" : "CAST(NULL AS " + sparkType(type) + ")";
    }
    switch (type) {
      case "int":
        return Long.toString(Long.parseLong(v.trim()));
      case "uint":
        return "CAST(" + new java.math.BigInteger(v.trim()) + " AS DECIMAL(20,0))";
      case "double":
        return "CAST(" + Double.parseDouble(v.trim()) + " AS DOUBLE)";
      case "bool":
        return Boolean.parseBoolean(v.trim()) ? "TRUE" : "FALSE";
      case "timestamp":
        return "TIMESTAMP " + quote(v.trim());
      case "list<string>":
        {
          List<String> items = new ArrayList<>();
          for (String s : v.split(",")) {
            items.add(quote(s.trim()));
          }
          return "array(" + String.join(", ", items) + ")";
        }
      default:
        return quote(v);
    }
  }

  private static String sparkType(String type) {
    switch (type) {
      case "int":
        return "BIGINT";
      case "uint":
        return "DECIMAL(20,0)";
      case "double":
        return "DOUBLE";
      case "bool":
        return "BOOLEAN";
      case "timestamp":
        return "TIMESTAMP";
      case "list<string>":
        return "ARRAY<STRING>";
      default:
        return "STRING";
    }
  }

  private static String quote(String s) {
    return "'" + s.replace("\\", "\\\\").replace("'", "\\'") + "'";
  }
}
