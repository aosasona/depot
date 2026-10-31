set dotenv-load := true

build-ui:
    pnpm --dir web build

check-rust:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace

check-ui:
    pnpm --dir web lint
    pnpm --dir web format:check
    pnpm --dir web typecheck

release: build-ui
    cargo build --release -p depot

smoke: release
    ./target/release/depot doctor
