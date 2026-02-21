# defi-cli

Agent-first DeFi retrieval CLI with stable JSON contracts, canonical identifiers, cache support, and protocol adapters.

## Features

- JSON-first output envelope with stable exit codes.
- Global automation flags: `--json`, `--plain`, `--select`, `--results-only`, `--enable-commands`, `--strict`.
- Canonical IDs: CAIP-2 chains and CAIP-19 assets.
- Retrieval commands for chains, protocols, lending, bridging, swapping, and yield opportunities.
- Direct protocol adapters for Aave and Morpho lending/yield (with DefiLlama fallback routing for supported protocols).
- SQLite cache with lock-file coordination and staleness policy.
- Deterministic schema export (`defi schema ...`).

## Install

```bash
go build -o defi ./cmd/defi
```

## Folder Structure

```text
cmd/
  defi/main.go                    # CLI entrypoint

internal/
  app/runner.go                   # command wiring, routing, cache flow
  providers/                      # external adapters
    aave/ morpho/                 # direct lending + yield
    defillama/                    # normalization + fallback
    across/ lifi/                 # bridge
    oneinch/ uniswap/             # swap
    types.go                      # provider interfaces
  config/                         # file/env/flags precedence
  cache/                          # sqlite cache + file lock
  id/                             # CAIP + amount normalization
  model/                          # envelope + domain models
  out/                            # renderers
  errors/                         # typed errors / exit codes
  schema/                         # machine-readable CLI schema
  policy/                         # command allowlist
  httpx/                          # shared HTTP client

.github/workflows/ci.yml          # CI (test/vet/build)
AGENTS.md                         # contributor guide for agents
RESEARCH_DEFI_CLI_WRAPPER.md      # design context
```

## Quick Start

```bash
./defi providers list --results-only
./defi chains top --limit 10 --results-only --select rank,chain,tvl_usd
./defi assets resolve --chain base --symbol USDC --results-only
./defi lend markets --protocol aave --chain 1 --asset USDC --results-only
./defi lend rates --protocol morpho --chain 1 --asset USDC --results-only
./defi yield opportunities --chain base --asset USDC --limit 20 --results-only
./defi yield opportunities --chain 1 --asset USDC --providers aave,morpho --limit 10 --results-only
```

`yield opportunities --providers` accepts provider names from `defi providers list` (e.g. `defillama,aave,morpho`).

Bridge quote example:

```bash
./defi bridge quote --provider lifi --from 1 --to 8453 --asset USDC --amount 1000000 --results-only
```

Swap quote example (`1inch` requires API key):

```bash
export DEFI_1INCH_API_KEY=...
./defi swap quote --provider 1inch --chain 1 --from-asset USDC --to-asset DAI --amount 1000000 --results-only
```

## API Keys

- `DEFI_1INCH_API_KEY`
- `DEFI_UNISWAP_API_KEY`

If a keyed provider is used without a key, CLI exits with code `10`.

## Config

Default config path:

- `${XDG_CONFIG_HOME:-~/.config}/defi/config.yaml`

Default cache paths:

- `${XDG_CACHE_HOME:-~/.cache}/defi/cache.db`
- `${XDG_CACHE_HOME:-~/.cache}/defi/cache.lock`

Example config:

```yaml
output: json
strict: false
timeout: 10s
retries: 2
cache:
  enabled: true
  max_stale: 5m
providers:
  uniswap:
    api_key_env: DEFI_UNISWAP_API_KEY
  oneinch:
    api_key_env: DEFI_1INCH_API_KEY
```

## Testing

```bash
go test ./...
```

## Caveats

- Morpho can surface extreme APY values on very small markets. Prefer `--min-tvl-usd` when ranking yield.
- Direct lending/yield adapters currently exist for `aave` and `morpho`; other protocols may still rely on DefiLlama normalization/fallback paths.

## Exit Codes

- `0`: success
- `1`: internal error
- `2`: usage/validation error
- `10`: auth required/failed
- `11`: rate limited
- `12`: provider unavailable
- `13`: unsupported input/provider pair
- `14`: stale data beyond SLA
- `15`: partial results in strict mode
- `16`: blocked by command allowlist
