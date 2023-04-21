---
sidebar_position: 3
---

# Reserve
The Reserve allows reserving an amount of money within an account for future usage purposes.
Checks include:
- The account is in `Normal` state
- The account balance after the request handling is within the `[lower limit, upper limit]`
- If the `reservation_id` exists, the reserved `amount` will be added to the existing reservation,
    otherwise a new reservation will be created.

```protobuf
message ReserveRequest {
  string dedup_id = 1;
  string reservation_id = 2;
  string account_id = 3;
  string amount = 4;
  string metadata = 5;
}

message ReserveResponse {
  errorpb.Error error = 1;
  commonpb.ResponseHeader header = 2;
  ReserveRequest request = 3;
  accountpb.AccountChange account_change = 4;
}
```