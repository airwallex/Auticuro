---
sidebar_position: 2
---

# Architecture
![image info](@site/static/img/design/CQRS_with_event_sourcing.svg)
Above is the high-level architecture of one Auticuro shard, which uses CQRS with Event Sourcing.

## Command Side
The command side of the Auticuro shard is written in Rust to achieve high performance and correctness
and uses Raft to achieve dependability under cloud environments. It processes requests for account 
management and balance operation, supports real-time balance checks in critical-path, and generates
event logs streamed to the query side in a real-time manner.

Auticuro leverages its single-threaded critical path and Copy-On-Write pattern to achieve all-or-nothing
semantics for a batch of operations. If any of the operations in a batch fails due to balance limitation
checks or improper account state, the preceding changes made by that batch on cloned accounts are discarded,
leaving the state unchanged.

## Query Side

The query side provides materialized views of the accounts by replaying event logs tailored to flexible
business requirements to maximize the query performance. These views are read-only caches of the events, 
while the event store is the golden source of truth. It is essential that the command side generates events 
with a consecutive sequence number, which can be used by query sides to perform integrity checks and 
detect missing or out-of-order events. When the query side system is damaged, its state can be 
restored by replaying all past account/balance change events, and snapshots are used to accelerate 
the process. Typical query side services are as follows:

**Account Hierarchy Service**

Our client could set up multiple accounts for a specific business requirement and organize them as
a tree-style hierarchy. Leaf accounts support modifications such as account management and balance 
operations, while non-leaf accounts are unmodifiable and illustrate an aggregated read-only view.

An Account Hierarchy Service is built to satisfy the above requirement:

* The Account Hierarchy Service applies events and calculates leaf accounts' balance 
* Users could CRUD account hierarchy configs via the UI, where CRUD is the acronym for CREATE, READ, 
UPDATE, and DELETE. 
* When receiving a query request, the Account Hierarchy Service reads the account hierarchy config 
from the database and calculates the non-leaf account's balance by aggregating the balance of
its children recursively.

**Account Query Service**

The Account Query Service provides the Read-Your-Write consistency query for account balances and
balance change history.  After you've updated the account, it is very natural that if you immediately 
read it back, you should read your last modification. This is called read-your-write consistency. 
It is desirable because it provides a more intuitive experience for users and can help ensure correct
application behavior.

* The Account Query Service applies events to build every version for every account.
* Each account instance contains a version number, facilitating account-wise pagination queries 
  for change history.
* The versioned accounts are replicated to a data warehouse like BigQuery(an enterprise data 
warehouse product of Google) for OLAP analysis.

**Kafka Connector**

The events are published to Kafka for downstream systems to subscribe from.
