set -x

# GC configuration
# export enable_gc_of_event_log=true
# export event_log_gc_batch_size=200
# export event_log_gc_count_limit=1000000
# export event_log_gc_poll_interval_millis = 1000
# export event_log_gc_percentage=0.1

# Wallet Service Configuration
# export query_events_max_count=100

# Optional
# size of the raft cluster, valid value is `1`, `3`, `5`.
# if not set, fallback to default value `5`.
export cluster_size=5

# store_id is equal to peer_id, locates in `[1, cluster_size]`
export store_id=$1
export peer_id=$1

# path to rocksDB
export db_path="./tmp/db_$1"
export raft_db_path="./tmp/raft_db_$1"
export wallet_db_path="./tmp/wallet_db_$1"

# cluster_id is hard coded to `1`
export cluster_id=1

# the nodes comprising the cluster
# `peer_i` should be set for all `i` within `[1, cluster_size]`.
export peer_1=0.0.0.0:20161
export peer_2=0.0.0.0:20162
export peer_3=0.0.0.0:20163
export peer_4=0.0.0.0:20164
export peer_5=0.0.0.0:20165

# Optional
# http port used by status_server
# `status_address_i` should be set for all `i` within `[1, cluster_size]`.
# if not set, fallback to default value `0.0.0.0:20211`.
export status_address_1=0.0.0.0:20211
export status_address_2=0.0.0.0:20212
export status_address_3=0.0.0.0:20213
export status_address_4=0.0.0.0:20214
export status_address_5=0.0.0.0:20215

# Optional
# log level of consensus module,
# valid value is `trace`, `debug`, `info`, `warn`, `error`, `critical`.
# if not set, fallback to default value `debug`.
export log_level="debug"

export log_consumer_buffer_size=1024
# Build the project and run service
cargo build --release
export RUST_BACKTRACE=1
../target/release/firm-wallet-service
