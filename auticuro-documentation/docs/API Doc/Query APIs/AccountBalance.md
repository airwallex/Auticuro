---
sidebar_position: 0
---

# Account Balance

**`AccountBalnce(seq_num: u64, account_id: string)`**

### Description

Given *<seq_num, account_id>*, `AccountBalance` returns balances for the account_id at input seq_num. Returned 
balances consist of available, reservation, pending_in, and pending_out.

*seq_num* is used for read-after-write consistency guarantee. If the query side's last seen seq_num is smaller than the input one,
an error message of **NotAvailable** is returned. If input *seq_num* is 0,  the latest balance is returned without seq_num
check.

### Definitions

Account balance response:

```protobuf3
message AccountBalanceResponse {
  option Error error = 1;
  string account_id = 2;
  uint64 seq_num = 3;
  Balance balance = 4;
}

message Balance {
  string available = 1;
  map<string, string> reservations = 2; // reservation_id -> reservation amount
  map<string, string> pending_in = 3;   // txn_id -> amount
  map<string, string> pending_out = 4;

}
```

