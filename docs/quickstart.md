## Quick Start
### Setup Environment
#### For Ubuntu
- The quick way
```
sh scripts/install_for_ubuntu.sh
```

- Manually setup
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
```

#### For MacOS
- The quick way
```
sh scripts/install_for_mac.sh
```

- Manually setup
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
```

### Clone the projects
Create a `working_dir` and clone `Auticuro` and `hologram-protos`.
```
# Create working dir
mkdir working_dir && cd working_dir

# Clone the Auticuro(firm-wallet) project
git clone https://gitlab.awx.im/financial_platform/open_source/firm-wallet.git

# Clone the proto definitions
git clone https://gitlab.awx.im/financial_platform/the-hologram/hologram-protos.git
```

Open two terminals, one for `firm-wallet` and another for `hologram-protos`

### Build and Test
Below commands are platform independent, applicable for both Ubuntu and MacOS.

#### Build
```
cd working_dir/firm-wallet

# Optional (Setting credentials for cloning hologram-protos in case of build failure)
export CARGO_NET_GIT_FETCH_WITH_CLI=true

cargo build --release
```

#### Unit Test
```
cd working_dir/firm-wallet
cargo test --release
```

### Start a 5-node cluster locally
- The quick way
```
cd working_dir/firm-wallet

sh scripts/start_cluster.sh

# Press `Ctrl + C` to shutdown the cluster
```

- Manually Start

To start all 5 raft nodes manually, execute the`run_node.sh` script with a store id:
```
# Change dir to firm-wallet-service
cd working_dir/firm-wallet/firm-wallet-service

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

### Send Request With gRPCurl
#### Prepare proto definitions

```
cd working_dir/hologram-protos

# Checkout the proto version currently used. 
# You could search "hologram_protos" in Cargo.lock to find the SHA of commit.
git checkout 683996b743a992f0cdf479d6387c55a83ad656ad

# Change to proto directory
cd src/proto
```
Write requests can ONLY be processed by the leader, read requests can be processed by every node.

#### Get Leader
Open a new terminal and execute below commands
```
cd working_dir/firm-wallet 
sh scripts/get_leader.sh

# Sample output as below means node 4 is the current leader
# Current Leader: 4
```

Run `grpcurl` commands in `hologram-protos/src/proto`
```
cd working_dir/hologram-protos/src/proto
```

#### Set LEADER_ID
```
# Get the leader id by calling 'sh scripts/get_leader.sh' as above
export LEADER_ID=4
```
#### Create Accounts
```
# Create an account for ben
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto account_management_servicepb.proto -d '{"account_config":{"asset_class":{"cash":{"currency":"USD"}}},"header":{"account_id":"ben","dedup_id":"asjdh78y"}}'  127.0.0.1:2016${LEADER_ID} firm_wallet.account_management_servicepb.AccountManagementService/CreateAccount

# Create an account for tony
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto account_management_servicepb.proto -d '{"account_config":{"asset_class":{"cash":{"currency":"USD"}}},"header":{"account_id":"tony","dedup_id":"gklfjg8937"}}'  127.0.0.1:2016${LEADER_ID} firm_wallet.account_management_servicepb.AccountManagementService/CreateAccount
```

#### Transfer
```
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"dedup_id":"1234567890", "transfer_spec": {"from_account_id": "tony", "to_account_id": "ben", "amount": "1234.5"}}'  127.0.0.1:2016${LEADER_ID} firm_wallet.balance_operation_servicepb.BalanceOperationService/Transfer
```

#### Query Balance
```
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20161 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20162 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20163 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20164 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto balance_operation_servicepb.proto -d '{"account_id": "tony"}'  127.0.0.1:20165 firm_wallet.balance_operation_servicepb.BalanceOperationService/QueryBalance
```

#### Query Events
```
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20161 firm_wallet.internal_servicepb.InternalService/QueryEvents
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20162 firm_wallet.internal_servicepb.InternalService/QueryEvents
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20163 firm_wallet.internal_servicepb.InternalService/QueryEvents
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20164 firm_wallet.internal_servicepb.InternalService/QueryEvents
grpcurl -plaintext -import-path ./firm_wallet -import-path ./ -proto internal_servicepb.proto  -d '{"first_seq_num": 1, "last_seq_num": 10}'  127.0.0.1:20165 firm_wallet.internal_servicepb.InternalService/QueryEvents
```