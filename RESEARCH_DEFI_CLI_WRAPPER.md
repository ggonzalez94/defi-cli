# DeFi CLI v1 Implementation Spec (Agent-First Retrieval)

Date: 2026-02-20  
Status: Ready for implementation

## 1. Executive Summary

This document turns the original research into a buildable v1 contract.
The scope stays retrieval-first, but the command, data, cache, auth, and test contracts are now explicit.

Core design:

1. `gogcli` style agent contract (deterministic JSON, schema command, stable exit codes, command allowlist).
2. `wacli` style layered internals (CLI -> app/use-case -> providers -> normalize -> cache/output).
3. `yield opportunities` is a first-class command in v1, not deferred.

## 2. Decisions Locked

1. Language: `Go` (Go 1.22+).
2. Platforms: macOS + Linux, both `amd64` and `arm64`.
3. Output mode: JSON-first (agent-first default).
4. Amount input: base units default, decimal input supported as explicit alternate flag.
5. API-key providers (`Uniswap`, `1inch`) are included in v1 and fail fast when key is missing.
6. Fixture/replay tests are included in v1, not deferred.

## 3. Scope and Non-Goals

### In scope (v1)

- Chain/protocol market snapshots.
- Lending market/rate retrieval.
- Bridge quote retrieval.
- Swap quote retrieval.
- Yield opportunity discovery and ranking.
- Provider-normalized data model and deterministic output contract.

### Out of scope (v1)

- Any transaction signing/submission.
- Portfolio write operations.
- Live streaming feeds (polling only).

## 4. CLI Contract

### Global flags

- `--json` (default)
- `--plain`
- `--select <csv_fields>`
- `--results-only`
- `--enable-commands <csv_command_paths>`
- `--strict`
- `--timeout <duration>`
- `--retries <n>`
- `--max-stale <duration>`
- `--no-stale`
- `--no-cache`
- `--config <path>`

### Output mode precedence

1. If both `--json` and `--plain` are set: exit `2`.
2. Default mode is JSON.
3. `--results-only` returns only `data`.
4. `--select` applies to `data` fields after normalization.

### JSON envelope (stable)

```json
{
  "version": "v1",
  "success": true,
  "data": [],
  "error": null,
  "warnings": [],
  "meta": {
    "request_id": "uuid-v4",
    "timestamp": "2026-02-20T03:00:00Z",
    "command": "yield opportunities",
    "providers": [
      {
        "name": "defillama",
        "status": "ok",
        "latency_ms": 92
      }
    ],
    "cache": {
      "status": "hit",
      "age_ms": 740,
      "stale": false
    },
    "partial": false
  }
}
```

### Exit codes

- `0`: success
- `1`: internal/unhandled error
- `2`: usage or validation error
- `10`: auth required/failed (missing or invalid API key)
- `11`: rate limited
- `12`: provider unavailable
- `13`: unsupported chain/provider/asset combination
- `14`: data too stale for requested SLA
- `15`: partial results in `--strict` mode
- `16`: command blocked by `--enable-commands` policy

### Partial results policy

- Default: return partial data with `success=true`, warning entries, `meta.partial=true`, exit `0`.
- `--strict`: partial data becomes exit `15`.

## 5. Canonical Data Standards

### Chain identifiers

- Canonical internal format: `CAIP-2`.
  - Example: `eip155:1`, `eip155:8453`.
- CLI input accepts:
  - CAIP-2 (`eip155:8453`)
  - numeric chain id (`8453`) for EVM chains
  - known slug alias (`base`, `ethereum`) resolved to CAIP-2

### Asset identifiers

- Canonical internal format: `CAIP-19`.
  - Example: `eip155:8453/erc20:0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`
- CLI input accepts:
  - CAIP-19 (preferred)
  - token address with `--chain`
  - symbol only if unambiguous in the given chain; else exit `2`

### Amount semantics

- Default flag: `--amount <base_units_integer_string>`
- Alternate flag: `--amount-decimal <decimal_string>` (mutually exclusive with `--amount`)
- Output always includes both:
  - `amount_base_units`
  - `amount_decimal`
  - `decimals`

## 6. Command Set (v1)

- `defi schema [command]`
- `defi providers list`
- `defi chains top --limit 20`
- `defi protocols top --category lending --limit 20`
- `defi assets resolve --chain eip155:8453 --symbol USDC`
- `defi lend markets --protocol aave --chain eip155:8453 --asset <caip19_or_address>`
- `defi lend rates --protocol morpho --chain eip155:1 --asset <caip19_or_address>`
- `defi bridge quote --provider across --from eip155:1 --to eip155:8453 --asset <caip19_or_address> --amount 1000000`
- `defi swap quote --provider 1inch --chain eip155:1 --from-asset <asset> --to-asset <asset> --amount 1000000`
- `defi yield opportunities --chain eip155:8453 --asset <asset> --min-tvl-usd 50000000 --limit 20`

## 7. Yield Opportunities (Detailed v1 Spec)

### Goal

Return high-quality, normalized opportunities for a target chain/asset with deterministic ranking and provenance.

### Supported providers in v1

- DefiLlama (broad yield universe)
- Aave
- Morpho
- Curve
- Pendle
- Spark (best-effort, lower stability)
- Optional enrichers: Uniswap, 1inch (for route/liquidity context where available)

### Command flags

- `--chain <caip2>` (required)
- `--asset <caip19|address|symbol>` (required)
- `--limit <n>` (default 20, max 200)
- `--min-tvl-usd <number>` (default 0)
- `--max-risk <low|medium|high|unknown>` (default `high`)
- `--min-apy <percent>` (default 0)
- `--providers <csv>` (optional provider subset)
- `--sort <score|apy_total|tvl_usd|liquidity_usd>` (default `score`)
- `--include-incomplete` (include opportunities with missing APY/TVL fields)

