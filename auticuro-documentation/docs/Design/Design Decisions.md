---
sidebar_position: 1
---

# Design Decisions
Auticuro can serve as the cornerstone for mission-critical financial applications. To further illustrate this point, the following high-level architecture showcases how Auticuro can be utilized to build a distributed wallet service and highlights its role within the overall system's ecosystem. In order to provide a comprehensive understanding of the architecture, we will also outline the most significant design decisions.

## Architecture of A Distributed Wallet Service
As depicted in the following diagram, the distributed wallet service is divided into three layers:

![image info](@site/static/img/design/High_level_architecture.svg)

- **Access Layer** that translates incoming flexible business requests into underlying stationary 
account operations,
- **Transaction Layer**, a.k.a. Marker, that orchestrates cross-shard money movements with ACID 
  guarantees, where ACID refers to the four key properties of a transaction: atomicity, consistency, isolation, and durability.
- **Storage Layer**, a.k.a. Auticuro, that supports low-level, high-performance atomic account 
  operations within a single shard.

Shards within the same layer do not communicate, while shards from different layers are fully connected. For example, Replicas in Access Layer can talk with any shard from the Transaction Layer or Storage Layer, and shards in the Transaction Layer can talk with any shard from Storage Layer.

## Decouple Transaction Layer and Storage Layer

The most important design decision we made was to decouple the transaction layer and storage layer. The decoupling dramatically decreases the engineering complexity and maintenance overhead while increasing the performance of transaction management and account operations.

The transaction layer is computation-intensive, while the storage layer is data-intensive. The transaction layer should be scaled up and down according to the traffic volume, while the storage layer should be scaled up according to the number of accounts. A new shard in the transaction layer can be brought up when encountering traffic spikes and decommissioned as long as it has no ongoing transactions.

A shard in the storage layer with a naive single-threaded implementation could easily achieve more than 20,000 account operations per second. In contrast, a carefully designed and implemented shard in the transaction layer can schedule 2,000 transactions per second.

According to our prior experience, a monolithic solution is hard to implement and evolve and much slower than the decoupled solution since it needs to handle quite a few complex situations relating to the pending conditional transfer due to the coupling of transaction management and account operation. It is also hard to smoothly scale up and down when encountering traffic spikes since it needs to repartition the accounts while scaling up and repartition again while scaling down, which is quite a burden for maintenance.


## Decouple Flex and Stationary

The wallet service separates the fundamental balance operation APIs from the business-facing APIs, the former is implemented in the storage layer, and the latter is implemented in the access layer. It offers several benefits:



* Independent Scalability: Decoupling the layers allows the scale of each layer independently based on its specific requirements. The fundamental balance operation APIs are stable and should be changed conservatively, while the business-facing APIs need to evolve quickly to adapt to the business requirements.
* Easier to test and debug: Separating the core balance operation APIs from the business-facing APIs makes it simpler to test and debug each layer independently, ensuring a more robust and reliable system.


## Decouple Read and Write - CQRS with Event Sourcing

Command and Query Responsibility Segregation(CQRS) means separating reads and writes into different models, using commands to update data and queries to read data. Commands represent actions to change the state of the system. Usually, we refer to the command as the write operation. We would prefer the use of command and will use it throughout this document. You can think they are the synonym of the write operations. Benefits of CQRS include:



* Independent scaling. CQRS allows the read and write workloads to scale independently and results in fewer lock contentions.
* Optimized data schemas. The query side can use a schema optimized for queries, while the command side uses a schema optimized for updates.
* Separation of concerns. Segregating the command and query sides can result in models that are more maintainable and flexible. Additionally, the command and query sides could have different optimization and scaling solutions.

Instead of storing just the current state of the data in a domain, in Event Sourcing, an append-only store is used to record the full series of actions taken on that data. The store acts as the golden source, and the records can be used to materialize the domain objects.

Marker and Auticuro follow the CQRS pattern with Event Sourcing, each shard of which is separated into the command side and query side, connected by an event store. Only the most critical logic, such as real-time balance check, is processed on the command side. Others are processed on the query side, tailored for query performance.


## Decouple Account Structure and Storage

The wallet service supports account hierarchy, a tree-like structure where an account can have a bunch of sub-accounts, and the account's balance is the total balance of its sub-accounts.



* Money movements are only allowed on the leaf accounts
* Balance queries of all accounts(leaf account + non-leaf accounts) are supported.

We decouple the two functionalities with CQRS methodology, putting the leaf account storage model on the command(write) side and the account hierarchy models on the query side because these two models are orthogonal. In this way, they can scale or adapt to business change independently.


## Correctness, Dependability, and Performance

The wallet service is implemented in the Rust programming language to achieve correctness and performance. Rust’s rich type system and ownership model guarantee memory safety and thread safety. Rust is blazingly fast and memory efficient: with no runtime or garbage collector, powering performance-critical services.

The wallet service employs a single-threaded, lockless critical path to reduce contention, improve throughput, and reduce latency. This choice dramatically reduces the engineering complexity as error-prone multi-threaded code is discarded without hurting the performance.

The command side of Marker and Auticuro is built on top of the [Raft algorithm](https://raft.github.io/), leveraging Raft to achieve strong consistency and high availability. Raft is a consensus algorithm, which is a protocol used in distributed systems to achieve agreement among multiple nodes on a specific value or state, even in the presence of failures. Raft achieves consensus via an elected leader. A server in a raft cluster is either a leader or a follower and can be a candidate in the precise case of an election (leader unavailable). The leader is responsible for log replication to the followers.
