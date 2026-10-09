# parcel

parcel is a data contract language and compiler for defining data quality rules and access policies, testing them against sample data, and compiling them for use in applications.

## Use cases

- **Define how partners can use data.** Specify which orders each supplier may access, which columns are available, and which values need masking.
- **Check data before it reaches a report.** Require valid amounts, complete identifiers or fresh data, with explicit actions for failed checks.
- **Test policy changes before using them.** Check sample data and caller profiles to catch invalid expressions and inspect access.
- **Use the same checks in another engine.** Export data validation as SQL or Substrait, or use parcel's Rust libraries in an application.

## Documentation

[Read the documentation online](https://griot-cloud.github.io/parcel/).

| Section | Contents |
| --- | --- |
| [Getting started](docs/getting-started.md) | Installation and a working example with four orders and two callers. |
| [Writing contracts](docs/authoring.md) | Contract identity, binding, exposed columns, namespaces, and rules. |
| [Checking and compiling contracts](docs/execution.md) | Data validation, caller checks, reports, and compiled contracts. |
| [Reference](docs/reference.md) | Contract fields, expressions, commands and Rust libraries. |

For the SQL integration, see [parcel and peQL](docs/parcel-and-peql.md).

**[Get started →](docs/getting-started.md)**

[Releases](https://github.com/griot-cloud/parcel/releases) · [Contributing](docs/contributing.md) · [Apache-2.0 license](LICENSE)