### Normalized opportunity schema

- `opportunity_id` (stable hash of provider + chain + market id + asset id)
- `provider`
- `protocol`
- `chain_id` (CAIP-2)
- `asset_id` (CAIP-19)
- `type` (`lend`, `lp_stable`, `lp_volatile`, `fixed_yield`, `staking`)
- `apy_base`
- `apy_reward`
- `apy_total`
- `tvl_usd`
- `liquidity_usd`
- `lockup_days`
- `withdrawal_terms`
- `risk_level` (`low`, `medium`, `high`, `unknown`)
- `risk_reasons` (string list)
- `score` (0-100)
- `source_url`
- `fetched_at`

### Ranking algorithm (deterministic)

Values are normalized to `[0,1]` before weighting:

- `apy_norm = clamp(apy_total, 0, 100) / 100`
- `tvl_norm = clamp(log10(tvl_usd + 1) / 10, 0, 1)`
- `liq_norm = clamp(liquidity_usd / max(tvl_usd, 1), 0, 1)`
- `risk_penalty`:
  - `low=0.10`
  - `medium=0.30`
  - `high=0.60`
  - `unknown=0.45`

Score formula:

- `score_raw = 0.45*apy_norm + 0.30*tvl_norm + 0.20*liq_norm - 0.25*risk_penalty`
- `score = round(clamp(score_raw, 0, 1) * 100, 2)`

Tie-breakers:

1. Higher `apy_total`
2. Higher `tvl_usd`
3. Lexicographic `opportunity_id`

### Missing data policy

- Missing `apy_total` or `tvl_usd`:
  - excluded by default
  - included only with `--include-incomplete`, with warning + `risk_level=unknown`

## 8. Providers, Auth, and Configuration

### Provider policy

- Public providers run without keys when possible.
- Keyed providers are enabled in v1 and required when their command path is used.

### API key env vars

- `DEFI_UNISWAP_API_KEY`
- `DEFI_1INCH_API_KEY`
- Additional provider keys can be added as `DEFI_<PROVIDER>_API_KEY`.

### Config precedence

1. CLI flags
2. Environment variables
3. Config file
4. Defaults

### Config and cache locations

- Config: `${XDG_CONFIG_HOME:-~/.config}/defi/config.yaml`
- Cache: `${XDG_CACHE_HOME:-~/.cache}/defi/cache.db`
- Lock file: `${XDG_CACHE_HOME:-~/.cache}/defi/cache.lock`

### Example config

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

## 9. Architecture

1. `cmd/defi`
2. `internal/app`
3. `internal/providers/<provider>`
4. `internal/normalize`
5. `internal/cache` (SQLite + WAL + lock)
6. `internal/out` (json/plain/select/results-only)
7. `internal/policy` (command allowlist, strictness, stale policy)
8. `internal/schema` (machine-readable command/flag/output schema)

Provider adapter interface requirements:

- `Capabilities()`
- `Health()`
- `Fetch(request)`
- `Normalize(response)`

## 10. Reliability, Cache, and Retry Policy

### Timeouts and retries

- Default provider timeout: `10s`
- Default retries: `2` (exponential backoff with jitter)
- Retry only idempotent retrievals

### TTL defaults

- `chains top`: `5m`
- `protocols top`: `5m`
- `lend markets`: `60s`
- `lend rates`: `30s`
- `bridge quote`: `15s`
- `swap quote`: `15s`
- `yield opportunities`: `60s`

### Staleness behavior

- If fresh cache exists, return it.
- If stale but within `--max-stale`, return with warning.
- If beyond stale budget:
  - with `--no-stale`: exit `14`
  - otherwise attempt refresh, and fail `12` on provider outage.

## 11. Test Strategy and Quality Gates

### Required test types in v1

- Unit tests for normalizers, amount/ID parsing, ranking math.
- Golden tests for CLI output envelopes and `--select/--results-only`.
- Fixture/replay HTTP tests for each provider.
- End-to-end command tests for all public commands.
- Cross-platform build tests (macOS/Linux, amd64/arm64).

### Minimum release bar

- All tests pass in CI (no allowed failures).
- Zero flaky tests in the default suite.
- `>= 95%` success on the fixed retrieval benchmark corpus.
- P95 latency targets:
  - cached reads: `<= 500ms`
  - uncached single-provider reads: `<= 2.5s`

## 12. Delivery Plan

### Phase 1 (foundation + complete v1 contract)

- Core command framework, envelope, schema, exit codes, policy flags.
- Providers: DefiLlama, Aave, Morpho, Across, LI.FI, Uniswap, 1inch.
- Commands: full v1 set, including `yield opportunities` and `swap quote`.
- Full test harness with fixtures/replay.

### Phase 2 (yield depth + provider hardening)

- Add Curve, Pendle, Spark yield adapters.
- Add provider health command and richer warnings.

### Phase 3 (hardening)

- Compound advanced adapter (if stable source quality is acceptable).
- Optional provenance signature mode for output verification.
- Additional benchmark automation.

## 13. Sources

- https://github.com/steipete/gogcli
- https://github.com/steipete/wacli
- https://api.llama.fi/v2/chains
- https://api.llama.fi/protocols
- https://api.v3.aave.com/graphql
- https://api.morpho.org/graphql
- https://docs.across.to/reference/api-reference
- https://docs.li.fi/introduction/welcome
- https://api.curve.finance/v1/getPools/ethereum/main
- https://api-v2.pendle.finance/core/v1/1/markets/active
- https://api-docs.uniswap.org/introduction
- https://api.1inch.dev/swap/v6.0/1/quote
