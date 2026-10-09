# Quickstart

Check a contract over four orders, compare access for a company and one supplier, then compile it.

## Install parcel

On Linux or macOS:

```bash
curl -LsSf https://github.com/griot-cloud/parcel/releases/latest/download/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"
parcel --version
```

On Windows, in PowerShell:

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/griot-cloud/parcel/releases/latest/download/install.ps1 | iex"
```

Open a new terminal after installing on Windows, then run `parcel --version`. On Linux or macOS, add the same PATH setting to your shell profile if the directory is not already included.

You can also download an archive from [releases](https://github.com/griot-cloud/parcel/releases), or build from source with `cargo build --release -p parcel-cli`.

Get the example files:

```bash
git clone https://github.com/griot-cloud/parcel.git
cd parcel/examples/suppliers
```

Run the commands below from this directory.

## Data and contract

`orders.csv` contains orders assigned to two suppliers. Order 3 has an invalid amount:

```{literalinclude} ../examples/suppliers/orders.csv
:language: text
```

`orders.yaml` lets Acme read every supplier's orders, limits each supplier to its own orders, and excludes non-positive amounts:

```{literalinclude} ../examples/suppliers/orders.yaml
:language: yaml
```

The `binding` declares a Parquet location for the orders. For this check, `--data orders.csv` supplies the sample; no Parquet files are needed or written.

## Define two callers

`acme.yaml` represents a member of Acme's purchasing team:

```{literalinclude} ../examples/suppliers/acme.yaml
:language: yaml
```

`globex.yaml` represents a supplier analyst:

```{literalinclude} ../examples/suppliers/globex.yaml
:language: yaml
```

These profiles mock `ctx` for the check: `id` identifies the caller, `tenant` identifies their organization, and `purpose` states why they need access.

## Check the contract

```bash
parcel check orders.yaml --data orders.csv --caller acme.yaml --caller globex.yaml
```

The report shows:

- **One failed quality check:** order 3 fails `positive_amount`.
- **A valid dataset verdict:** `on_fail: drop` excludes the bad row from results without invalidating the whole dataset.
- **Different row counts:** 3 of 4 rows pass for Acme; 2 of 4 pass for Globex. The CSV stays unchanged.

The row counts include both the supplier access rule and the positive-amount check.

To require every amount to pass, change `on_fail: drop` to `on_fail: deny` and rerun the check. The verdict becomes invalid and the command exits unsuccessfully. Restore `drop` before continuing.

## Compile it

```bash
parcel compile orders.yaml --schema orders.csv -o orders.parcel
```

`--schema orders.csv` supplies the column names and types. Compilation checks the rules against them and writes `orders.parcel`. Checking the data values is the job of `parcel check`.

For the contract fields and more examples, see {doc}`authoring`. Command options and output are covered in {doc}`checking` and {doc}`compiling`.
