# defi-cli

<p align="center">
  <img src="assets/logo.png" alt="defi-cli logo" width="600" />
</p>

Query lending rates, compare yield, get bridge and swap quotes — across protocols and chains, from a single CLI.

Built for AI agents and scripts. Stable JSON output, canonical identifiers (CAIP-2/CAIP-19), and deterministic exit codes make it easy to pipe into any workflow.

## Features

- **Lending** — query markets and rates from Aave, Morpho, and more (with DefiLlama fallback).
- **Yield** — compare opportunities across protocols and chains, filter by TVL and APY.
- **Bridging** — get cross-chain quotes (Across, LiFi, Bungee Auto) and bridge analytics (volume, chain breakdown).
- **Swapping** — get on-chain swap quotes (1inch, Uniswap, Bungee Auto).
- **Chains & protocols** — browse top chains by TVL, inspect chain TVL by asset, discover protocols, resolve asset identifiers.
- **Automation-friendly** — JSON-first output, field selection (`--select`), strict mode, and a machine-readable schema export.

## Install

### 1) Quick install (macOS/Linux)

Installs the latest tagged release from GitHub:

```bash
curl -fsSL https://raw.githubusercontent.com/ggonzalez94/defi-cli/main/scripts/install.sh | sh
```

Install a specific version (accepted: `latest`, `stable`, `vX.Y.Z`, `X.Y.Z`):

```bash
curl -fsSL https://raw.githubusercontent.com/ggonzalez94/defi-cli/main/scripts/install.sh | sh -s -- v0.1.1
```

### 2) Go install

```bash
go install github.com/ggonzalez94/defi-cli/cmd/defi@latest
```

### 3) Manual install from release artifacts

1. Download the right archive from GitHub Releases:
   - Linux/macOS: `defi_<version>_<os>_<arch>.tar.gz`
   - Windows: `defi_<version>_windows_<arch>.zip`
2. Verify checksums with `checksums.txt`.
3. Extract and move `defi` into your `PATH`.

### 4) Build from source

```bash
go build -o defi ./cmd/defi
```

Verify install:

```bash
defi version --long
```

## Quick Start

```bash
defi providers list --results-only
defi chains top --limit 10 --results-only --select rank,chain,tvl_usd
defi chains assets --chain 1 --asset USDC --results-only # Requires DEFI_DEFILLAMA_API_KEY
defi assets resolve --chain base --symbol USDC --results-only
defi lend markets --protocol aave --chain 1 --asset USDC --results-only
defi lend rates --protocol morpho --chain 1 --asset USDC --results-only
defi yield opportunities --chain base --asset USDC --limit 20 --results-only
defi yield opportunities --chain 1 --asset USDC --providers aave,morpho --limit 10 --results-only
defi bridge list --limit 10 --results-only # Requires DEFI_DEFILLAMA_API_KEY
defi bridge details --bridge layerzero --results-only # Requires DEFI_DEFILLAMA_API_KEY
defi bridge quote --from 1 --to 8453 --asset USDC --amount 1000000 --results-only
defi bridge quote --provider bungee --from hyperevm --to 8453 --asset USDC --amount 1000000 --results-only
```

`yield opportunities --providers` accepts provider names from `defi providers list` (e.g. `defillama,aave,morpho`).

Bridge quote examples:

```bash
defi bridge quote --from 1 --to 8453 --asset USDC --amount 1000000 --results-only # Defaults to Across
defi bridge quote --provider lifi --from 1 --to 8453 --asset USDC --amount 1000000 --results-only
defi bridge quote --provider bungee --from 1 --to 8453 --asset USDC --amount 5000000 --results-only
```

Swap quote examples:

```bash
export DEFI_1INCH_API_KEY=...
defi swap quote --provider 1inch --chain 1 --from-asset USDC --to-asset DAI --amount 1000000 --results-only
defi swap quote --provider bungee --chain hyperevm --from-asset USDC --to-asset WHYPE --amount 5000000 --results-only
```

## Command API Key Requirements

Most commands do not require provider API keys.

When a provider requires authentication, bring your own key:

- `defi swap quote --provider 1inch` -> `DEFI_1INCH_API_KEY`
- `defi swap quote --provider uniswap` -> `DEFI_UNISWAP_API_KEY`
- `defi chains assets` -> `DEFI_DEFILLAMA_API_KEY`
- `defi bridge list` -> `DEFI_DEFILLAMA_API_KEY`
- `defi bridge details` -> `DEFI_DEFILLAMA_API_KEY`

Bungee quotes (`bridge quote --provider bungee`, `swap quote --provider bungee`) are keyless by default. Optional dedicated-backend mode is enabled only when both `DEFI_BUNGEE_API_KEY` and `DEFI_BUNGEE_AFFILIATE` are set.

