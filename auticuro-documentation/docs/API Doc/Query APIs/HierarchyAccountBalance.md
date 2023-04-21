---
sidebar_position: 2
---

# Hierarchy Account Balance

**`HierarchyAccountBalance(seq_num: u64, hierarchy_account_id: String, root_only: bool)`**
### Description

Account hierarchy organizes the accounts with parent-children relationships that form a tree structure. While Firm 
Wallet is only aware of and operates on leaf accounts, while non-leaf accounts provide an aggregated view of all its leaf 
children.

`HierarchyAccountBalance` takes a *seq_num* and *hierarchy_account_id* and returns the accumulated balance 
(**available balance only**) of the target account. If *root_only* is set to true, only the balance of input hierarchy account
is returned. Otherwise, balance of all accounts in the tree (with input account as root) are included.

For consistency, a non-zero *seq_num* must be specified.

### Definitions

Hierarchy Account balance response:

```protobuf3
message HierarchyAccountBalanceResponse {
  option Error error = 1;
  uint64 seq_num = 2;
  string root_account_id = 3;
  string root_account_balance = 4;
  repeated HierarchyAccountBalance non_root_account_balances = 5;
}

message HierarchyAccountBalance {
  string account_id = 1;
  option string paret_account_id = 2;
  string available_balance = 3;
```