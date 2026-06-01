#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

DEFI_BIN="$ROOT_DIR/rust/target/release/defi"
ETH_RPC_URL="${DEFI_ETH_RPC_URL:-https://ethereum-rpc.publicnode.com}"

cargo build --manifest-path rust/Cargo.toml --release -p defi-cli

"$DEFI_BIN" providers list --results-only >/dev/null

"$DEFI_BIN" swap quote \
  --provider taikoswap \
  --chain taiko \
  --from-asset USDC \
  --to-asset WETH \
  --amount 1000000 \
  --results-only >/dev/null

"$DEFI_BIN" bridge quote \
  --provider lifi \
  --from 1 \
  --to 8453 \
  --asset USDC \
  --amount 1000000 \
  --results-only >/dev/null

"$DEFI_BIN" approvals plan \
  --chain taiko \
  --asset USDC \
  --spender 0x00000000000000000000000000000000000000bb \
  --amount 1000000 \
  --from-address 0x00000000000000000000000000000000000000aa \
  --results-only >/dev/null

"$DEFI_BIN" bridge plan \
  --provider lifi \
  --from 1 \
  --to 8453 \
  --asset USDC \
  --amount 1000000 \
  --from-address 0x00000000000000000000000000000000000000aa \
  --rpc-url "$ETH_RPC_URL" \
  --results-only >/dev/null

"$DEFI_BIN" lend supply plan \
  --provider aave \
  --chain 1 \
  --asset USDC \
  --amount 1000000 \
  --from-address 0x00000000000000000000000000000000000000aa \
  --rpc-url "$ETH_RPC_URL" \
  --results-only >/dev/null

"$DEFI_BIN" rewards claim plan \
  --provider aave \
  --chain 1 \
  --from-address 0x00000000000000000000000000000000000000aa \
  --assets 0x00000000000000000000000000000000000000d1 \
  --reward-token 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48 \
  --rpc-url "$ETH_RPC_URL" \
  --results-only >/dev/null

"$DEFI_BIN" rewards compound plan \
  --provider aave \
  --chain 1 \
  --from-address 0x00000000000000000000000000000000000000aa \
  --assets 0x00000000000000000000000000000000000000d1 \
  --reward-token 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48 \
  --amount 1000 \
  --rpc-url "$ETH_RPC_URL" \
  --results-only >/dev/null
