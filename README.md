# defi-cli

<p align="center">
  <img src="assets/logo.png" alt="defi-cli logo" width="600" />
</p>

Query and act on DeFi lending, yield, bridge, and swap — across protocols and chains, from a single CLI.

Built for AI agents and scripts. Stable JSON output, canonical identifiers (CAIP-2/CAIP-19), and deterministic exit codes make it easy to pipe into any workflow.

## Features

- **Lending** — query markets/rates from Aave/Morpho/Kamino/Moonwell, account positions from Aave/Morpho/Moonwell, and execute loan actions (`lend supply|withdraw|borrow|repay`).
- **Yield** — compare opportunities, query positions, fetch historical series, and execute deposit/withdraw flows (Aave, Morpho, Moonwell).
- **Bridging** — get cross-chain quotes (Across, LiFi, Bungee), bridge analytics, and execute bridge plans (Across, LiFi).
- **Swapping** — get swap quotes (1inch, Uniswap, Jupiter, Tempo, TaikoSwap, Fibrous, Bungee) and execute swap plans (Tempo with native type 0x76 transactions and batched calls, TaikoSwap).
- **Approvals, transfers & rewards** — ERC-20 approvals/transfers and Aave rewards claim/compound flows.
- **Wallet** — query native and ERC-20 token balances on any supported EVM chain (no API key required).
- **Chains & protocols** — browse top chains by TVL, inspect chain TVL by asset, query live gas prices, discover protocols, track stablecoin market caps, resolve asset identifiers.
- **Automation-friendly** — JSON-first output, field selection (`--select`), structured JSON/file input (`--input-json`, `--input-file`), and a machine-readable schema export with required flags, enums, input constraints, auth, and request/response metadata.

## Documentation Site (Mintlify)

This repo includes a dedicated Mintlify docs site under [`docs/`](docs) (`docs/docs.json` + `.mdx` pages).

Preview locally:

```bash
cd docs
npx --yes mint@4.2.378 dev --no-open
```

Validate before publishing:

```bash
cd docs
npx --yes mint@4.2.378 validate
npx --yes mint@4.2.378 broken-links
npx --yes mint@4.2.378 a11y
```

Production docs deployment should target `docs-live` in Mintlify Git settings. The release workflow syncs `docs-live` on stable release tags (non-prerelease) so live docs align with main-channel binaries.

## Install

### 1) Quick install (macOS/Linux)

Installs the latest tagged release from GitHub:

```bash
curl -fsSL https://raw.githubusercontent.com/ggonzalez94/defi-cli/main/scripts/install.sh | sh
```

Install a specific version (accepted: `latest`, `stable`, `vX.Y.Z`, `X.Y.Z`):

