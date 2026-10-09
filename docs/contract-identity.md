# Contract identity

`contract`, `version`, and `owner` give the contract its name, version number, and data owner.

```yaml
contract: purchasing/orders
version: 1
owner: acme
```

| Field | Meaning |
| --- | --- |
| `contract` | Required name of the contract. |
| `version` | Required version number, written as a non-negative whole number. |
| `owner` | Optional name of the owner of the data. |

`purchasing/orders` names this contract. A separate contract for invoices could be named `finance/invoices`.

Changing `version: 1` to `version: 2` identifies a new version of the contract. Parcel does not compare versions to check compatibility.

`owner: acme` identifies Acme as the data owner.
