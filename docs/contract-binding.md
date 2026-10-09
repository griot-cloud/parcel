# Binding

`binding` specifies where the data is stored. parcel supports a Parquet file or directory, or an Iceberg table. It is required unless the contract inherits it from a parent contract. See [Contract inheritance](language.md#inheritance).

## Parquet file or directory

```yaml
contract: purchasing/orders
version: 1
binding:
  parquet: data/orders/
```

`parquet` accepts a file path, directory path, or URL. In this example, `data/orders/` contains the order files. A single file can be specified as `parquet: data/orders-2026-10.parquet`.

## Iceberg table

```yaml
contract: finance/invoices
version: 2
binding:
  iceberg: finance.invoices
```

An Iceberg table is identified as `namespace.table`. In this example, `finance` is the namespace—a group of tables—and `invoices` is the table containing the data.

## Partition columns

Data can be stored in separate groups by values such as region or month. `partitioned_by` lists the columns used for those groups:

```yaml
binding:
  iceberg: finance.invoices
  partitioned_by: [region, invoice_month]
```

Here, invoices are stored in separate groups by region and month. `partitioned_by` records those columns in the contract. It applies to both Parquet and Iceberg data.