```bash
curl -fsSL https://raw.githubusercontent.com/ggonzalez94/defi-cli/main/scripts/install.sh | sh -s -- v0.5.0
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

## Agent Skill (Claude Code / Cursor / Codex)

If your AI agent uses [Claude Code](https://claude.com/claude-code), [Cursor](https://cursor.com), or another agent that supports [open skills](https://agentskills.io), install the bundled skill to teach the agent how to use defi-cli correctly — including exact field names, execution patterns, and common gotchas:

```bash
npx skills add ggonzalez94/defi-cli
```

Or install manually: copy `skills/defi-cli/` from this repo to `~/.claude/skills/defi-cli/`.

## Signing Backends

Execution commands (`plan`, `submit`, `status`) support two signing backends:

### OWS (recommended)

[Open Wallet Standard (OWS)](https://docs.openwallet.sh/) keeps keys encrypted at rest with built-in policy controls.

**Setup:**

```bash
npm install -g @open-wallet-standard/core
ows wallet create --name agent-treasury
export DEFI_OWS_TOKEN=$(ows token create --wallet agent-treasury --ttl 24h)
```

**Plan and submit:**

```bash
defi lend supply plan --provider aave --chain 1 --asset USDC --amount 1000000 --wallet agent-treasury
defi lend supply submit --action-id <action_id>
```

### Local signer

Sign directly with a local private key — no external tooling required.

**Plan and submit:**

```bash
defi lend supply plan --provider aave --chain 1 --asset USDC --amount 1000000 --from-address 0xYourEOA
export DEFI_PRIVATE_KEY_FILE=~/.config/defi/key.hex
defi lend supply submit --action-id <action_id>
```

> Read-only commands (`markets`, `positions`, `quote`, etc.) do not require either backend.

## Quick Start

### Read: query markets and quotes

```bash
defi providers list --results-only
defi chains list --results-only --select slug,caip2,namespace
defi chains gas --chain 1 --results-only
defi chains gas --chain 1,10,137,8453,42161 --results-only   # multi-chain batch
defi chains top --limit 10 --results-only --select rank,chain,tvl_usd
defi chains assets --chain 1 --asset USDC --results-only # Requires DEFI_DEFILLAMA_API_KEY
defi protocols fees --limit 10 --results-only --select rank,protocol,fees_24h_usd,change_1d_pct
defi protocols revenue --limit 10 --results-only --select rank,protocol,revenue_24h_usd,change_1d_pct
defi dexes volume --limit 10 --results-only --select rank,protocol,volume_24h_usd,change_1d_pct
defi dexes volume --chain Arbitrum --limit 5 --results-only  # Filter DEXes active on Arbitrum
defi stablecoins top --limit 10 --results-only --select rank,symbol,circulating_usd,price
defi stablecoins chains --limit 10 --results-only --select rank,chain,circulating_usd
defi wallet balance --chain 1 --address 0xYourEOA --results-only
defi wallet balance --chain base --address 0xYourEOA --asset USDC --results-only
defi assets resolve --chain base --symbol USDC --results-only
defi lend markets --provider aave --chain 1 --asset USDC --results-only
defi lend positions --provider aave --chain 1 --address 0xYourEOA --type all --results-only
defi yield opportunities --chain 1 --asset USDC --providers aave,morpho --limit 10 --results-only
defi yield history --chain 1 --asset USDC --providers aave --metrics apy_total --interval day --window 7d --limit 1 --results-only
defi bridge quote --provider across --from 1 --to 8453 --asset USDC --amount 1000000 --results-only
defi swap quote --provider tempo --chain tempo --from-asset pathUSD --to-asset USDC.e --amount 1000000 --results-only
```

### Act: plan and execute transactions

```bash
# Plan standard EVM actions with an OWS wallet reference
defi lend supply plan --provider aave --chain 1 --asset USDC --amount 1000000 --wallet agent-treasury --results-only
defi bridge plan --provider across --from 1 --to 8453 --asset USDC --amount 1000000 --wallet agent-treasury --results-only
defi swap plan --provider taikoswap --chain taiko --from-asset USDC --to-asset WETH --amount 1000000 --wallet agent-treasury --results-only

# Tempo remains the explicit exception and still plans with --from-address
defi swap plan --provider tempo --chain tempo --from-asset pathUSD --to-asset USDC.e --amount 1000000 --from-address 0xYourEOA --results-only

# Execute a wallet-backed action
export DEFI_OWS_TOKEN=...
defi lend supply submit --action-id <action_id> --results-only

# Local signer: plan and submit with a local private key
defi lend supply plan --provider aave --chain 1 --asset USDC --amount 1000000 --from-address 0xYourEOA --results-only
export DEFI_PRIVATE_KEY_FILE=~/.config/defi/key.hex
defi lend supply submit --action-id <action_id> --results-only

# Structured input for agents
defi lend supply plan --input-json '{"provider":"aave","chain":"1","asset":"USDC","amount":"1000000","wallet":"agent-treasury"}' --results-only

