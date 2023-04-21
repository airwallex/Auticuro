---
sidebar_position: 1
---

# Transfer
Transfer money from the `from` account to the `to` account. Checks will be conducted for both
the `from` and `to` account. 
  - The account is in `Normal` state
  - The account balance after the transfer is within the `[lower limit, upper limit]` 
  - The account currency is the same as the currency in the transfer request

### Usage Scenario
- **Basic Usage**:
Bilateral money transfer from one account to another account.

- **Advanced Usage**:
CAS-style check on balance is provided to implement an optimistic lock.
If setting fields expected_from_balance / expected_to_balance for pre-checks, the transfer 
fails if any of the pre-checks fails.

`expected_from_balance`: If set, the balance of the from_account before transfer MUST == the 
  expected_from_balance.

`expected_to_balance`: If set, the balance of the to_account before transfer MUST == the 
  expected_to_balance

```protobuf
message TransferRequest {
  string dedup_id = 1;
  TransferSpec transfer_spec = 2;
  string context = 3;

  message TransferSpec {
    string amount = 1;
    string from_account_id = 2;
    string to_account_id = 3;
    string currency = 4;
    string metadata = 5;

    string expected_from_balance = 6;
    string expected_to_balance = 7;
  }
}

message TransferResponse {
  errorpb.Error error = 1;
  commonpb.ResponseHeader header = 2;
  TransferRequest request = 3;
  accountpb.AccountChange from_account_change = 4;
  accountpb.AccountChange to_account_change = 5;
}
```