`defi providers list` includes both provider-level key metadata and capability-level key metadata (`capability_auth`).

## API Keys

- `DEFI_1INCH_API_KEY` (required for `swap quote --provider 1inch`)
- `DEFI_UNISWAP_API_KEY` (required for `swap quote --provider uniswap`)
- `DEFI_DEFILLAMA_API_KEY` (required for `chains assets`, `bridge list`, and `bridge details`)
- `DEFI_BUNGEE_API_KEY` + `DEFI_BUNGEE_AFFILIATE` (optional pair for Bungee dedicated backend on quote routes)

Configure keys with environment variables (recommended):

```bash
export DEFI_1INCH_API_KEY=...
export DEFI_UNISWAP_API_KEY=...
export DEFI_DEFILLAMA_API_KEY=...
export DEFI_BUNGEE_API_KEY=...
export DEFI_BUNGEE_AFFILIATE=...
```

For persistent shell setup, add exports to your shell profile (for example `~/.zshrc`).

If a keyed provider is used without a key, CLI exits with code `10`.

## Config (Optional)

Most users only need env vars for provider keys. Use config when you want persistent non-secret defaults (output mode, timeout/retries, cache behavior).

Config precedence is:

- `flags > env > config file > defaults`

Default config path:

- `${XDG_CONFIG_HOME:-~/.config}/defi/config.yaml`

Default cache paths:

- `${XDG_CACHE_HOME:-~/.cache}/defi/cache.db`
- `${XDG_CACHE_HOME:-~/.cache}/defi/cache.lock`

Example optional config:

```yaml
output: json
strict: false
timeout: 10s
retries: 2
cache:
  enabled: true
  max_stale: 5m
```

## Cache Policy

- Command TTLs are fixed in code (`chains/protocols/chains assets`: `5m`, `lend markets`: `60s`, `lend rates`: `30s`, `yield`: `60s`, `bridge/swap quotes`: `15s`).
- Cache entries are served directly only while fresh (`age <= ttl`).
- After TTL expiry, the CLI fetches provider data immediately.
- `cache.max_stale` / `--max-stale` is only a temporary provider-failure fallback window (currently `unavailable` / `rate_limited`).
- If fallback is disabled (`--no-stale` or `--max-stale 0s`) or stale data exceeds the budget, the CLI exits with code `14`.
- Metadata commands (`version`, `schema`, `providers list`) bypass cache initialization.

## Caveats

- Morpho can surface extreme APY values on very small markets. Prefer `--min-tvl-usd` when ranking yield.
- `chains assets` requires `DEFI_DEFILLAMA_API_KEY` because DefiLlama chain asset TVL is key-gated.
- `bridge list` and `bridge details` require `DEFI_DEFILLAMA_API_KEY`; quote providers (`across`, `lifi`, `bungee`) are keyless by default.
- Category rankings from `protocols categories` are deterministic and sorted by `tvl_usd`, then protocol count, then name.
- `--chain` normalization supports additional aliases/IDs including `mantle`, `megaeth`/`mega eth`/`mega-eth`, `ink`, `scroll`, `berachain`, `gnosis`/`xdai`, `linea`, `sonic`, `blast`, `fraxtal`, `world-chain`, `celo`, `taiko`/`taiko alethia`, `zksync`, and `hyperevm`.
- Bungee Auto-mode quote coverage is chain+token dependent; unsupported pairs return provider errors even when chain normalization succeeds.
- Bungee quote requests use deterministic placeholder sender/receiver addresses for quote-only resolution (`0x000...001`).
- Bungee dedicated backend routing only activates when both `DEFI_BUNGEE_API_KEY` and `DEFI_BUNGEE_AFFILIATE` are set; if either is missing, requests use the public backend.
- MegaETH bootstrap symbol parsing currently supports `MEGA`, `WETH`, and `USDT` (`USDT` maps to the chain's `USDT0` contract address). Official Mega token list currently has no Ethereum L1 `MEGA` token entry.
- For chains without bootstrap symbol entries, pass token address or CAIP-19 via `--asset`/`--from-asset`/`--to-asset` for deterministic resolution.
- For `lend`/`yield`, unresolved asset symbols skip DefiLlama-based symbol matching and may disable fallback/provider selection to avoid unsafe broad matches.

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

## Development

### Folder Structure

```text
cmd/
  defi/main.go                    # CLI entrypoint

internal/
  app/runner.go                   # command wiring, routing, cache flow
  providers/                      # external adapters
    aave/ morpho/                 # direct lending + yield
    defillama/                    # normalization + fallback + bridge analytics
    across/ lifi/ bungee/         # bridge quotes
    oneinch/ uniswap/ bungee/     # swap
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
```
### Testing

```bash
go test ./...
```
