#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

go build -o defi ./cmd/defi

./defi providers list --results-only >/dev/null

./defi swap quote \
  --provider taikoswap \
  --chain taiko \
  --from-asset USDC \
  --to-asset WETH \
  --amount 1000000 \
  --results-only >/dev/null

./defi bridge quote \
  --provider lifi \
  --from 1 \
  --to 8453 \
  --asset USDC \
  --amount 1000000 \
  --results-only >/dev/null

./defi approvals plan \
  --chain taiko \
  --asset USDC \
  --spender 0x00000000000000000000000000000000000000bb \
  --amount 1000000 \
  --from-address 0x00000000000000000000000000000000000000aa \
  --results-only >/dev/null

./defi bridge plan \
  --provider lifi \
  --from 1 \
  --to 8453 \
  --asset USDC \
  --amount 1000000 \
  --from-address 0x00000000000000000000000000000000000000aa \
  --results-only >/dev/null

./defi lend supply plan \
  --provider aave \
  --chain 1 \
  --asset USDC \
  --amount 1000000 \
  --from-address 0x00000000000000000000000000000000000000aa \
  --results-only >/dev/null

./defi rewards claim plan \
  --provider aave \
  --chain 1 \
  --from-address 0x00000000000000000000000000000000000000aa \
  --assets 0x00000000000000000000000000000000000000d1 \
  --reward-token 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48 \
  --results-only >/dev/null

./defi rewards compound plan \
  --provider aave \
  --chain 1 \
  --from-address 0x00000000000000000000000000000000000000aa \
  --assets 0x00000000000000000000000000000000000000d1 \
  --reward-token 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48 \
  --amount 1000 \
  --results-only >/dev/null

rm -f ./defi
