# parcel

parcel is a data contract language and compiler that lets you define data quality rules and data access policies, test them against sample data, and compile them for a query engine such as peQL to enforce.

## Use cases

- **Define how partners can use data.** Describe which orders each supplier can see, which columns they can query, and which values need masking.
- **Check data before it reaches a report.** Require valid amounts, complete identifiers or fresh data, with explicit actions for failed checks.
- **Test policy changes before using them.** Check sample data and caller profiles to catch invalid expressions and inspect access.
- **Use the same checks in another engine.** Export data validation as SQL or Substrait, or embed parcel's Rust libraries in a DataFusion application.

## Documentation

[Read the documentation online](https://griot-cloud.github.io/parcel/).

| Section | Start here for |
| --- | --- |
| [Getting started](docs/getting-started.md) | Concepts and a working example with four orders and two callers. |
| [Writing contracts](docs/authoring.md) | Rules, testing, inheritance, imports and custom functions. |
| [How it works](docs/execution.md) | Compilation, bundles and what an engine enforces. |
| [Reference](docs/reference.md) | Contract fields, expressions, commands and Rust libraries. |

Contracts use YAML or JSON with CEL expressions for their rules. parcel checks and compiles them; [peQL](https://griot-cloud.github.io/peQL/) manages data and enforces them on SQL queries.

**[Get started →](docs/getting-started.md)**

[Releases](https://github.com/griot-cloud/parcel/releases) · [Contributing](docs/contributing.md) · [Apache-2.0 license](LICENSE)
