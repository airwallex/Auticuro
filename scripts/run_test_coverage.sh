#!/bin/sh

# Remove the grcov outputs
find . -type f -name '*.prof*' -delete
rm -rf ./target

export RUSTFLAGS="-Zinstrument-coverage"

cargo build --release

export LLVM_PROFILE_FILE="coverage-%p-%m.profraw"

# Run the tests in firm-wallet-service
cargo test --release --package firm-wallet-service

# Generate test coverage report
grcov . --binary-path ./target/release -s ./firm-wallet-service -t html --branch --ignore-not-existing -o ./target/release/coverage/

# Clean up the grcov outputs
find . -type f -name '*.prof*' -delete

# Open the test coverage report in browser
open target/release/coverage/index.html