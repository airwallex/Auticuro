# Lightning: A High Performance, Strong Consistent Distributed Wallet Service
=========
[<img alt="docs.rs" src="https://img.shields.io/badge/docs-firm-wallet-service-66c2a5?style=for-the-badge&labelColor=555555&logo=docs.rs" height="20">](https://financial_platform.pages.awx.im/the-hologram/firm-wallet/firm-wallet-service/)
[<img alt="docs.rs" src="https://img.shields.io/badge/docs-firm-wallet-gateway-66c2a5?style=for-the-badge&labelColor=555555&logo=docs.rs" height="20">](https://financial_platform.pages.awx.im/the-hologram/firm-wallet/firm-wallet-gateway/)

## Introduction
Lightning is a high-performance wallet service, leveraging [Raft consensus algorithm](https://raft.github.io/raft.pdf) 
to achieve both high availability and strong consistency, which could tolerate 2 nodes down in a 5-nodes deployment. 
With data flowing only from Raft leader to followers, it's naturally an event sourcing system with single source of
truth well fit for mission-critical financial scenarios.

Rust language is selected for its performance comparable to C++, as well as the language property of memory and thread
safety. For raft consensus implementation, we used [raft-rs](https://github.com/tikv/raft-rs) crate developed by
PingCAP.

Transferring money between 2 accounts is a typical use case. With well-modularized underlying consensus support, 
it's easy to be extended to support more complicated business scenarios.

## Features
### Functional Features
  - Balance operations 
    - ✅ Bilateral money transfer
    - Batch balance operation (Increase/Decrease balance on more than accounts atomically)
  - Account management operations
    - Create/Delete/Lock/Unlock an account
    - Change the upper/lower balance limit of an account
  - Support account hierarchy
  - Support multi asset class (cash, crypto, coupon…)

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

<img src="resources/Event Sourcing.svg" width = "600" height = "650" alt="图片名称" align=center />

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

### Quick start

#### Rust Environment Setup
For Ubuntu
```
# 1. Install tools and set git credentials
apt-get update -y && apt-get upgrade -y && apt-get install -y build-essential curl cmake

# Set Git Credential (for hologram-protos)
export CARGO_NET_GIT_FETCH_WITH_CLI=true

# 2. download rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --profile=default -y

# 3. set cargo path
source $HOME/.cargo/env

# 4. install rust nightly
rustup toolchain install nightly-2022-01-13

# 5. set rust nightly as default
rustup default nightly-2022-01-13

# 6. install rustfmt component
rustup component add rustfmt
```

For MacOS
```
# 1. Install tools and set git credentials
brew install curl
brew install protobuf
brew install cmake && brew link cmake

# Install xcode(Ignore if installed)
xcode-select --install

# Set Git Credential (for hologram-protos)
export CARGO_NET_GIT_FETCH_WITH_CLI=true

# 2. download rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --profile=default -y

# 3. set cargo path
source $HOME/.cargo/env

# 4. install rust nightly
rustup toolchain install nightly-2022-01-13

# 5. set rust nightly as default
rustup default nightly-2022-01-13

# 6. install rustfmt component
rustup component add rustfmt
```

#### Build Project and Start Service
Below commands are platform independent, applicable for both Ubuntu and MacOS.

Build 
```
cargo build --release
```

Test
```
cargo test --release
```

To start all 5 raft nodes locally, execute the`run_node.sh` script with a store id:

```
# 1. change dir to firm-wallet-service
cd firm-wallet-service

# 2. Start 5 peers
sh run_node.sh 1
sh run_node.sh 2
sh run_node.sh 3
sh run_node.sh 4
sh run_node.sh 5
```

Otherwise, please update the corresponding `peer_<id>` and `status_address` for each service.

Upon execution, cluster configuration, raft log/state, and application state are persisted to RocksDB specified by
`db_path`, `raft_db_path`, `wallet_db_path` each. For a fresh new start, clear all three databases by deleting files under
the specified path.



#### Send Request With gRPCurl
Refer to the [API Doc](docs/api.md) for more details.
Todo (Should put consensus protos in a seperated project from hologram_protos and put the grpcurl example there)
1. Download and install [gRPCurl](https://github.com/fullstorydev/grpcurl) on Linux x86 (see all releases
   [here](https://github.com/fullstorydev/grpcurl/releases))

```
wget https://github.com/fullstorydev/grpcurl/releases/download/v1.8.5/grpcurl_1.8.5_linux_x86_64.tar.gz --no-check-certificate
tar -xvf grpcurl_1.8.5_linux_x86_64.tar.gz
chmod +x grpcurl
./grpcurl --help
```

2. Request example, specify proto path with `--import-path`, proto with `--proto`

Please fetch proto definitions as specified in [API Doc](docs/api.md)
```
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"dedup_id":"1234567890", "transfer_spec": {"from_account_id": "tony", "to_account_id": "ben", "amount": "1234.5"}}'  127.0.0.1:20161 firm_wallet.balance_operation_servicepb.BalanceOperationService/Transfer
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"dedup_id":"1234567890", "transfer_spec": {"from_account_id": "tony", "to_account_id": "ben", "amount": "1234.5"}}'  127.0.0.1:20162 firm_wallet.balance_operation_servicepb.BalanceOperationService/Transfer
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"dedup_id":"1234567890", "transfer_spec": {"from_account_id": "tony", "to_account_id": "ben", "amount": "1234.5"}}'  127.0.0.1:20163 firm_wallet.balance_operation_servicepb.BalanceOperationService/Transfer
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"dedup_id":"1234567890", "transfer_spec": {"from_account_id": "tony", "to_account_id": "ben", "amount": "1234.5"}}'  127.0.0.1:20164 firm_wallet.balance_operation_servicepb.BalanceOperationService/Transfer
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"dedup_id":"1234567890", "transfer_spec": {"from_account_id": "tony", "to_account_id": "ben", "amount": "1234.5"}}'  127.0.0.1:20165 firm_wallet.balance_operation_servicepb.BalanceOperationService/Transfer
```

```
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20161 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20162 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20163 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20164 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20165 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance
```

```
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20161 firm_wallet.internal_servicepb.InternalService/QueryEvents
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20162 firm_wallet.internal_servicepb.InternalService/QueryEvents
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20163 firm_wallet.internal_servicepb.InternalService/QueryEvents
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20164 firm_wallet.internal_servicepb.InternalService/QueryEvents
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20165 firm_wallet.internal_servicepb.InternalService/QueryEvents
```

### Configurations
#### Wallet Service Configuration
| Wallet Service configuration |                  Description                   |  Default Value  |
|:----------------------------:|:----------------------------------------------:|:---------------:|
|    query_events_max_count    | Max queried events count of QueryEventsRequest |      1000       |

- Tuning the Wallet Service Configuration

    `export query_events_max_count=100`


#### GC Configuration
The wallet's earliest entries(by seq_num) will be truncated if the GC is enabled and exceeds the `event_log_gc_count_limit`, currently
the GC is disabled by default.

- How to enable GC

   ```export enable_gc_of_event_log=true```


- Tuning the GC configurations

  Please remember to export gc configuration with gc **enabled**.

  ```
   export enable_gc_of_event_log=true
   export event_log_gc_count_limit=10000
  ```

|             GC configuration              |                                     Description                                     | Default Value |
|:-----------------------------------------:|:-----------------------------------------------------------------------------------:|:-------------:|
| event_log_gc_poll_interval_millis(millis) |                  The poll interval if gc thread find gc not needed                  |     1000      |
|       event_log_gc_count_limit(u64)       |                     Max entries that can be stored in rocks db                      |  50 million   |
|       event_log_gc_batch_size(u64)        |                   Num of seq_nums to be deleted in a delete batch                   |      200      |
|       event_log_gc_percentage(f64)        | Each time event_log_gc_percentage * event_log_gc_count_limit events will be deleted |      0.1      |

## Performance

#### Configuration

8 vCPUs * 5 node cluster on GCP, attached with SSD persistent disks (pd-ssd)

#### Test Strategy

A rust-based performance testing tool was built for the wallet service, the tool will start multiple coroutines and
send requests sequentially to the server. See details of the test tool [here](https://gitlab.awx.im/amanda.tian/concensus-stress-test).

#### Test Result

|RPC type|Concurrency |Metadata Payload|TPS|Avg. Latency (ms)|p99 Latency (ms)|
|:---:|:---:|:---:|:---:|:---:|:---:|
|     Transfer     |100|128B|13.5k|13|81|


### Monitoring
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