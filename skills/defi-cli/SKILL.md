---
name: defi-cli
description: How to use the defi-cli tool for on-chain DeFi queries and execution. Use this skill whenever the user mentions defi-cli, DeFi data lookups (lending rates, yield, swap quotes, bridge quotes, gas prices, protocol TVL), on-chain execution (supply, borrow, swap, bridge, transfer, approvals), or working with DeFi protocols like Aave, Morpho, Moonwell, Across, LiFi, Uniswap, 1inch, Tempo, TaikoSwap, or Jupiter. Also trigger when the user asks about checking lending positions, yield opportunities, stablecoin data, DEX volume, or anything involving the `defi` command.
---

# defi-cli — Agent Usage Guide

`defi` is an agent-first DeFi CLI. Every command returns structured JSON with a stable envelope, typed exit codes, and deterministic field ordering — parse stdout as JSON, branch on exit codes, and pipe between commands.

## Install

Check if already installed: `defi version`. If not:

```bash
curl -fsSL https://raw.githubusercontent.com/ggonzalez94/defi-cli/main/scripts/install.sh | sh
```

Installs to `~/.local/bin/defi` (or first writable `PATH` dir). Pin a version: `curl ... | sh -s v0.5.0`.

### Quick smoke test

After install, verify the CLI works end-to-end:

```bash
defi version                                    # should print version
defi providers list --results-only | head -20   # should print JSON array of providers
defi chains list --results-only | head -20      # should print supported chains
```

## Core Concepts

### Output Format

Every command returns a JSON envelope to stdout:

```json
{
  "version": "v1",
  "success": true,
  "data": <payload>,
  "error": null,
  "warnings": [],
  "meta": {
    "request_id": "hex16",
    "timestamp": "RFC3339",
    "command": "lend markets",
    "providers": [{ "name": "aave", "status": "ok", "latency_ms": 340 }],
    "cache": { "status": "hit|miss|write|bypass|stale", "age_ms": 0, "stale": false },
    "partial": false
  }
}
```

On error, the envelope goes to **stderr** (not stdout) — always a full envelope regardless of flags.

### Agent-Essential Flags

| Flag | What it does | When to use |
|------|-------------|-------------|
| `--results-only` | Emit only the `data` payload, skip envelope | Always use in automation — less to parse |
| `--select f1,f2` | Project specific fields from each result | When you only need a few fields |
| `--no-cache` | Skip cache reads/writes | When you need guaranteed-fresh data |
| `--strict` | Fail (exit 15) if any provider returns partial | When you need complete data or nothing |

**Example — get just APY and TVL:**
```bash
defi yield opportunities --chain 8453 --asset USDC --providers aave,moonwell --limit 5 --select provider,apy_total,tvl_usd --results-only
```

### Exit Codes

Branch on these in automation:

| Code | Meaning | Typical action |
|------|---------|---------------|
| 0 | Success | Parse stdout JSON |
| 2 | Usage error (bad flags, missing required) | Fix the command |
| 10 | Auth error (missing API key) | Set the env var |
| 11 | Rate limited | Back off and retry |
| 12 | Provider unavailable | Retry or try different provider |
| 13 | Unsupported operation | Use a different provider/chain combo |
| 15 | Partial results (with `--strict`) | Retry or drop `--strict` |
| 20-24 | Execution errors (plan/submit failures) | Check action status, investigate |

### Self-Discovery with `schema`

The `schema` command returns machine-readable metadata for any command — flags, types, defaults, required fields, auth requirements, and response structure:

```bash
defi schema                    # root schema with all subcommands
defi schema lend markets       # schema for a specific command
defi schema swap quote         # includes auth requirements and input modes
```

Use this to programmatically discover available commands and their parameters rather than hardcoding.

## Chain and Asset Resolution

### Chains

`--chain` accepts any of: numeric chain ID (`1`), CAIP-2 (`eip155:1`), slug (`ethereum`), or alias (`mainnet`). Case-insensitive.

Common aliases: `base` (8453), `arbitrum` (42161), `optimism` (10), `polygon` (137), `avalanche` (43114), `tempo`/`presto` (4217), `taiko` (167000), `megaeth` (4326), `monad` (143).

List all chains: `defi chains list --results-only`

### Assets

`--asset` accepts: symbol (`USDC`), EVM hex address (`0xa0b8...`), or CAIP-19 (`eip155:1/erc20:0xa0b8...`).

Symbol resolution uses a built-in registry covering ~30 tokens per major chain. On less-common chains, **use the address or CAIP-19** for deterministic matching — unrecognized symbols become filters that may not match.

Resolve an asset to its canonical ID:
```bash
defi assets resolve --chain 1 --asset USDC --results-only
# → { "symbol": "USDC", "asset_id": "eip155:1/erc20:0xa0b8...", "decimals": 6, ... }
```

