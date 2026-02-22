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
./defi lend markets --protocol kamino --chain solana --asset USDC --results-only
./defi swap quote --chain solana --from-asset USDC --to-asset SOL --amount 1000000 --results-only
```

## Folder structure

```text
cmd/
  defi/main.go                    # CLI entrypoint

internal/
  app/runner.go                   # command wiring, provider routing, cache flow
  providers/                      # external adapters
    aave/ morpho/ kamino/         # direct GraphQL/REST lending + yield
    defillama/                    # market/yield normalization + fallback + bridge analytics
    across/ lifi/                 # bridge quotes
    oneinch/ uniswap/ jupiter/    # swap quotes
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
.github/workflows/release.yml     # tagged release pipeline (GoReleaser)
scripts/install.sh                # macOS/Linux installer from GitHub Releases
.goreleaser.yml                   # cross-platform release artifact config
assets/                            # static assets (logo, images)
README.md                         # user-facing usage + caveats
```

## Non-obvious but important

- Error output always returns a full envelope, even with `--results-only` or `--select`.
- Config precedence is `flags > env > config file > defaults`.
- `yield --providers` expects provider names (`defillama,aave,morpho,kamino`), not protocol categories.
- Lending routes by `--protocol` to direct adapters when available, then may fallback to DefiLlama on selected failures.
- Most commands do not require provider API keys.
- Key-gated routes: `swap quote --provider 1inch` (`DEFI_1INCH_API_KEY`), `swap quote --provider uniswap` (`DEFI_UNISWAP_API_KEY`), and `bridge list` / `bridge details` via DefiLlama (`DEFI_DEFILLAMA_API_KEY`).
- `swap quote --provider jupiter` supports `DEFI_JUPITER_API_KEY` optionally (higher limits); Solana swap defaults to `jupiter`.
- Key requirements are command + provider specific; `providers list` is metadata only and should remain callable without provider keys.
- Prefer env vars for provider keys in docs/examples; keep config file usage optional and focused on non-secret defaults.
- APY values are percentage points (`2.3` means `2.3%`), not ratios.
- Morpho can emit extreme APYs in tiny markets; use `--min-tvl-usd` in ranking/filters.
- Kamino direct routes currently support Solana mainnet only.
- Fresh cache hits (`age <= ttl`) skip provider calls; once TTL expires, the CLI re-fetches providers and only serves stale data within `max_stale` on temporary provider failures.
- Amounts used for swaps/bridges are base units; keep both base and decimal forms consistent.
- Release artifacts are built on `v*` tags via `.github/workflows/release.yml` and `.goreleaser.yml`.
- `scripts/install.sh` installs the latest tagged release artifact into a writable user-space `PATH` directory by default (fallback `~/.local/bin`) and never uses sudo unless explicitly requested.

## Change patterns

- New provider:
  1. implement adapter in `internal/providers/<name>/client.go`
  2. register routes/info in `internal/app/runner.go`
  3. add `httptest`-based adapter tests
  4. update README caveats if data quality/semantics differ
  5. document any command that requires an API key explicitely
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
- CHANGELOG updated for user-visible changes

## Changelog workflow

- Keep `CHANGELOG.md` in a simple release-notes format with `## [Unreleased]` at the top.
- Add user-facing changes under `Unreleased` using sections in this order: `Added`, `Changed`, `Fixed`, `Docs`, `Security`.
- Keep entries concise and action-oriented (what changed for users, not internal refactors unless user impact exists).
- On release, move `Unreleased` items into `## [vX.Y.Z] - YYYY-MM-DD` and update compare links at the bottom.
- If a section has no updates while editing, use `- None yet.` to keep structure stable.

## Maintenance note

- Keep `README.md`, `AGENTS.md`, and `CHANGELOG.md` aligned when commands, routing, caveats, or release-relevant behavior change.

Do not commit transient binaries like `./defi`.
