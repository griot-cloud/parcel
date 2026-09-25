# parcel

<div class="parcel-hero">
<p>Data contracts in <a href="https://cel.dev">CEL</a>, compiled into query plans. A contract
says who may use a dataset, which rows and columns they see, how values are masked, and what the
data must satisfy. parcel checks it against the data's schema and compiles it into expressions
and plans any Arrow engine can run.</p>
</div>

```yaml
contract: sales/orders
version: 1
owner: acme
binding: {parquet: data/orders/, partitioned_by: [region]}
expose:
  - {name: order_id, type: int64}
  - {name: email, type: utf8}
  - {name: region, type: utf8}
  - {name: amount_cents, type: int64}
rules:
  - {id: analytics_only, op: decide, expr: "ctx.purpose in ['analytics', 'reporting']"}
  - {id: own_or_admin, op: admit, expr: "row.tenant_id == ctx.tenant || 'admin' in ctx.roles"}
  - {id: pk_present, op: assert, expr: "has(row.order_id)", on_fail: deny}
  - {id: mask_email, op: transform, column: email, expr: "ctx.tenant == 'acme' ? row.email : hash_sha256(row.email)"}
  - {id: ids_present, op: guarantee, expr: "dataset.customer_id.null_rate < 0.02", on_fail: deny}
  - {id: small_cells, op: shape, operator: suppress, params: {k: 5}, unless: "ctx.tenant == 'acme'"}
```

::::{grid} 1 2 2 2
:gutter: 3

:::{grid-item-card} Quickstart
:link: getting-started
:link-type: doc

Check a contract against sample data and compile it.
:::

:::{grid-item-card} The contract language
:link: language
:link-type: doc

Operations, namespaces, types, functions, and nulls.
:::

:::{grid-item-card} How rules execute
:link: concepts
:link-type: doc

What each rule costs, and the three artifacts parcel produces.
:::

:::{grid-item-card} parcel and peQL
:link: parcel-and-peql
:link-type: doc

The language and the runtime, and the bundle between them.
:::
::::

```{toctree}
:maxdepth: 1
:caption: Learn
:hidden:

getting-started
```

```{toctree}
:maxdepth: 2
:caption: How-to
:hidden:

functions
exports
odcs
```

```{toctree}
:maxdepth: 2
:caption: Reference
:hidden:

language
cli
crates
```

```{toctree}
:maxdepth: 2
:caption: Explanation
:hidden:

concepts
parcel-and-peql
```

```{toctree}
:maxdepth: 1
:caption: Contribute
:hidden:

contributing
```
