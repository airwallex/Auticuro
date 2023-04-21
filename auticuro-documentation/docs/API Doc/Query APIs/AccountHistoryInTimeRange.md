---
sidebar_position: 4
---

# Account History In Time Range

**`AccountHistoryInTimeRange(account_id: String, start_timestamp: Timestamp, end_timestamp: Timestamp)`**

### Description

Besides in the range of seq_num, account history could also be retrieved in the range of time.

Given an *account_id* and time range, `AccountHistoryInTimeRange` retrieves the history of account change happened 
during **[start_timestamp, end_timestamp)**. Timestamp support human-readable time with a timezone and Unix Epoch. 

### Definitions

Account history in time range response:

```protobuf3
message AccountHistoryInTimeRangeResponse {
  option Error error = 1;
  
  Timestamp start_time = 2;
  Timestamp end_time = 3;
  
  account_id = 4;
  map<uint64, Account> account_history = 5;  // seq_num -> Account
}

message Account {
  AccountConfig config = 1;
  Balance balance = 2;
  AccountState state = 3;
  uint64 version_number = 5;
}