### Amounts

Amounts are in **base units** (e.g., `1000000` = 1 USDC with 6 decimals). Every amount in the output includes both forms:
```json
{ "amount_base_units": "1000000", "amount_decimal": "1.0", "decimals": 6 }
```

Most commands accept either `--amount` (base units) or `--amount-decimal` for convenience. Use `assets resolve` to get decimals if unsure.

APY values are **percentage points** — `2.3` means `2.3%`, not `0.023`.

## Key Output Fields

These are the exact field names returned by the most-used commands. Use these with `--select` for precise field projection.

**`lend markets` / `lend rates`**: `protocol`, `provider`, `chain_id`, `asset_id`, `provider_native_id`, `provider_native_id_kind`, `supply_apy`, `borrow_apy`, `tvl_usd`, `liquidity_usd`, `source_url`, `fetched_at`

**`lend positions`**: `protocol`, `provider`, `chain_id`, `account_address`, `position_type` (`supply`|`borrow`|`collateral`), `asset_id`, `provider_native_id`, `provider_native_id_kind`, `amount` (AmountInfo object), `amount_usd`, `apy`, `source_url`, `fetched_at`

- **No `health_factor` field exists.** The CLI returns individual positions, not account-level health. To approximate health factor, sum `amount_usd` for `collateral` positions and divide by the sum for `borrow` positions. This is an approximation — true Aave health factor uses per-asset liquidation thresholds.

**`yield opportunities`**: `opportunity_id`, `provider`, `protocol`, `chain_id`, `asset_id`, `provider_native_id`, `provider_native_id_kind`, `type`, `apy_base`, `apy_reward`, `apy_total`, `tvl_usd`, `liquidity_usd`, `lockup_days`, `withdrawal_terms`, `backing_assets`, `source_url`, `fetched_at`

- For Morpho vaults, `provider_native_id_kind` is `vault_address` and `provider_native_id` is the vault contract address (use it as `--vault-address` in deposit plans).

**`bridge quote`**: `provider`, `from_chain_id`, `to_chain_id`, `from_asset_id`, `to_asset_id`, `input_amount` (AmountInfo), `estimated_out` (AmountInfo), `estimated_fee_usd`, `fee_breakdown`, `estimated_time_s`, `route`, `source_url`, `fetched_at`

- `estimated_out` is an AmountInfo object with `amount_base_units`, `amount_decimal`, and `decimals` — not a raw number. Parse `estimated_out.amount_base_units` for numeric comparison.

**`protocols fees`**: `rank`, `protocol`, `category`, `fees_24h_usd`, `fees_7d_usd`, `fees_30d_usd`, `change_1d_pct`, `change_7d_pct`, `change_1m_pct`, `chains`

- The `category` field value for lending protocols is `"Lending"` (capital L). The `protocol` field is the display name (e.g., `"Aave V3"`), not the CLI provider slug.

**`swap quote`**: `provider`, `chain_id`, `from_asset_id`, `to_asset_id`, `trade_type`, `input_amount` (AmountInfo), `estimated_out` (AmountInfo), `estimated_gas_usd`, `price_impact_pct`, `route`, `source_url`, `fetched_at`

## Read Commands

### Market Data (no API key needed)

```bash
# Top protocols by TVL
defi protocols top --limit 10 --results-only

# Filter by category and chain
defi protocols top --category lending --chain Ethereum --limit 5 --results-only

# Protocol fees and revenue
defi protocols fees --limit 10 --results-only
defi protocols revenue --chain Arbitrum --limit 10 --results-only

# DEX volume
defi dexes volume --limit 10 --results-only

# Stablecoins
defi stablecoins top --limit 10 --results-only
defi stablecoins chains --limit 10 --results-only

# Live gas prices (EVM only, bypasses cache)
defi chains gas --chain 1 --results-only
defi chains gas --chain 1,8453,42161 --results-only   # multi-chain parallel
```

### Lending

```bash
# Markets — requires --provider
defi lend markets --provider aave --chain 1 --asset USDC --results-only
defi lend markets --provider morpho --chain 1 --asset WETH --results-only
defi lend markets --provider moonwell --chain 8453 --asset USDC --results-only

# Rates (same flags as markets)
defi lend rates --provider aave --chain 1 --asset USDC --results-only

# Positions for an address
defi lend positions --provider aave --chain 1 --address 0x... --type all --results-only
# --type: all | supply | borrow | collateral
```

Providers: `aave`, `morpho`, `kamino` (Solana only), `moonwell` (Base + Optimism only).

Positions supported by: `aave`, `morpho`, `moonwell` (not `kamino`).

### Yield

