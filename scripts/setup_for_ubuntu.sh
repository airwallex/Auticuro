# Installation guide for Ubuntu
set -x

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