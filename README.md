Auticuro: A High Performance, Strong Consistent Distributed Wallet Service
=========
[<img alt="docs.rs" src="https://img.shields.io/badge/docs-firm-wallet-service-66c2a5?style=for-the-badge&labelColor=555555&logo=docs.rs" height="20">](https://financial_platform.pages.awx.im/the-hologram/firm-wallet/firm-wallet-service/)
[<img alt="docs.rs" src="https://img.shields.io/badge/docs-firm-wallet-gateway-66c2a5?style=for-the-badge&labelColor=555555&logo=docs.rs" height="20">](https://financial_platform.pages.awx.im/the-hologram/firm-wallet/firm-wallet-gateway/)

## Introduction
Auticuro is a high-performance wallet service, leveraging [Raft consensus algorithm](https://raft.github.io/raft.pdf) 
to achieve both high availability and strong consistency, which could tolerate 2 nodes down in a 5-nodes deployment. 
With data flowing only from Raft leader to followers, it's naturally an event sourcing system with single source of
truth well fit for mission-critical financial scenarios.

Rust language is selected for its performance comparable to C++, as well as the language property of memory and thread
safety. For raft consensus implementation, we used [raft-rs](https://github.com/tikv/raft-rs) crate developed by
PingCAP.

Transferring money between 2 accounts is a typical use case. With well-modularized underlying consensus support, 
it's easy to be extended to support more complicated business scenarios.

About the naming -- **Auticuro**: AU (gold in Latin is aurum, which is the chemistry symbol for gold AU) + “curo (Italian for protect)

## Features
### Functional Features
  - Balance operations 
    - ✅ Bilateral money transfer
    - ✅ Batch balance operation (Increase/Decrease balance on more than accounts atomically)
  - Account management operations
    - ✅ Create/Delete/Lock/Unlock an account
    - ✅ Change the upper/lower balance limit of an account
- ✅ Support multi asset class (cash, crypto, coupon…)
- Support account hierarchy(TBD)

Currently, the bilateral money transfer is ready, other features are on the road map.

### Non-functional Features
- **High throughput**

    Separated reads and writes, no lock on the critical write path
- **High availability**

    Built upon the Raft-based consensus message queue to achieve HA
- **Low latency**

    The wallet service pushes down computation to the storage layer to achieve low latency 
- **Horizontal scalability**

   The horizontal scalability can be achieved with the help of an in-house workflow engine(Marker)


## Quick start
See details [here](docs/quickstart.md)

## Documentation
### Architecture
![Alt text](./resources/Architecture.drawio.svg)

The above figure shows the design architecture and how a transfer gRPC request is handled. Each raft node holds its raft
log, `Wallet StateMachine`, with attached `RocksDB` and `In-Mem State`. The raft log replication process is not shown here.

__Steps:__

1. Raft leader accepts the request, appends it to its raft log, and receives the Raft log index and offset. (Raft
   consensus module handles the log replication and advances the `applied_index`, indicating a log entry is ready for
   applying).

2. The leader gRPC thread registers the TransferRequest in the msg_broker with `Raft(index, offset)` and waits for the
response from the msg_broker asynchronously. 

    On every node, a single thread loop of `Log Consumer`  polls committed raft logs until raft log's `applied_index`
    and sends them to `Wallet StateMachine` via a Single Processor Single Consumer (SPSC) channel.

3. `Wallet StateMachine` applies the received log to the state and persists the state to RocksDB, 
and send the response for `Raft(index, offset)` to msg_broker

4. The leader gRPC thread receives the response from the msg_broker(by joining the request and response at 
Raft(index, offset)) and sends it back to the client.

### Event Sourcing with CQRS

<img src="resources/Event Sourcing.svg" width = "600" height = "650" alt="Event Sourcing" align=center />

The design follows the CQRS system to segregate writing and reading.

__Wallet Service Cluster__

The Leader node serves client transfer requests and replies with the execution result.

__Query Service(Archiver)__

It will persist all the historical commands and events

__Query Service(OLTP)__

Derived views are built from the event stream, a typical scenario is to calculate the near real-time account balance at 
the non-leaf level. This way is very flexible since whenever a new derived view is needed, a new service can just be
added without changes to the existing services, which aligns with the Open-Close principle.

__Query Service(OLAP)__

Event streams can be synchronized from PG to BigQuery through CDC, such that analytical requirements can be easily supported.

## Configurations
See details [here](docs/configuration.md)

## Performance

#### Configuration

8 vCPUs * 5 node cluster on GCP, attached with SSD persistent disks (pd-ssd)

#### Test Strategy

A rust-based performance testing tool was built for the wallet service, the tool will start multiple coroutines and
send requests sequentially to the server.

#### Test Result
|RPC type|Concurrency |Metadata Payload|TPS|Avg. Latency (ms)|p99 Latency (ms)|
|:---:|:---:|:---:|:---:|:---:|:---:|
|     Transfer     |100|128B|13.5k|13|81|

- Latency Distribution vs Throughput

  **P99 latency < 20 ms @ TPS=10K**

  <img alt="latency distribution" src="resources/latency_distribution.gif" height="400">

## Monitoring
#### Http Metrics Server

An HTTP metrics server is built on each node to
check [prometheus](https://prometheus.io/docs/prometheus/latest/getting_started/)
metrics and change the log level dynamically.

```
# Get prometheus metrics
curl http://0.0.0.0:20211/metrics

# Change log level
curl -X PUT http://0.0.0.0:20211/log-level -d '{"log-level":"debug"}'
curl -X PUT http://0.0.0.0:20211/log-level -d '{"log-level":"info"}'
```