```bash
# Compare yield across providers — fan-out when --providers omitted
defi yield opportunities --chain 1 --asset USDC --providers aave,morpho --limit 10 --results-only

# Filter low-quality vaults
defi yield opportunities --chain 1 --asset USDC --min-tvl-usd 100000 --limit 10 --results-only

# Positions
defi yield positions --chain 1 --address 0x... --providers aave,morpho --results-only

# Historical APY/TVL
defi yield history --chain 1 --asset USDC --providers morpho --window 7d --results-only
```

Morpho can emit extreme APYs in tiny markets — always use `--min-tvl-usd` when ranking.

### Swap Quotes

```bash
# Tempo (no API key)
defi swap quote --provider tempo --chain tempo --from-asset USDC.e --to-asset USDT.e --amount 1000000 --results-only

# TaikoSwap (no API key)
defi swap quote --provider taikoswap --chain taiko --from-asset USDC --to-asset WETH --amount 1000000 --results-only

# Uniswap (requires DEFI_UNISWAP_API_KEY + --from-address)
defi swap quote --provider uniswap --chain 1 --from-asset USDC --to-asset WETH --amount 1000000000 --from-address 0x... --results-only

# 1inch (requires DEFI_1INCH_API_KEY)
defi swap quote --provider 1inch --chain 1 --from-asset USDC --to-asset WETH --amount 1000000000 --results-only

# Exact-output (only uniswap and tempo)
defi swap quote --provider uniswap --chain 1 --from-asset USDC --to-asset WETH --type exact-output --amount-out 1000000000000000000 --from-address 0x... --results-only
```

### Bridge Quotes

```bash
# Across (no API key)
defi bridge quote --provider across --from 1 --to 42161 --asset USDC --amount 1000000000 --results-only

# LiFi (no API key)
defi bridge quote --provider lifi --from 1 --to 8453 --asset USDC --amount 1000000000 --results-only

# LiFi with gas top-up on destination
defi bridge quote --provider lifi --from 1 --to 8453 --asset USDC --amount 1000000000 --from-amount-for-gas 5000000 --results-only
```

### Wallet

```bash
# Native balance
defi wallet balance --chain 1 --address 0x... --results-only

# ERC-20 balance
defi wallet balance --chain 1 --address 0x... --asset USDC --results-only
```

## Execution Commands (plan/submit/status)

All mutation commands follow a three-step pattern: **plan** (build + persist), **submit** (sign + broadcast), **status** (check result).

### Step 1: Plan

Plans validate inputs, build calldata, simulate the transaction, and persist an `Action` with steps to the local action store. The plan returns an `action_id` you'll use for submit and status.

```bash
# Supply USDC to Aave on Ethereum via OWS wallet (recommended)
defi lend supply plan --provider aave --chain 1 --asset USDC --amount 1000000 --wallet my-wallet --results-only

# Bridge USDC from Ethereum to Arbitrum via Across
defi bridge plan --provider across --from 1 --to 42161 --asset USDC --amount 1000000000 --wallet my-wallet --results-only

# Transfer USDC
defi transfer plan --chain 1 --asset USDC --amount 1000000 --recipient 0xRecipient --wallet my-wallet --results-only

# ERC-20 approval
defi approvals plan --chain 1 --asset USDC --spender 0xSpender --amount 1000000 --wallet my-wallet --results-only
```

### Step 2: Submit

Submit loads the action, signs, broadcasts, and polls for confirmation:

```bash
# OWS wallet submit (reads DEFI_OWS_TOKEN)
export DEFI_OWS_TOKEN=$(ows token create --wallet my-wallet --ttl 24h)
defi lend supply submit --action-id act_abc123 --results-only

# Legacy local signer (reads DEFI_PRIVATE_KEY or key file)
defi lend supply submit --action-id act_abc123 --results-only

# Tempo signer (requires Tempo CLI configured)
defi swap submit --action-id act_abc123 --signer tempo --results-only
```

### Step 3: Status

```bash
defi lend supply status --action-id act_abc123 --results-only
```

### Action Management

```bash
defi actions list --results-only                        # list all actions
defi actions list --status planned --results-only       # filter by status
defi actions show --action-id act_abc123 --results-only # full action detail
defi actions estimate --action-id act_abc123 --results-only  # gas estimate
```

### Structured Input

All plan commands accept structured JSON input as an alternative to flags:

```bash
# Inline JSON
defi lend supply plan --input-json '{"provider":"aave","chain":"1","asset":"USDC","amount":"1000000","wallet":"my-wallet"}' --results-only

# From file
defi lend supply plan --input-file params.json --results-only

# From stdin (use - as filename)
echo '{"provider":"aave","chain":"1","asset":"USDC","amount":"1000000","wallet":"my-wallet"}' | defi lend supply plan --input-file - --results-only
```

Explicit flags override values from structured input.

### Wallet Identity

Two modes for execution:

| Flag | Backend | Submit auth | Best for |
|------|---------|------------|----------|
| `--wallet <name>` | OWS | `DEFI_OWS_TOKEN` env var | Agents (recommended) |
| `--from-address <hex>` | Local signer | Private key via env/file/keystore | Manual/legacy |

