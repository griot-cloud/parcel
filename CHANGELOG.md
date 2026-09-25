# Changelog

A version reaches users when it lands on `main` untagged: CI builds, tests and publishes it,
then tags it. See [CONTRIBUTING](docs/contributing.md#releasing).

## [0.0.1]: first public release

- The contract language: seven operations (`decide`, `expose`, `admit`, `assert`, `transform`,
  `guarantee`, `shape`) over three namespaces (`ctx`, `dataset`, `row`), checked against the
  data's Arrow schema.
- The compiler: CompiledContract, ValidationPlan and WritePlan sharing one hash, and the
  `parcel-bundle/1` artifact an engine verifies by recompiling.
- Shapes: small-cell suppression, sampling, and Laplace noise at the row or the aggregate.
- Exports: SQL for seven dialects, Substrait, and ODCS v3 import.
- Tenant functions as WebAssembly modules, pinned by hash.
- The `parcel` command: `check`, `compile`, `schema`, `import odcs`, `function verify`.
- Installers for Linux, macOS and Windows, and `pip install griot-parcel`.
