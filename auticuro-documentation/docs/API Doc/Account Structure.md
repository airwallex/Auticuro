---
sidebar_position: 0
---

### The Account Structure
An account is divided into four parts: TransactionSection, Available, ReservedSection, and Configuration.

![image info](@site/static/img/api/account_structure.svg)

TransactionSection is a mapping from transactionId to its PendingIn amount and PendingOut amount 
manipulated by the TCC interfaces.

ReservedSection is a mapping from reservationId to its reserved amount, which supports Reserve, 
IncrementalReserve, Release, and PartialRelease interface.

The Reserve interface allows reserving an amount of money within an account for future usage 
purposes by moving the money from the available section to the reserved section. The only permitted 
operation on that reserved money is the Release interface which moves the money back to the available 
section. IncrementalReserve and PartialRelease are just variants of Reserve and Release.

The Configuration contains fields like UpperLimit, LowerLimit, State, Currency, and Version, which 
could be updated by UpdateUpperLimit, UpdateLowerLimit, LockAccount, and UnlockAccount interface.

The Transfer interface is a bilateral money movement between two accounts inside the same Auticuro 
shard. BatchBalanceOperation is a batch of money movement for accounts inside the same Auticuro 
shard, and each money movement credits or debits an amount from one account.

Updating the account follows a Copy-On-Write pattern. Instead of directly updating the account,
each modification will be applied to a copy of the account. Each time the copy-on-write happens,
we will assign an increased version number to it. so we know the entire history of the account modification.

### The account protobuf description:
- **AccountConfig**: The configuration settings for the account.
- **Balance**: The current balance of the account, including available funds, reservations, and
  pending transactions.
- **AccountState**: The current state of the account, such as normal, locked, or deleted.
- **account_id**: A unique identifier for the account.
- **version_number**: A monotonically increasing version number that increments with each change to
  the account.

```protobuf
message Account {
  AccountConfig config = 1;
  Balance balance = 2;
  AccountState state = 3;
  string account_id = 4;
  uint64 version_number = 5; // monotonically increasing, increased by 1 upon any account changes
}

message AccountConfig {
  BalanceLimit balance_limit = 1;
  AssetClass asset_class = 2;
}

message BalanceLimit {
  string upper = 1;
  string lower = 2;
}

message AssetClass {
  oneof asset_class {
    Cash cash = 1;
  }

  message Cash {
    string currency = 1;
  }
}

enum AccountState {
  InvalidState = 0;
  Deleted = 1;
  Locked = 2;
  Normal = 3;
}
```

### Balance Structure
The **Balance** message contains information about the account's balances, including:
- **available**: The available balance for general use.
- **reservations**: A map of reservation IDs to reserved amounts, representing funds set aside for 
  specific purposes.
- **pending_in**: A map of transaction IDs to pending incoming amounts, representing funds expected 
  to be credited to the account.
- **pending_out**: A map of transaction IDs to pending outgoing amounts, representing funds expected 
  to be debited from the account.

```protobuf
message Balance {
  string available = 1;
  map<string, string> reservations = 2; // reservation_id -> reservation amount

  map<string, string> pending_in = 3; // txn_id -> pending_in amount
  map<string, string> pending_out = 4; // txn_id -> pending_out amount
}
```