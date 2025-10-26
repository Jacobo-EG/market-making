# List all available tasks
list:
    cargo update 
    just --list

# Check 
check:
    cargo check --workspace 
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features --all -- --deny warnings
    cargo clippy -- --deny warnings

# Automatically format code and fix simple Clippy warnings
fix:
    cargo fmt --all
    cargo clippy --allow-dirty --allow-staged --fix -- -W unused_imports -W clippy::all

clean:
    cargo clean

# Build the project in debug mode
build:
    # TODO

# Build the project in release mode
release:
    # TODO

bench:
    cargo bench

# Run unit tests
test:
  cargo test --workspace -- --nocapture
  cargo test  --workspace --doc
  cargo test  --workspace --all-targets --all-features -- --nocapture