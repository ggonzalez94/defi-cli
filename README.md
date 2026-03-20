# defi-cli

<p align="center">
  <img src="assets/logo.png" alt="defi-cli logo" width="600" />
</p>

Query and act on DeFi lending, yield, bridge, and swap — across protocols and chains, from a single CLI.

Built for AI agents and scripts. Stable JSON output, canonical identifiers (CAIP-2/CAIP-19), and deterministic exit codes make it easy to pipe into any workflow.

## Features

- **Lending** — query markets/rates from Aave/Morpho/Kamino, account positions from Aave/Morpho, and execute loan actions (`lend supply|withdraw|borrow|repay`).
- **Yield** — compare opportunities, query positions, fetch historical series, and execute deposit/withdraw flows (Aave, Morpho).
- **Bridging** — get cross-chain quotes (Across, LiFi, Bungee), bridge analytics, and execute bridge plans (Across, LiFi).
- **Swapping** — get swap quotes (1inch, Uniswap, Jupiter, Tempo, TaikoSwap, Fibrous, Bungee) and execute swap plans (Tempo with native type 0x76 transactions and batched calls, TaikoSwap).
- **Approvals, transfers & rewards** — ERC-20 approvals/transfers and Aave rewards claim/compound flows.
- **Chains & protocols** — browse chains by TVL, inspect chain TVL by asset, discover protocols, resolve asset identifiers.
- **Automation-friendly** — JSON-first output, field selection (`--select`), structured JSON/file input (`--input-json`, `--input-file`), and a machine-readable schema export with required flags, enums, auth, and request/response metadata.

## Install

### 1) Quick install (macOS/Linux)

Installs the latest tagged release from GitHub:

```bash
curl -fsSL https://raw.githubusercontent.com/ggonzalez94/defi-cli/main/scripts/install.sh | sh
```

Install a specific version (accepted: `latest`, `stable`, `vX.Y.Z`, `X.Y.Z`):

```bash
curl -fsSL https://raw.githubusercontent.com/ggonzalez94/defi-cli/main/scripts/install.sh | sh -s -- v0.4.0
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

### Read: query markets and quotes

```bash
defi providers list --results-only
defi lend markets --provider aave --chain 1 --asset USDC --results-only
defi lend positions --provider aave --chain 1 --address 0xYourEOA --type all --results-only
defi yield opportunities --chain 1 --asset USDC --providers aave,morpho --limit 10 --results-only
defi yield history --chain 1 --asset USDC --providers aave --metrics apy_total --interval day --window 7d --limit 1 --results-only
defi bridge quote --provider across --from 1 --to 8453 --asset USDC --amount 1000000 --results-only
defi swap quote --provider tempo --chain tempo --from-asset pathUSD --to-asset USDC.e --amount 1000000 --results-only
```

### Act: plan and execute transactions

```bash
# Plan a swap (dry-run, no signer needed)
defi swap plan --provider tempo --chain tempo --from-asset pathUSD --to-asset USDC.e --amount 1000000 --from-address 0xYourEOA --results-only

# Execute a planned action (requires signer)
export DEFI_PRIVATE_KEY_FILE=~/.config/defi/key.hex
defi swap submit --action-id <action_id> --results-only

# Structured input for agents
defi lend supply plan --input-json '{"provider":"aave","chain":"1","asset":"USDC","amount":"1000000","from_address":"0xYourEOA"}' --results-only

