---
sidebar_position: 2
---

# BatchBalanceOperation 
Increase/decrease the balance of multiple accounts in a single request. Checks are performed 
for each account:
- The account is in `Normal` state
- The account balance after the request handling is within the `[lower limit, upper limit]`
- The account currency is the same as the currency in the request

### Usage Scenario
- Basic Usage:
A list of unilateral money increasing/decreasing operations

- Advanced Usage:
Set the `Precondition preconditions` to implement an optimistic lock. All the `preconditions` 
will be checked before the request handling. If any of the preconditions fails, the request will
fail. 
The `BalanceCheck` precondition is provided to check: the balance of an account == expected_balance

```protobuf
message BatchBalanceOperationRequest {
  string dedup_id = 1;
  repeated BalanceOperationSpec balance_operation_specs = 2;
  string context = 3;
  repeated Precondition preconditions = 4; // Optional

  // Provide flexible support for balance operations happened among more than 2 accounts
  message BalanceOperationSpec {
    string account_id = 1;
    string amount = 2;
    string currency = 3;
    string metadata = 4;
  }
}

message Precondition {
  oneof precondition {
    BalanceCheck balance_check = 1;
  }

  message BalanceCheck {
    string account_id = 1;
    string expected_balance = 2;
  }
}

message BatchBalanceOperationResponse {
  errorpb.Error error = 1;
  commonpb.ResponseHeader header = 2;
  BatchBalanceOperationRequest request = 3;
  repeated accountpb.AccountChange account_changes = 4;
}
```