---
sidebar_position: 3
---

# Account History In Seq Range

**`AccountHistoryInSeqRange(account_id: String, start_seq_num: u64, end_seq_num: u64)`**

### Description

Auticuro preserves all the change history for a given account, including balances and state, and configuration.

Given an *account_id* and seq_num range, `AccountHistoryInSeqRange` retrieves the history of account change at every
seq_num within _[start_seq_num, end_seq_num)_. In case that input seq_num range out competes what service can provide, only
available account history is partially returned.

### Definitions

Account history in seq range response:

```protobuf3
message AccountHistoryInSeqRangeResponse {
  option Error error = 1;
  
  uint64 start_seq_num = 2;
  uint64 end_seq_num = 3;
  
  account_id = 4;
  map<uint64, Account> account_history = 5;  // seq_num -> Account
}

message Account {
  AccountConfig config = 1;
  Balance balance = 2;
  AccountState state = 3;
  uint64 version_number = 5;
}
```