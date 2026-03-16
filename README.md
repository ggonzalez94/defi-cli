# defi-cli

<p align="center">
  <img src="assets/logo.png" alt="defi-cli logo" width="600" />
</p>

Query lending rates, compare yield, get bridge and swap quotes — across protocols and chains, from a single CLI.

Built for AI agents and scripts. Stable JSON output, canonical identifiers (CAIP-2/CAIP-19), and deterministic exit codes make it easy to pipe into any workflow.

## Features

- **Lending** — query markets/rates from Aave/Morpho/Kamino and account positions from Aave/Morpho, plus execute Aave/Morpho loan actions (`lend supply|withdraw|borrow|repay`).
- **Yield** — compare opportunities, query account yield positions, fetch historical yield/TVL series, and execute yield deposit/withdraw flows.
- **Bridging** — get cross-chain quotes (Across, LiFi, Bungee), bridge analytics (volume, chain breakdown), and execute Across/LiFi bridge plans.
- **Swapping** — get swap quotes (1inch, Uniswap, TaikoSwap) and execute TaikoSwap plans on-chain.
- **Approvals, transfers & rewards** — create and execute ERC-20 approvals/transfers plus Aave rewards claim/compound flows.
- **Chains & protocols** — browse top chains by TVL, inspect chain TVL by asset, query live gas prices, discover protocols, track stablecoin market caps, resolve asset identifiers.
- **Automation-friendly** — JSON-first output, field selection (`--select`), structured JSON/file input for mutation workflows, and a machine-readable schema export with required flags, enums, auth, and request/response metadata.

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
curl -fsSL https://raw.githubusercontent.com/ggonzalez94/defi-cli/main/scripts/install.sh | sh -s -- v0.3.1
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
defi assets resolve --chain base --symbol USDC --results-only
defi lend markets --provider aave --chain 1 --asset USDC --results-only
defi lend rates --provider morpho --chain 1 --asset USDC --results-only
defi lend positions --provider aave --chain 1 --address 0xYourEOA --type all --limit 20 --results-only
defi yield opportunities --chain base --asset USDC --limit 20 --results-only
defi yield positions --chain 1 --address 0xYourEOA --providers aave,morpho --limit 20 --results-only
defi yield opportunities --chain 1 --asset USDC --providers aave,morpho --limit 10 --results-only
defi yield history --chain 1 --asset USDC --providers aave,morpho --metrics apy_total,tvl_usd --interval day --window 7d --limit 1 --results-only
defi bridge list --limit 10 --results-only # Requires DEFI_DEFILLAMA_API_KEY
defi bridge details --bridge layerzero --results-only # Requires DEFI_DEFILLAMA_API_KEY
defi bridge quote --provider across --from 1 --to 8453 --asset USDC --amount 1000000 --results-only
defi bridge quote --provider lifi --from 1 --to 8453 --asset USDC --amount 1000000 --from-amount-for-gas 100000 --results-only
defi swap quote --provider taikoswap --chain taiko --from-asset USDC --to-asset WETH --amount 1000000 --results-only
defi swap plan --provider taikoswap --chain taiko --from-asset USDC --to-asset WETH --amount 1000000 --from-address 0xYourEOA --results-only
defi bridge plan --provider lifi --from 1 --to 8453 --asset USDC --amount 1000000 --from-address 0xYourEOA --from-amount-for-gas 100000 --results-only
defi bridge plan --provider across --from 1 --to 8453 --asset USDC --amount 1000000 --from-address 0xYourEOA --results-only
defi lend supply plan --provider aave --chain 1 --asset USDC --amount 1000000 --from-address 0xYourEOA --results-only
defi lend supply plan --provider morpho --chain 1 --asset USDC --market-id 0x... --amount 1000000 --from-address 0xYourEOA --results-only
defi yield deposit plan --provider morpho --chain 1 --asset USDC --vault-address 0x... --amount 1000000 --from-address 0xYourEOA --results-only
defi rewards claim plan --provider aave --chain 1 --from-address 0xYourEOA --assets 0x... --reward-token 0x... --results-only
defi approvals plan --chain taiko --asset USDC --spender 0xSpender --amount 1000000 --from-address 0xYourEOA --results-only
defi transfer plan --chain taiko --asset USDC --amount 1000000 --from-address 0xYourEOA --recipient 0xRecipient --results-only
defi transfer plan --input-json '{"chain":"taiko","asset":"USDC","amount":"1000000","from_address":"0xYourEOA","recipient":"0xRecipient"}' --results-only
defi swap status --action-id <action_id> --results-only
defi actions list --results-only
defi actions estimate --action-id <action_id> --results-only
```

`yield opportunities --providers`, `yield positions --providers`, and `yield history --providers` accept provider names from `defi providers list` (for example `aave,morpho,kamino`).

Bridge quote examples:

```bash
defi bridge quote --provider across --from 1 --to 8453 --asset USDC --amount 1000000 --results-only
defi bridge quote --provider lifi --from 1 --to 8453 --asset USDC --amount 1000000 --results-only
defi bridge quote --provider lifi --from 1 --to 8453 --asset USDC --amount 1000000 --from-amount-for-gas 100000 --results-only
```

Swap quote examples:

```bash
export DEFI_1INCH_API_KEY=...
export DEFI_UNISWAP_API_KEY=...
defi swap quote --provider 1inch --chain 1 --from-asset USDC --to-asset DAI --amount 1000000 --results-only
defi swap quote --provider taikoswap --chain taiko --from-asset USDC --to-asset WETH --amount 1000000 --results-only
defi swap quote --provider uniswap --chain 1 --from-asset USDC --to-asset DAI --amount 1000000 --from-address 0xYourEOA --results-only
# Exact-output on Uniswap
defi swap quote --provider uniswap --chain 1 --from-asset USDC --to-asset DAI --type exact-output --amount-out 1000000000000000000 --from-address 0xYourEOA --results-only
# Optional manual slippage override for Uniswap (percent)
defi swap quote --provider uniswap --chain 1 --from-asset USDC --to-asset DAI --amount 1000000 --slippage-pct 1.0 --from-address 0xYourEOA --results-only
defi swap quote --provider bungee --chain hyperevm --from-asset USDC --to-asset WHYPE --amount 5000000 --results-only
```

Swap execution flow (local signer):

```bash
export DEFI_PRIVATE_KEY_FILE=~/.config/defi/key.hex
# or pass --private-key 0x... on submit commands for one-off usage

