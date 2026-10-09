# parcel and peQL

parcel defines and compiles data contracts. [peQL](https://griot-cloud.github.io/peQL/) manages data and applies those contracts to SQL queries.

| Task | Where it belongs |
| --- | --- |
| Define contract syntax and check expressions. | parcel compiler. |
| Produce row filters, projections, validation and write plans. | parcel compiler. |
| Bind caller values, evaluate rules and load compiled contracts. | parcel runtime libraries. |
| Store registered contracts and control their visibility. | peQL. |
| Write data, maintain statistics and validation results. | peQL. |
| Apply contract rules to SQL queries and manage audit records and budgets. | peQL. |

peQL calls parcel's Rust libraries directly. Registering a contract document in peQL does not require the parcel CLI.

The parcel CLI supports authoring and testing contracts separately from the engine. `parcel compile -o contract.parcel` writes the compiled contract as bytes, which a compatible peQL version registers as given (`Engine::register_compiled`) without compiling it.

[peQL's quickstart](https://griot-cloud.github.io/peQL/quickstart.html) covers the data-and-query workflow. The {doc}`language` reference specifies contract syntax and rule expressions.