# Inspect actions
defi actions list --results-only
defi actions estimate --action-id <action_id> --results-only
```

### Execution command surface

- `swap plan|submit|status` (Tempo, TaikoSwap)
- `bridge plan|submit|status` (Across, LiFi)
- `lend supply|withdraw|borrow|repay plan|submit|status` (Aave, Morpho)
- `yield deposit|withdraw plan|submit|status` (Aave, Morpho)
- `rewards claim|compound plan|submit|status` (Aave)
- `approvals plan|submit|status`
- `transfer plan|submit|status`
- `actions list|show|estimate`

All `plan` commands support `--rpc-url` to override chain default RPCs.
`plan` and `submit` accept `--input-json` / `--input-file` for structured input; explicit flags override JSON values.
`--providers` flags accept provider names from `defi providers list` (e.g. `aave,morpho,kamino`).

### More quote examples

```bash
defi swap quote --provider 1inch --chain 1 --from-asset USDC --to-asset DAI --amount 1000000 --results-only   # requires DEFI_1INCH_API_KEY
defi swap quote --provider uniswap --chain 1 --from-asset USDC --to-asset DAI --amount 1000000 --from-address 0xYourEOA --results-only  # requires DEFI_UNISWAP_API_KEY
defi swap quote --provider uniswap --chain 1 --from-asset USDC --to-asset DAI --type exact-output --amount-out 1000000000000000000 --from-address 0xYourEOA --results-only
defi swap quote --provider tempo --chain tempo --from-asset pathUSD --to-asset USDC.e --type exact-output --amount-out 1000000 --results-only
defi bridge quote --provider lifi --from 1 --to 8453 --asset USDC --amount 1000000 --from-amount-for-gas 100000 --results-only
```

## Command API Key Requirements

Most commands do not require provider API keys.

When a provider requires authentication, bring your own key:

- `defi swap quote --provider 1inch` -> `DEFI_1INCH_API_KEY`
- `defi swap quote --provider uniswap` -> `DEFI_UNISWAP_API_KEY`
- `defi chains assets` -> `DEFI_DEFILLAMA_API_KEY`
- `defi bridge list` -> `DEFI_DEFILLAMA_API_KEY`
- `defi bridge details` -> `DEFI_DEFILLAMA_API_KEY`
- `defi swap quote --provider tempo` -> no API key required
- `defi swap quote --provider taikoswap` -> no API key required

`defi providers list` includes both provider-level key metadata and capability-level key metadata (`capability_auth`).

## API Keys

- `DEFI_1INCH_API_KEY` (required for `swap quote --provider 1inch`)
- `DEFI_UNISWAP_API_KEY` (required for `swap quote --provider uniswap`)
- `DEFI_DEFILLAMA_API_KEY` (required for `chains assets`, `bridge list`, and `bridge details`)

Configure keys with environment variables (recommended):

```bash
export DEFI_1INCH_API_KEY=...
export DEFI_UNISWAP_API_KEY=...
export DEFI_DEFILLAMA_API_KEY=...
```

For persistent shell setup, add exports to your shell profile (for example `~/.zshrc`).

If a keyed provider is used without a key, CLI exits with code `10`.

## Execution Signer Inputs (Submit Commands)

Execution `submit` commands support a local key signer and (on Tempo) an agent wallet signer.

Signer selection:

- `--signer tempo` — use the Tempo CLI agent wallet (`tempo wallet -j whoami`); requires the Tempo CLI installed and configured with delegated access keys.
- Local key signer (default) — uses the key input precedence below.

Key input precedence:

- `--private-key` (hex string, one-off override; less safe)
- env/file/keystore inputs below (when `--private-key` is not provided)

Key env/file inputs (in precedence order when `--key-source auto` and `--private-key` is unset):

- `DEFI_PRIVATE_KEY` (hex string, supported but less safe)
- `DEFI_PRIVATE_KEY_FILE` (preferred explicit key-file path)
- default key file: `~/.config/defi/key.hex` (or `$XDG_CONFIG_HOME/defi/key.hex` when `XDG_CONFIG_HOME` is set)
- `DEFI_KEYSTORE_PATH` + (`DEFI_KEYSTORE_PASSWORD` or `DEFI_KEYSTORE_PASSWORD_FILE`)

You can force source selection with `--key-source env|file|keystore`.

`submit` commands support optional `--from-address` as an explicit signer-address guard.

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
execution:
  actions_path: ~/.cache/defi/actions.db
  actions_lock_path: ~/.cache/defi/actions.lock
providers:
  uniswap:
    api_key_env: DEFI_UNISWAP_API_KEY
```

`swap quote` (on-chain quote providers) and execution `plan` `--rpc-url` flags override chain default RPCs for that invocation.
`submit`/`status` commands use stored per-step RPC URLs from the persisted action.

## Execution Metadata Locations (Implementers)

- `internal/registry`: canonical execution endpoints/contracts/ABI fragments and default chain RPC map used when no `--rpc-url` is provided.
- `internal/providers/*/client.go`: provider quote/read API base URLs and external source URLs.
- `internal/id/id.go`: bootstrap token symbol/address registry used for deterministic symbol parsing.

## Cache Policy

- Command TTLs are fixed in code (`chains/protocols/chains assets`: `5m`, `lend markets`: `60s`, `lend rates`: `30s`, `lend positions`: `30s`, `yield opportunities`: `60s`, `yield positions`: `30s`, `yield history`: `5m`, `bridge/swap quotes`: `15s`).
- Cache entries are served directly only while fresh (`age <= ttl`).
- After TTL expiry, the CLI fetches provider data immediately.
- `cache.max_stale` / `--max-stale` is only a temporary provider-failure fallback window (currently `unavailable` / `rate_limited`).
- If fallback is disabled (`--no-stale` or `--max-stale 0s`) or stale data exceeds the budget, the CLI exits with code `14`.
- Metadata commands (`version`, `schema`, `providers list`) bypass cache initialization.
- Execution commands (`swap|bridge|approvals|transfer|lend|yield|rewards ... plan|submit|status`, `actions list|show|estimate`) bypass cache reads/writes.