# 1) Plan only
defi swap plan \
  --provider taikoswap \
  --chain taiko \
  --from-asset USDC \
  --to-asset WETH \
  --amount 1000000 \
  --from-address 0xYourEOA \
  --results-only

# 2) Execute the saved action
defi swap submit \
  --action-id <action_id> \
  --results-only
```

Execution `plan` and `submit` commands also accept structured input for agents:

```bash
defi bridge plan --input-file ./bridge-plan.json --results-only
defi lend supply plan --input-json '{"provider":"aave","chain":"1","asset":"USDC","amount":"1000000","from_address":"0xYourEOA"}' --results-only
defi swap submit --input-json '{"action_id":"<action_id>","from_address":"0xYourEOA"}' --results-only
```

For structured input, flags still win over JSON/file values when both are provided.

`swap quote` (on-chain quote providers) and execution `plan` commands support optional `--rpc-url` overrides (`swap`, `bridge`, `approvals`, `transfer`, `lend`, `yield`, `rewards`).
For bridge flows, `--rpc-url` applies to the source-chain execution RPC.

Execution command surface:

- `swap plan|submit|status`
- `bridge plan|submit|status` (provider: `across|lifi`)
- `approvals plan|submit|status`
- `transfer plan|submit|status`
- `lend supply|withdraw|borrow|repay plan|submit|status` (provider: `aave|morpho`)
- `yield deposit|withdraw plan|submit|status` (provider: `aave|morpho`)
- `rewards claim|compound plan|submit|status` (provider: `aave`)
- `actions list|show|estimate`

Schema output is designed for agent discovery. `defi schema` now includes inherited flags plus command/flag metadata such as `required`, `enum`, `format`, `input_modes`, `auth`, and request/response structure hints.

## Command API Key Requirements

Most commands do not require provider API keys.

When a provider requires authentication, bring your own key:

- `defi swap quote --provider 1inch` -> `DEFI_1INCH_API_KEY`
- `defi swap quote --provider uniswap` -> `DEFI_UNISWAP_API_KEY`
- `defi chains assets` -> `DEFI_DEFILLAMA_API_KEY`
- `defi bridge list` -> `DEFI_DEFILLAMA_API_KEY`
- `defi bridge details` -> `DEFI_DEFILLAMA_API_KEY`
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

Execution `submit` commands currently support a local key signer.

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

- Command TTLs are fixed in code (`chains/protocols/stablecoins/chains assets`: `5m`, `lend markets`: `60s`, `lend rates`: `30s`, `lend positions`: `30s`, `yield opportunities`: `60s`, `yield positions`: `30s`, `yield history`: `5m`, `bridge/swap quotes`: `15s`).
- Cache entries are served directly only while fresh (`age <= ttl`).
- After TTL expiry, the CLI fetches provider data immediately.
- `cache.max_stale` / `--max-stale` is only a temporary provider-failure fallback window (currently `unavailable` / `rate_limited`).
- If fallback is disabled (`--no-stale` or `--max-stale 0s`) or stale data exceeds the budget, the CLI exits with code `14`.
- Metadata commands (`version`, `schema`, `providers list`, `chains list`) bypass cache initialization.
- Execution commands (`swap|bridge|approvals|transfer|lend|yield|rewards ... plan|submit|status`, `actions list|show|estimate`) bypass cache reads/writes.

## Caveats

- Morpho can surface extreme APY values on very small markets. Prefer `--min-tvl-usd` when ranking yield.
- `yield` and `lend` represent distinct user intents: use `yield` for passive deposit/withdraw flows and `lend` for market loan lifecycle (`supply|withdraw|borrow|repay`).
- Morpho execution surfaces are intentionally split: `yield deposit|withdraw` target Morpho vaults (`--vault-address`), while `lend ...` targets Morpho Blue markets (`--market-id`).
- Aave `yield deposit|withdraw` routes to the same reserve mechanics as Aave lend supply/withdraw with yield-intent command semantics.
- `yield opportunities` returns objective metrics and composition data: `apy_total`, `tvl_usd`, `liquidity_usd`, and full `backing_assets` (subjective `risk_*`/`score` fields were removed).
- `liquidity_usd` is provider-sourced available liquidity and is intentionally distinct from `tvl_usd` (total supplied/managed value).
- `actions estimate` reports source-chain EVM step gas/fee projections from planned calldata (`eth_estimateGas` + EIP-1559); it does not add destination settlement gas unless that transaction is an explicit action step.
- `yield history --metrics` supports `apy_total` and `tvl_usd`; Aave currently supports `apy_total` only.
- Aave historical windows are lookback-based and effectively end near current time; use `--window` for Aave-friendly history requests.
- `chains assets` requires `DEFI_DEFILLAMA_API_KEY` because DefiLlama chain asset TVL is key-gated.
- `bridge list` and `bridge details` require `DEFI_DEFILLAMA_API_KEY`; quote providers (`across`, `lifi`) do not.
- Category rankings from `protocols categories` are deterministic and sorted by `tvl_usd`, then protocol count, then name.
- `protocols fees` rankings are sorted by 24h fees descending; protocols with null or zero 24h fees are excluded.
- `protocols revenue` rankings are sorted by 24h revenue descending; protocols with null or zero 24h revenue are excluded. Revenue represents the portion of fees retained by the protocol (not LPs/validators).
- `dexes volume` rankings are sorted by 24h volume descending; DEXes with null or zero 24h volume are excluded. `--chain` filters by chain presence (e.g. `--chain Ethereum`).
- `--chain` normalization supports additional aliases/IDs including `mantle`, `megaeth`/`mega eth`/`mega-eth`, `ink`, `scroll`, `berachain`, `gnosis`/`xdai`, `linea`, `sonic`, `blast`, `fraxtal`, `world-chain`, `celo`, `taiko`/`taiko alethia`, `taiko hoodi`/`hoodi`, `zksync`, `hyperevm`/`hyper evm`/`hyper-evm`, `monad`, and `citrea`.
- Bungee Auto-mode quote coverage is chain+token dependent; unsupported pairs return provider errors even when chain normalization succeeds.
- Bungee quote requests use deterministic placeholder sender/receiver addresses for quote-only resolution (`0x000...001`).
- Bungee dedicated backend routing only activates when both `DEFI_BUNGEE_API_KEY` and `DEFI_BUNGEE_AFFILIATE` are set; if either is missing, requests use the public backend.
- Swap quote type defaults to `--type exact-input`; use `--type exact-output` with `--amount-out`/`--amount-out-decimal` when supported by the provider.
- Exact-output swap quotes currently support `--provider uniswap` only; Solana exact-output is currently unsupported.
- Uniswap supports both `exact-input` and `exact-output`; 1inch/Jupiter/Fibrous/Bungee currently support `exact-input` only.
- Uniswap quote requests require `--from-address` as the `swapper`; provider auto slippage is used by default, and `--slippage-pct` sets a manual max slippage percent.
- MegaETH bootstrap symbol parsing currently supports `MEGA`, `WETH`, and `USDT` (`USDT` maps to the chain's `USDT0` contract address). Official Mega token list currently has no Ethereum L1 `MEGA` token entry.
- `fibrous` swap quotes are currently limited to `base`, `hyperevm`, and `citrea` (`monad` temporarily disabled due unstable route responses).
- For chains without bootstrap symbol entries, pass token address or CAIP-19 via `--asset`/`--from-asset`/`--to-asset` for deterministic resolution.
- For `lend`/`yield`, unresolved asset symbols skip DefiLlama-based symbol matching and may disable fallback/provider selection to avoid unsafe broad matches.
- `lend positions --type all` returns disjoint rows by intent: `supply` (non-collateralized supplied balance), `collateral` (posted collateral), and `borrow` (debt).
- Swap execution currently supports TaikoSwap only.
- Bridge execution currently supports Across and LiFi.
- Transfer execution supports native ERC-20 `transfer(...)` actions on EVM chains.
- Lend execution supports Aave and Morpho (`--market-id` required for Morpho).
- Yield execution supports Aave and Morpho (`--vault-address` required for Morpho).
- Rewards execution currently supports Aave only.
- Aave execution resolves pool addresses automatically on Ethereum, Optimism, Polygon, Base, Arbitrum, and Avalanche; use `--pool-address` / `--pool-address-provider` on unsupported chains.
- LiFi bridge execution now waits for destination settlement status before marking the bridge step complete; adjust `--step-timeout` for slower routes.
- Across bridge execution now waits for destination settlement status before marking the bridge step complete; adjust `--step-timeout` for slower routes.
- `--step-timeout` applies to each bridge wait stage (receipt and settlement polling); execution wait budget is derived from `--step-timeout` and remaining action stages.
- LiFi bridge quote/plan support `--from-amount-for-gas` (source token base units reserved for destination native gas top-up).
- Execution pre-sign checks enforce bounded ERC-20 approvals (`approve <= planned input amount`) by default; use `--allow-max-approval` when a route requires larger approvals.
- Transfer execution pre-sign checks validate ERC-20 `transfer(to,amount)` calldata, recipient, amount, and token target invariants before signing.
- Swap execution validates `--from-address` and `--recipient` as EVM hex addresses before planning transactions.
- Bridge execution pre-sign checks validate settlement provider metadata and known settlement endpoint URLs for Across/LiFi; use `--unsafe-provider-tx` to bypass these guardrails.
- All `submit` execution commands will broadcast signed transactions.
- Rewards `--assets` expects comma-separated on-chain addresses used by Aave incentives contracts.
- `chains gas` returns live EVM gas prices via RPC; it is EVM-only and bypasses cache. Use `--rpc-url` to override the default chain RPC. Pass comma-separated chains (e.g. `--chain 1,10,8453`) for parallel multi-chain queries; `--rpc-url` is only allowed with a single chain.
- Selector choice is explicit for multi-provider flows; pass `--provider` (no implicit defaults).

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
    aave/ morpho/                 # direct lending + yield
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
