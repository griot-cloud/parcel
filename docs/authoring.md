# Writing contracts

A parcel contract is a YAML or JSON file that describes a dataset and the rules for using it. For example, Acme owns the following orders dataset:

| order_id | supplier_id | amount |
| --- | --- | --- |
| 1 | globex | 120 |
| 2 | initech | 80 |
| 3 | globex | -5 |
| 4 | globex | 60 |

We want to create a contract that declares data quality rules and access policies for these orders. Each supplier should have access only to its own orders, and orders with zero or negative amounts should be excluded. The owner wants to make only `order_id` and `amount` available.

The contract has four sections: contract identity, binding, expose, and rules.

## Contract identity

`contract` is the name of the contract, `version` is its version number, and `owner` is the owner of the data. For more details, see {doc}`Contract identity <contract-identity>`.

```yaml
contract: purchasing/orders
version: 1
owner: acme
```

## Binding

`binding` specifies where the data is stored. For this example, the orders above are stored as Parquet files in `data/orders/`. For more details, see {doc}`Binding <contract-binding>`.

```yaml
binding:
  parquet: data/orders/
```

## Expose

`expose` lists the columns the owner chooses to make available, along with their data types. Of the three columns in the dataset above, the owner exposes `order_id` and `amount`. The rules can still use `supplier_id` to match each order to a supplier. For more details, see {doc}`Expose <contract-expose>`.

```yaml
expose:
  - {name: order_id, type: int64}
  - {name: amount, type: int64}
```

## Rules

`rules` lets the owner define data quality rules and access policies for the dataset. When sharing these orders with suppliers' analysts, we want to:

1. Allow each supplier to see only its own orders.
2. Return only orders with positive amounts.

The `supplier_orders` rule compares each order's `supplier_id` with the analyst's organization, supplied as `ctx.tenant`. The `positive_amount` rule excludes orders with zero or negative amounts. For more details, see {doc}`Rules <contract-rules>`.

```yaml
rules:
  - id: supplier_orders
    op: admit
    expr: "row.supplier_id == ctx.tenant"
  - id: positive_amount
    op: assert
    expr: "row.amount > 0"
    on_fail: drop
```

## Complete contract

```yaml
contract: purchasing/orders
version: 1
owner: acme
binding:
  parquet: data/orders/
expose:
  - {name: order_id, type: int64}
  - {name: amount, type: int64}
rules:
  - id: supplier_orders
    op: admit
    expr: "row.supplier_id == ctx.tenant"
  - id: positive_amount
    op: assert
    expr: "row.amount > 0"
    on_fail: drop
```

```{toctree}
:hidden:
:maxdepth: 1

contract-identity
contract-binding
contract-expose
contract-rules
```