`--wallet` and `--from-address` are mutually exclusive. OWS submit rejects all legacy signer flags.

**Exception**: Tempo chains use `--from-address` (not `--wallet`) and `--signer tempo` on submit.

## Available Execution Commands

| Domain | Actions | Providers |
|--------|---------|-----------|
| `lend` | `supply`, `withdraw`, `borrow`, `repay` | aave, morpho, moonwell |
| `yield` | `deposit`, `withdraw` | aave, morpho, moonwell |
| `swap` | (plan/submit/status) | tempo, taikoswap |
| `bridge` | (plan/submit/status) | across, lifi |
| `transfer` | (plan/submit/status) | native |
| `approvals` | (plan/submit/status) | native |
| `rewards` | `claim`, `compound` | aave |

Each has `plan`, `submit`, and `status` subcommands.

## API Key Requirements

Most commands work without API keys. These are the exceptions:

| Command | Env var | Notes |
|---------|---------|-------|
| `swap quote --provider 1inch` | `DEFI_1INCH_API_KEY` | Required |
| `swap quote --provider uniswap` | `DEFI_UNISWAP_API_KEY` | Required |
| `chains assets` | `DEFI_DEFILLAMA_API_KEY` | Required |
| `bridge list`, `bridge details` | `DEFI_DEFILLAMA_API_KEY` | Required |
| `swap quote --provider jupiter` | `DEFI_JUPITER_API_KEY` | Optional (raises rate limits) |

`providers list` shows all capabilities and which need keys.

## Common Patterns for Agents

### Discover then act

```bash
# 1. Find best yield
BEST=$(defi yield opportunities --chain 8453 --asset USDC --min-tvl-usd 100000 --limit 1 --select provider,apy_total,provider_native_id --results-only)

# 2. Plan deposit based on result
defi yield deposit plan --provider morpho --chain 8453 --asset USDC --amount 1000000 --vault-address 0x... --wallet my-wallet --results-only

# 3. Submit
defi yield deposit submit --action-id act_... --results-only
```

### Multi-chain position check

```bash
for chain in 1 8453 42161 10; do
  defi lend positions --provider aave --chain $chain --address 0x... --type all --results-only
done
```

### Compare bridge routes

```bash
defi bridge quote --provider across --from 1 --to 42161 --asset USDC --amount 1000000000 --select provider,estimated_out,estimated_fee_usd,estimated_time_s --results-only
defi bridge quote --provider lifi --from 1 --to 42161 --asset USDC --amount 1000000000 --select provider,estimated_out,estimated_fee_usd,estimated_time_s --results-only
```

### Error handling in scripts

Errors go to stderr as JSON. Use this pattern in bash:

```bash
output=$(defi lend markets --provider aave --chain 1 --asset USDC --results-only 2>/dev/null) || {
  exit_code=$?
  case $exit_code in
    10) echo "Missing API key" ;;
    11) echo "Rate limited — back off" ;;
    12) echo "Provider down — retry later" ;;
    *)  echo "Failed with exit code $exit_code" ;;
  esac
  exit 1
}
# Success — parse $output as JSON
```

## Gotchas

1. **`--provider` is always required** for lending, yield positions, swap, and bridge commands. There's no auto-selection — you must choose.

2. **Morpho-specific flags**: Morpho lending needs `--market-id` (bytes32). Morpho yield needs `--vault-address`.

3. **Moonwell coverage**: Only Base (8453) and Optimism (10). Uses on-chain RPC reads, no API key.

4. **Tempo is different**: Uses `--from-address` (not `--wallet`), `--signer tempo` on submit, type 0x76 batched transactions, and `--fee-token` (defaults to USDC.e). Tempo DEX is USD TIP-20 stablecoins only.

5. **Aave chain coverage**: Default pool-address-provider for chains 1, 10, 137, 8453, 42161, 43114. Other chains need `--pool-address` or `--pool-address-provider`.

6. **Errors go to stderr**: Even with `--results-only`, errors emit a full JSON envelope to stderr. Parse stderr for error details.

7. **Cache**: Fresh cache hits skip provider calls. Use `--no-cache` when you need live data. Execution commands and `chains gas` always bypass cache.

8. **Partial results**: When querying multiple providers (e.g., `yield opportunities` without `--providers`), individual failures produce warnings and `meta.partial = true`. Use `--strict` to fail instead.

9. **Pre-sign safety**: ERC-20 approvals are bounded by default. Use `--allow-max-approval` to opt into larger approvals. Bridge plans validate execution targets; `--unsafe-provider-tx` bypasses this.

10. **`--rpc-url`**: Available on most read and plan commands for custom RPC endpoints. Not available on submit/status (they use stored URLs).
