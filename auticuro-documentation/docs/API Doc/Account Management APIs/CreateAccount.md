---
sidebar_position: 1
---

# CreateAccount
Create an account with the given account config. The request fails if there is already an account
with the same account ID. If successful, the account will be created and returned in the response.

```protobuf
message CreateAccountRequest {
  RequestHeader header = 1;
  accountpb.AccountConfig account_config = 2;
  string metadata = 3;
}

message CreateAccountResponse {
  errorpb.Error error = 1;
  commonpb.ResponseHeader header = 2;
  CreateAccountRequest request = 3;
  accountpb.Account account = 4;
}
```