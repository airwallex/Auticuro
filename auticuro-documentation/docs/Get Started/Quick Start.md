---
sidebar_position: 1
---

# Quick Start
To quickly get started with Auticuro, follow these simple steps:
- Visit the Auticuro GitHub repository at `https://github.com/airwallex/Auticuro`.
- Star and fork the repository to your personal account.
- Clone the forked repository to your local machine using `git clone https://github.
  com/your-username/Auticuro.git`.
- Navigate to the Auticuro directory using `cd Auticuro`.
- Get started with instructions below.

## 1. For the impatient
### 1.1. Setup Environment
#### For Ubuntu
```
sh scripts/setup_for_ubuntu.sh
```

#### For MacOS
```
sh scripts/setup_for_mac.sh
```

### 1.2. Clone and Build
```
git clone https://github.com/your-username/Auticuro.git
cd Auticuro
cargo build --release
```

### 1.3. Start the Auticuro cluster
```
sh scripts/start_cluster.sh
```

### 1.4. Send Requests to the Cluster
Create two accounts, and transfer money from one account to another, then check their balance and balance change history.
```
sh scripts/transfer_example.sh
```

## 2. The detailed Guide
### 2.1. Setup Environment
#### For Ubuntu
```
# Install tools
apt-get update -y && apt-get upgrade -y && apt-get install -y build-essential curl cmake

# Install grpcurl
wget https://github.com/fullstorydev/grpcurl/releases/download/v1.8.5/grpcurl_1.8.5_linux_x86_64.tar.gz --no-check-certificate
tar -xvf grpcurl_1.8.5_linux_x86_64.tar.gz
chmod +x grpcurl

# Download rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --profile=default -y

# Set cargo path
source $HOME/.cargo/env

# Install rust nightly
rustup toolchain install nightly-2022-01-13

# Set rust nightly as default
rustup default nightly-2022-01-13

# Install rustfmt component
rustup component add rustfmt

# For UT Coverage, need install grcov and llvm-tools
cargo install grcov
rustup component add llvm-tools-preview
```

#### For MacOS
```
# Install tools
brew install curl
brew install protobuf
brew install cmake && brew link cmake
brew install grpcurl

# Download rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --profile=default -y

# Set cargo path
source $HOME/.cargo/env

# Install rust nightly
rustup toolchain install nightly-2022-01-13

# Set rust nightly as default
rustup default nightly-2022-01-13

# Install rustfmt component
rustup component add rustfmt

# Install xcode(Ignore if installed)
# xcode-select --install

# For UT Coverage, need install grcov and llvm-tools
cargo install grcov
rustup component add llvm-tools-preview
```

### 2.2. Build and Test
Below commands are platform independent, applicable for both Ubuntu and MacOS.

#### Build
```
cargo build --release
```

#### Unit Test
```
cargo test --release
```

#### Run Test Coverage
- Generate the test coverage report
  The report will be auto-open in the browser
```
cd firm-wallet
sh scripts/run_test_coverage.sh
```

- View the report directory(Optional)

  Find the report in the dir `target/release/coverage`
```
ls target/release/coverage 

# The output
# badges          coverage.json   index.html      src
```


### 2.3. Start a 5-node cluster locally
- The quick way
  It will start a 5-node `firm-wallet-service` cluster and a `firm-wallet-gateway` which will find leader and redirect
  requests to the leader node of `firm-wallet-service` cluster.

```
cd firm-wallet

sh scripts/start_cluster.sh

# Press `Ctrl + C` to shutdown the cluster
```

- Manually Start

To start all 5 raft nodes manually, execute the`run_node.sh` script with a store id:
```
# Start the firm-wallet-gateway
cd firm-wallet/firm-wallet-gateway
sh run_gateway.sh

# Change dir to firm-wallet-service
cd firm-wallet/firm-wallet-service

# Start 5 nodes, open a new termial for each node
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

### 2.4. Send Request With gRPCurl
Write requests can ONLY be processed by the leader, read requests can be processed by every node.
The `firm-wallet-gateway`serves at `port=20171`, the `firm-wallet-service` cluster serves at `port=20161~20165`.

Run `grpcurl` commands in `dependencies/hologram-protos/src/proto`
```
cd dependencies/hologram-protos/src/proto
```

#### 2.4.1. Send Write Requests
The `firm-wallet-gateway` will redirect write requests to the leader of the `wallet-service` cluster.

- Create Accounts
```
# Create an account for ben
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto account_management_servicepb.proto -d '{"account_config":{"asset_class":{"cash":{"currency":"USD"}}},"header":{"account_id":"ben","dedup_id":"asjdh78y"}}'  127.0.0.1:20171 firm_wallet.account_management_servicepb.AccountManagementService/CreateAccount

# Create an account for tony
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto account_management_servicepb.proto -d '{"account_config":{"asset_class":{"cash":{"currency":"USD"}}},"header":{"account_id":"tony","dedup_id":"gklfjg8937"}}'  127.0.0.1:20171 firm_wallet.account_management_servicepb.AccountManagementService/CreateAccount
```

- Transfer
```
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"dedup_id":"1234567890", "transfer_spec": {"from_account_id": "tony", "to_account_id": "ben", "amount": "1234.5"}}'  127.0.0.1:20171 firm_wallet.balance_operation_servicepb.BalanceOperationService/Transfer
```

#### 2.4.2. Send Read Requests
Each node of the `wallet-service` cluster has consistent state, send read requests to each node to check.

- Query Balance
```
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20161 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20162 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20163 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20164 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20165 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance
```

- Query Events
```
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20161 firm_wallet.internal_servicepb.InternalService/QueryEvents
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20162 firm_wallet.internal_servicepb.InternalService/QueryEvents
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20163 firm_wallet.internal_servicepb.InternalService/QueryEvents
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20164 firm_wallet.internal_servicepb.InternalService/QueryEvents
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20165 firm_wallet.internal_servicepb.InternalService/QueryEvents
```
