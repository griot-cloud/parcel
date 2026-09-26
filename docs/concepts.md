# Concepts

parcel lets you define a **data contract**: the data quality rules and access policies for a dataset. A contract also identifies the data and lists the columns available to callers.

## What is a data contract?

Acme wants to share orders with its suppliers. Acme staff should see all orders, while each supplier should see only its own. Orders with zero or negative amounts should be excluded from everyone's results.

```{literalinclude} ../examples/suppliers/orders.yaml
:language: yaml
```

The source data has `order_id`, `supplier_id` and `amount` columns. This contract exposes only `order_id` and `amount`. The `supplier_id` column is available to the rules but hidden from queries.

The `supplier_orders` rule allows Acme to see all orders and suppliers to see their assigned orders. The `positive_amount` rule excludes non-positive amounts from both groups. Owning a contract does not automatically bypass its rules: Acme's access comes from the explicit condition in `supplier_orders`.

## Contract fields

A contract is a YAML or JSON document. Its main fields are:

| Field | Meaning in the example |
| --- | --- |
| `contract` and `version` | The name `purchasing/orders` and version `1`. |
| `owner` | The organisation that owns the contract, `acme`. |
| `binding` | Where an engine can find the data. |
| `expose` | The columns and types a caller may query. |
| `rules` | The checks and policies the engine must apply. |

The schema of the **source data** and the schema in **expose** can differ. Rules may read source columns that callers cannot select, as `supplier_id` demonstrates.

## Rules and expressions

A **rule** has a name (`id`), an operation (`op`) and the expression or settings for that operation. `admit` determines which rows a caller may read; `assert` checks whether each row meets a quality requirement.

Expressions use a supported subset of **CEL**, the Common Expression Language: a lightweight language for conditions and calculations. For example, `row.amount > 0` returns true or false. `||` means “or”, so the supplier policy reads “the caller is Acme, or this order belongs to the caller's tenant”.

parcel checks expressions against column names and types before compiling them. An unknown column, unsupported expression or invalid use of caller information produces an error.

## Namespaces

A **namespace** groups the values an expression can read. parcel has three:

| Namespace | Contains | Example |
| --- | --- | --- |
| `row` | Values from one record. | `row.amount > 0` |
| `ctx` | The caller's identity and attributes. | `ctx.tenant == 'acme'` |
| `dataset` | Statistics about the whole dataset. | `dataset.row_count > 0` |

A **caller** is a user, agent or service. Its **tenant** identifies an organisation or group, such as Acme or Globex. The application authenticates the caller and supplies these attributes; parcel evaluates them.

Different operations permit different namespaces. An access policy can depend on the caller, but a data quality assertion must describe the data itself. The {doc}`language` reference lists these restrictions and the other operations.

## Checking, compiling and enforcing

**Checking** runs a contract against sample data. It reports quality failures, tests caller access and compares rule results from a CEL interpreter with those from DataFusion, the SQL engine parcel compiles for.

**Compiling** produces expressions and plans an engine can execute. A **bundle** packages these results, the contract, its schema and any custom functions in one file.

**Enforcement** happens in the application or query engine using those results. [peQL](https://griot-cloud.github.io/peQL/) uses parcel to validate data and apply contracts to SQL queries. Running `parcel check` alone does not change or protect the source files.

Continue to the {doc}`quickstart` to try the supplier example.
