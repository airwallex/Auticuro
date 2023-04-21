---
sidebar_position: 4
---

# UnlockAccount
Unlock the account with the given account ID. If successful, the account is unlocked and can handle
balance operations again.

Pre-checks:
- The account is in `Locked` state

```protobuf
message UnlockAccountRequest {
  RequestHeader header = 1;
  string metadata = 2;
}

message UnlockAccountResponse {
  errorpb.Error error = 1;
  commonpb.ResponseHeader header = 2;
  UnlockAccountRequest request = 3;
  accountpb.AccountChange account_change = 4;
}
```
