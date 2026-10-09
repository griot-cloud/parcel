# Expose

`expose` lists the columns the owner chooses to make available, along with their data types.

For an orders dataset with these columns:

| order_id | supplier_id | amount |
| --- | --- | --- |
| 1 | globex | 120 |
| 2 | initech | 80 |

The owner can expose only `order_id` and `amount`:

```yaml
expose:
  - {name: order_id, type: int64}
  - {name: amount, type: int64}
```

`supplier_id` is left out of the output. It remains available to rules that need to identify the supplier for each order.

## Column names and types

Each entry has a `name` matching a column in the data and a `type` describing its values. For a dataset containing order identifiers, status text, and a delivery flag:

```yaml
expose:
  - {name: order_id, type: int64}
  - {name: status, type: utf8}
  - {name: delivered, type: bool}
```

| Type | Values |
| --- | --- |
| `int64` | Whole numbers, such as `120`. |
| `utf8` | Text, such as `shipped`. |
| `bool` | `true` or `false`. |
| `float64` | Floating-point numbers, such as `12.5`. |

Parcel checks that the columns exist, their names are unique, and their types match. If a rule replaces a column's value, its output must match the type declared here. For more details, see [Column types](language.md#column-types).

`expose` is required unless inherited from a parent contract. See [Contract inheritance](language.md#inheritance).
