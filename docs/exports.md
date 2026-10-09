# Validation exports

Validation plans can be exported as SQL or Substrait, or executed directly with DataFusion.

| Interface | Reference |
| --- | --- |
| Command line | [SQL and Substrait options](compiling.md#export-validation-as-sql) |
| Rust export functions | [Compiled artifacts and exports](rust-artifacts.md) |
| DataFusion execution | [Data validation](rust-validation.md) |

Exports check data quality. They do not apply caller access policies, masking, or result shapes to other queries in the receiving engine.
