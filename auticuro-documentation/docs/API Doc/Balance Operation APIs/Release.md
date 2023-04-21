---
sidebar_position: 4
---

# Release
The Release allows releasing an amount of money within an account from the corresponding 
reservations.
Checks include:
- The account is in `Normal` state
- The account balance after the request handling is within the `[lower limit, upper limit]`
- The `reservation_id` must exist, and the reserved `amount` will be released from the existing reservation.
- If `amount` is not provided, release all the reserved money; otherwise, release the specified 
  amount from reserved to available.

```protobuf
message ReleaseRequest {
  string dedup_id = 1;
  string reservation_id = 2;
  string amount = 3;
  string account_id = 4;
  string metadata = 5;
}

message ReleaseResponse {
  errorpb.Error error = 1;
  commonpb.ResponseHeader header = 2;
  ReleaseRequest request = 3;
  accountpb.AccountChange account_change = 4;
}
```
