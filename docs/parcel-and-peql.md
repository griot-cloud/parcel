# parcel and peQL

parcel defines and compiles data contracts. [peQL](https://griot-cloud.github.io/peQL/) manages data and applies those contracts to SQL queries.

| Task | Where it belongs |
| --- | --- |
| Define contract syntax and check expressions. | parcel compiler. |
| Produce row filters, projections, validation and write plans. | parcel compiler. |
| Bind caller values, evaluate rules and verify bundles. | parcel runtime libraries. |
| Store registered contracts and control their visibility. | peQL. |
| Write data, maintain statistics and validation results. | peQL. |
| Apply contract rules to SQL queries and manage audit records and budgets. | peQL. |

peQL calls parcel's Rust libraries directly. You can register a contract document in peQL without installing the parcel CLI first.

Use the parcel CLI when authoring and testing contracts separately from the engine. `parcel compile -o contract.parcel.json` produces a bundle that a compatible peQL version can register. The receiver recompiles and verifies its contents before using them.

For a complete data-and-query workflow, start with [peQL's quickstart](https://griot-cloud.github.io/peQL/quickstart.html). For contract syntax and rule expressions, stay in the {doc}`language` reference.
