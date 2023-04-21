---
sidebar_position: 5
---

# UpdateAccountConfig
Update the balance limit of the account with the given account ID. Below pre-check will be 
conducted:
- available balance >= lower limit
- available balance + sum{reservations} + sum{pending_in + pending_out}  <= upper limit

```protobuf
message UpdateAccountConfigRequest {
  RequestHeader header = 1;
  accountpb.BalanceLimit balance_limit = 2;
  string metadata = 3;
}

message UpdateAccountConfigResponse {
  errorpb.Error error = 1;
  commonpb.ResponseHeader header = 2;
  UpdateAccountConfigRequest request = 3;
  accountpb.AccountChange account_change = 4;
}
```