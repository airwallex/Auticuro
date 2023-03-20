# Installation guide for MacOS
set -x

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

# For UT Coverage, need install grcov and llvm-tools
cargo install grcov
rustup component add llvm-tools-preview

# Install xcode(Ignore if installed)
#xcode-select --install
