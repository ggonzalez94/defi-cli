# AGENTS.md

Short guide for agents working on `defi-cli`.

## Project intent

`defi-cli` is an agent-first DeFi retrieval CLI. Core priorities are:

- stable JSON contract (envelope + fields + deterministic ordering)
- stable exit codes
- canonical IDs/amounts for automation (CAIP + base units)

## First 5 minutes

```bash
go build -o defi ./cmd/defi
go test ./...
go test -race ./...
go vet ./...

./defi providers list --results-only
./defi lend markets --protocol aave --chain 1 --asset USDC --results-only
./defi yield opportunities --chain 1 --asset USDC --providers aave,morpho --limit 5 --results-only
```

## Folder structure

```text
cmd/
  defi/main.go                    # CLI entrypoint

internal/
  app/runner.go                   # command wiring, provider routing, cache flow
  providers/                      # external adapters
    aave/ morpho/                 # direct GraphQL lending + yield
    defillama/                    # market/yield normalization + fallback
    across/ lifi/                 # bridge quotes
    oneinch/ uniswap/             # swap quotes
    types.go                      # provider interfaces
  config/                         # defaults + file/env/flags precedence
  cache/                          # sqlite cache + file lock
  id/                             # CAIP parsing + amount normalization
  model/                          # output envelope + domain models
  out/                            # json/plain rendering and field selection
  errors/                         # typed errors -> exit codes
  schema/                         # machine-readable command schema
  policy/                         # command allowlist
  httpx/                          # shared HTTP client/retry behavior

.github/workflows/ci.yml          # CI (test/vet/build)
README.md                         # user-facing usage + caveats
RESEARCH_DEFI_CLI_WRAPPER.md      # design/spec context
```

## Non-obvious but important

- Error output always returns a full envelope, even with `--results-only` or `--select`.
- Config precedence is `flags > env > config file > defaults`.
- `yield --providers` expects provider names (`defillama,aave,morpho`), not protocol categories.
- Lending routes by `--protocol` to direct adapters when available, then may fallback to DefiLlama on selected failures.
- APY values are percentage points (`2.3` means `2.3%`), not ratios.
- Morpho can emit extreme APYs in tiny markets; use `--min-tvl-usd` in ranking/filters.
- Cache hits skip provider calls, so provider latency metadata may be absent on cached responses.
- Amounts used for swaps/bridges are base units; keep both base and decimal forms consistent.

## Change patterns

- New provider:
  1. implement adapter in `internal/providers/<name>/client.go`
  2. register routes/info in `internal/app/runner.go`
  3. add `httptest`-based adapter tests
  4. update README caveats if data quality/semantics differ
- Contract changes:
  1. treat as breaking unless explicitly intended
  2. update `internal/model` + `internal/out` tests first
- Behavior changes:
  1. keep cache keys deterministic
  2. add runner-level tests for routing/fallback/strict mode

## Quality bar

- `go test ./...` passes
- `go test -race ./...` passes
- `go vet ./...` passes
- smoke at least one command on each touched provider path
- README updated for user-visible changes

## Maintenance note

- Keep `README.md` and `AGENTS.md` aligned when commands, routing, caveats, or folder structure change.

Do not commit transient binaries like `./defi`.