## Caveats

### Data

- Morpho can surface extreme APY values on very small markets; use `--min-tvl-usd` when ranking.
- `yield opportunities` returns `apy_total`, `tvl_usd`, `liquidity_usd`, and `backing_assets` (objective metrics only).
- `yield history --metrics` supports `apy_total` and `tvl_usd`; Aave currently supports `apy_total` only. Use `--window` for Aave history.
- `lend positions --type all` returns disjoint rows: `supply`, `collateral`, and `borrow`.
- For chains without bootstrap symbol entries, pass token address or CAIP-19 for deterministic resolution.
- `--chain` supports CAIP-2, numeric IDs, and aliases (`tempo`, `presto`, `moderato`, `tempo devnet`, `mantle`, `megaeth`, `taiko`, `gnosis`, `linea`, `zksync`, `hyperevm`, `monad`, `citrea`, and more).

### Quotes

- `swap quote --type` defaults to `exact-input`; `exact-output` is currently supported by `uniswap` and `tempo` (`--amount-out`/`--amount-out-decimal`).
- Uniswap requires `--from-address`; `--slippage-pct` is optional (default: provider auto).
- Tempo DEX currently supports USD-denominated TIP-20 swaps only and auto-routes supported pairs through quote-token relationships; non-USD assets such as `EURC.e` are rejected.
- Tempo swap execution settles to the sender only; omit `--recipient` or keep it equal to `--from-address`.
- `actions estimate` returns fee-token-denominated estimates for Tempo actions (includes `fee_unit` and `fee_token` fields instead of native-gas EIP-1559 pricing).
- `fibrous` currently supports `base`, `hyperevm`, and `citrea`.
- Bungee dedicated backend requires both `DEFI_BUNGEE_API_KEY` and `DEFI_BUNGEE_AFFILIATE`.

### Execution

- `yield` and `lend` are split by intent: `yield` for passive deposits/withdrawals, `lend` for loan lifecycle.
- Morpho: `yield deposit|withdraw` targets vaults (`--vault-address`), `lend` targets markets (`--market-id`).
- Aave execution auto-resolves pool addresses on Ethereum, Optimism, Polygon, Base, Arbitrum, and Avalanche; use `--pool-address` on other chains.
- Bridge execution waits for destination settlement; adjust `--step-timeout` for slower routes.
- Pre-sign checks enforce bounded ERC-20 approvals by default; use `--allow-max-approval` to opt in to larger approvals.
- Bridge pre-sign checks validate settlement endpoints; use `--unsafe-provider-tx` to bypass.
- All `submit` commands broadcast signed transactions.
- `--signer tempo` enables agent wallet support via the Tempo CLI (`tempo wallet -j whoami`), with delegated access keys, spending limits, and expiry checks.
- `--provider` is required for multi-provider flows (no implicit defaults).

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
- `20`: action plan validation failed
- `21`: action simulation failed
- `22`: execution rejected by policy
- `23`: action timed out while waiting for confirmation
- `24`: signer unavailable or signing failed

## Development

### Folder Structure

```text
cmd/
  defi/main.go                    # CLI entrypoint

internal/
  app/runner.go                   # command wiring, routing, cache flow
  providers/                      # external adapters
    aave/ morpho/                 # lending + yield (read + execution)
    defillama/                    # normalization + fallback + bridge analytics
    across/ lifi/                 # bridge quotes + lifi execution planning
    oneinch/ uniswap/ taikoswap/  # swap (quote + taikoswap execution planning)
    types.go                      # provider interfaces
  execution/                      # action store + planner helpers + signer + executor
  registry/                       # canonical execution endpoints/contracts/ABI fragments
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
.github/workflows/nightly-execution-smoke.yml # nightly live execution planning smoke
docs/                             # Mintlify docs site (docs.json + MDX pages)
AGENTS.md                         # contributor guide for agents
```
### Testing

```bash
go test ./...
go test -race ./...
go vet ./...
bash scripts/nightly_execution_smoke.sh
```

### Documentation Site (Mintlify)

The `docs/` directory contains a Mintlify docs site (`docs.json` + `.mdx` pages).

```bash
cd docs
npx --yes mint@4.2.378 dev --no-open        # local preview
npx --yes mint@4.2.378 validate             # validate before publishing
npx --yes mint@4.2.378 broken-links
npx --yes mint@4.2.378 a11y
```

Production docs deploy from the `docs-live` branch, which the release workflow syncs on stable (non-prerelease) tags.
