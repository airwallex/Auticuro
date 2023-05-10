---
sidebar_position: 2
---

# DeleteAccount
Delete an account with the given account ID. The request fails if there is no account with the same
account ID. If successful, the account is deleted and the response contains the account change.

Pre-checks:
- available balance == 0
- no _pending_in_ or _pending_out_ transactions
- no reservations

```protobuf
message DeleteAccountRequest {
  RequestHeader header = 1;
  string metadata = 2;
}

message DeleteAccountResponse {
  errorpb.Error error = 1;
  commonpb.ResponseHeader header = 2;
  DeleteAccountRequest request = 3;
  accountpb.AccountChange account_change = 4;
}
```