# Inspect actions
defi actions list --results-only
defi actions estimate --action-id <action_id> --results-only
```

### Execution command surface

- `swap plan|submit|status` (Tempo, TaikoSwap)
- `bridge plan|submit|status` (Across, LiFi)
- `lend supply|withdraw|borrow|repay plan|submit|status` (Aave, Morpho, Moonwell)
- `yield deposit|withdraw plan|submit|status` (Aave, Morpho, Moonwell)
- `rewards claim|compound plan|submit|status` (Aave)
- `approvals plan|submit|status`
- `transfer plan|submit|status`
- `actions list|show|estimate`

All `plan` commands support `--rpc-url` to override chain default RPCs.
`plan` and `submit` accept `--input-json` / `--input-file` for structured input; explicit flags override JSON values.
`--providers` flags accept provider names from `defi providers list` (e.g. `aave,morpho,kamino,moonwell`).

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

## Execution Auth (Submit Commands)

Two signing backends are supported. The backend is determined at plan time and persisted with the action.

**OWS (recommended):**

- Plan with `--wallet` to create an `ows`-backed action.
- Submit reads the persisted `wallet_id` and requires `DEFI_OWS_TOKEN`.
- Does not accept local signer flags during submit.

```bash
export DEFI_OWS_TOKEN=...
defi bridge submit --action-id <action_id> --results-only
```

**Local signer:**

- Plan with `--from-address` to create a `legacy_local`-backed action.
- Submit uses local key inputs (`--private-key`, env vars, keystore).

Key input precedence (when `--key-source auto` and `--private-key` is unset):

1. `DEFI_PRIVATE_KEY` (hex string)
2. `DEFI_PRIVATE_KEY_FILE` (key file path)
3. `~/.config/defi/key.hex` (or `$XDG_CONFIG_HOME/defi/key.hex`)
4. `DEFI_KEYSTORE_PATH` + password env

Force source with `--key-source env|file|keystore`.

**Tempo exception:**

- Tempo swap planning uses `--from-address` (OWS does not cover Tempo-native execution yet).
- Tempo submit uses `--signer tempo`.

`submit` commands support optional `--from-address` as an explicit sender-address guard.

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

- Command TTLs are fixed in code (`chains/protocols/stablecoins/chains assets`: `5m`, `lend markets`: `60s`, `lend rates`: `30s`, `lend positions`: `30s`, `yield opportunities`: `60s`, `yield positions`: `30s`, `yield history`: `5m`, `wallet balance`: `15s`, `bridge/swap quotes`: `15s`).
- Cache entries are served directly only while fresh (`age <= ttl`).
- After TTL expiry, the CLI fetches provider data immediately.
- `cache.max_stale` / `--max-stale` is only a temporary provider-failure fallback window (currently `unavailable` / `rate_limited`).
- If fallback is disabled (`--no-stale` or `--max-stale 0s`) or stale data exceeds the budget, the CLI exits with code `14`.
- Metadata commands (`version`, `schema`, `providers list`, `chains list`) bypass cache initialization.
- Execution commands (`swap|bridge|approvals|transfer|lend|yield|rewards ... plan|submit|status`, `actions list|show|estimate`) bypass cache reads/writes.
- Expired entries (past TTL + `max_stale`) are automatically pruned on startup to prevent unbounded growth.

## Caveats

### Data

- `wallet balance` currently supports EVM chains only; Solana is not yet supported.
- `wallet balance` uses `eth_getBalance` for native tokens and ERC-20 `balanceOf` for tokens; it does not query pending/unconfirmed balances.
- Morpho can surface extreme APY values on very small markets; use `--min-tvl-usd` when ranking.
- `yield opportunities` returns `apy_total`, `tvl_usd`, `liquidity_usd`, and `backing_assets` (objective metrics only).
- `yield history --metrics` supports `apy_total` and `tvl_usd`; Aave currently supports `apy_total` only. Use `--window` for Aave history.
- `lend positions --type all` returns disjoint rows: `supply`, `collateral`, and `borrow`.
- For chains without bootstrap symbol entries, pass token address or CAIP-19 for deterministic resolution.
- `--chain` supports CAIP-2, numeric IDs, and aliases (`tempo`, `presto`, `moderato`, `tempo devnet`, `mantle`, `megaeth`, `taiko`, `gnosis`, `linea`, `zksync`, `hyperevm`, `monad`, `citrea`, and more).
- `chains assets` requires `DEFI_DEFILLAMA_API_KEY`; `bridge list`/`bridge details` also require it; quote providers (`across`, `lifi`) do not.
- `protocols fees` rankings are sorted by 24h fees descending; protocols with null or zero 24h fees are excluded.
- `protocols revenue` rankings are sorted by 24h revenue descending; protocols with null or zero 24h revenue are excluded. Revenue represents the portion of fees retained by the protocol (not LPs/validators).
- `dexes volume` rankings are sorted by 24h volume descending; DEXes with null or zero 24h volume are excluded. `--chain` filters by chain presence.
- `chains gas` returns live EVM gas prices via RPC; it is EVM-only and bypasses cache. Use `--rpc-url` to override the default chain RPC. Pass comma-separated chains (e.g. `--chain 1,10,8453`) for parallel multi-chain queries; `--rpc-url` is only allowed with a single chain.

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
- Moonwell execution targets mToken contracts (Compound v2 style) on Base and Optimism; use `--pool-address` to specify the mToken directly or let auto-resolution match by underlying asset. The mWETH market auto-unwraps to native ETH on borrow/withdraw; the planner uses ERC-20 paths, so callers must handle ETH/WETH wrapping externally.
- Bridge execution waits for destination settlement; adjust `--step-timeout` for slower routes.
- Pre-sign checks enforce bounded ERC-20 approvals by default; use `--allow-max-approval` to opt in to larger approvals.
- Bridge pre-sign checks validate settlement endpoints; use `--unsafe-provider-tx` to bypass.
- All `submit` commands broadcast signed transactions.
- `--signer tempo` enables agent wallet support via the Tempo CLI (`tempo wallet -j whoami`), with delegated access keys and expiry checks.
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
    aave/ morpho/ moonwell/       # lending + yield (read + execution)
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
