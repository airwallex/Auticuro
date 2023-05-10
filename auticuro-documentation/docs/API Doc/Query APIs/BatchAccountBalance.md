---
sidebar_position: 1
---

# Batch Account Balance

**`BatchAccountBalance(seq_num: u64, account_ids: Vec<String>)`**

### Description

`BatchAccountBalance` is a batch API for `AccountBalance` that takes a list of account ids and returns their balances.
For this batch API, a non-zero *seq_num* must be specified to return a snapshot view of all accounts at the same _seq_num_.

In case of the input account ids are partially found in the system, the unavailable accounts are specified in response. 

### Definitions

Batch account balance response:
```protobuf3
message BatchAccountBalanceResponse {
  option Error error = 1;
  uint64 seq_num = 2;
  map<string, Balance> balances = 3;
  repeated String unavailable_accounts = 4;
}

message Balance {
  string available = 1;
  map<string, string> reservations = 2; // reservation_id -> reservation amount
  map<string, string> pending_in = 3;   // txn_id -> amount
  map<string, string> pending_out = 4;

}