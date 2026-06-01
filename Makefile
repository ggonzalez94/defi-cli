.PHONY: build test test-release clippy fmt run release-check release-snapshot

build:
	cargo build --manifest-path rust/Cargo.toml --release -p defi-cli

test:
	cargo test --manifest-path rust/Cargo.toml --workspace

test-release:
	cargo test --manifest-path rust/Cargo.toml --workspace --release

clippy:
	cargo clippy --manifest-path rust/Cargo.toml --all-targets --all-features -- -D warnings

fmt:
	cargo fmt --manifest-path rust/Cargo.toml --all

run:
	cargo run --manifest-path rust/Cargo.toml -p defi-cli -- $(ARGS)

release-check:
	goreleaser check

release-snapshot:
	ulimit -n 8192 && goreleaser release --snapshot --clean --parallelism 1
