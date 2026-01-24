list:
    cargo update 
    just --list

check:
    cargo check --workspace 
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features --all -- --deny warnings
    cargo clippy -- --deny warnings

fix:
    cargo fmt --all
    cargo clippy --allow-dirty --allow-staged --fix -- -W unused_imports -W clippy::all

clean:
    cargo clean

build:
    cargo build --workspace

release:
    cargo build --release --workspace

run config_file api_key api_secret:
    cargo run -- --config-file {{config_file}} --api-key {{api_key}} --api-secret {{api_secret}}

run-release config_file api_key api_secret:
    cargo run --release -- --config-file {{config_file}} --api-key {{api_key}} --api-secret {{api_secret}}

run-default:
    #!/usr/bin/env bash
    if [ -z "$KRAKEN_API_KEY" ] || [ -z "$KRAKEN_API_SECRET" ]; then
        echo "Error: KRAKEN_API_KEY and KRAKEN_API_SECRET environment variables must be set"
        exit 1
    fi
    cargo run -- --config-file config.toml --api-key "$KRAKEN_API_KEY" --api-secret "$KRAKEN_API_SECRET"

bench:
    cargo bench

test:
  cargo test --workspace -- --nocapture
  cargo test  --workspace --doc
  cargo test  --workspace --all-targets --all-features -- --nocapture