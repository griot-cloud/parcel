# Contract identity

`contract`, `version`, and `owner` give the contract its name, version number, and data owner.

```yaml
contract: purchasing/orders
version: 1.0.0
owner: acme
```

| Field | Meaning |
| --- | --- |
| `contract` | Required name of the contract. |
| `version` | Required semantic version, `MAJOR.MINOR.PATCH`, e.g. `1.0.0` or `2.1.0-beta.1`. |
| `owner` | Optional name of the owner of the data. |

`purchasing/orders` names this contract. A separate contract for invoices could be named `finance/invoices`.

Changing `version: 1.0.0` to `version: 1.1.0` or `version: 2.0.0` identifies a new version of the contract. Parcel does not compare versions to check compatibility.

A whole number is read as a major version: `version: 2` is `2.0.0`. A decimal is refused, because YAML reads `1.10` as the number 1.1; write `1.10.0`.

`owner: acme` identifies Acme as the data owner.
