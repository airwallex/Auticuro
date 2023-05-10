---
sidebar_position: 3
---

# LockAccount

Lock an account with the given account ID. Pre-checks are performed to ensure that the account:
- available balance == 0
- no _pending_in_ or _pending_out_ transactions
- no reservations

If successful, the account is locked and no balance operations can be performed on the account until
it is unlocked. 

```protobuf
message LockAccountRequest {
  RequestHeader header = 1;
  string metadata = 2;
}

message LockAccountResponse {
  errorpb.Error error = 1;
  commonpb.ResponseHeader header = 2;
  LockAccountRequest request = 3;
  accountpb.AccountChange account_change = 4;
}
```