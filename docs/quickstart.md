# Quickstart

Check a contract over four orders, compare access for a company and one supplier, then compile it into a bundle.

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

## Meet the data and contract

`orders.csv` contains orders assigned to two suppliers. Order 3 has an invalid amount:

```{literalinclude} ../examples/suppliers/orders.csv
:language: text
```

`orders.yaml` lets Acme read every supplier's orders, limits each supplier to its own orders, and excludes non-positive amounts:

```{literalinclude} ../examples/suppliers/orders.yaml
:language: yaml
```

The `binding` is the Parquet location a query engine will use later. For this check, `--data orders.csv` supplies the sample instead; no Parquet files are needed or written.

## Define two callers

`acme.yaml` represents a member of Acme's purchasing team:

```{literalinclude} ../examples/suppliers/acme.yaml
:language: yaml
```

`globex.yaml` represents a supplier analyst:

```{literalinclude} ../examples/suppliers/globex.yaml
:language: yaml
```

These files supply test identities. An application must supply authenticated caller attributes when enforcing the contract.

## Check the contract

```bash
parcel check orders.yaml --data orders.csv --caller acme.yaml --caller globex.yaml
```

The report shows:

- **One failed quality check:** order 3 fails `positive_amount`.
- **A valid dataset verdict:** `on_fail: drop` excludes the bad row from results without invalidating the whole dataset.
- **Different row counts:** Acme can read 3 of 4 rows; Globex can read 2 of 4. The underlying CSV stays unchanged.
- **Agreement between evaluators:** the CEL interpreter and DataFusion produce the same rule results for this sample and these callers.

The caller counts include row access rules and drop-level assertions. They are a check of those filters, not the result of an arbitrary SQL query.

To require every amount to pass, change `on_fail: drop` to `on_fail: deny` and rerun the check. The verdict becomes invalid and the command exits unsuccessfully. Restore `drop` before continuing.

## Compile a bundle

```bash
parcel compile orders.yaml --schema orders.csv -o orders.parcel.json
```

The schema tells parcel which columns and types the expressions must work with. Compilation checks the rules but does not validate the data values. The output file contains the contract, source schema and compiled expressions and plans.

Use the bundle with a compatible engine such as [peQL](https://griot-cloud.github.io/peQL/quickstart.html), which manages data and applies the rules to queries. To keep writing contracts, continue to {doc}`authoring`.
