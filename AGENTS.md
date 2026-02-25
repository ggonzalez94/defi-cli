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
    defillama/                    # market/yield normalization + fallback + bridge analytics
    across/ lifi/                 # bridge quotes + lifi execution planning
    oneinch/ uniswap/ taikoswap/  # swap quotes + taikoswap execution planning
    types.go                      # provider interfaces
  execution/                      # action persistence + planner helpers + signer abstraction + tx execution
  registry/                       # canonical execution endpoints/contracts/ABI fragments
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
.github/workflows/nightly-execution-smoke.yml # nightly execution planning drift checks
.github/workflows/release.yml     # tagged release pipeline (GoReleaser)
scripts/install.sh                # macOS/Linux installer from GitHub Releases
.goreleaser.yml                   # cross-platform release artifact config
assets/                            # static assets (logo, images)
README.md                         # user-facing usage + caveats
```

## Non-obvious but important

- Error output always returns a full envelope, even with `--results-only` or `--select`.
- Config precedence is `flags > env > config file > defaults`.
- `yield --providers` expects provider names (`defillama,aave,morpho`), not protocol categories.
- Lending routes by `--protocol` to direct adapters when available, then may fallback to DefiLlama on selected failures.
- Most commands do not require provider API keys.
- Key-gated routes: `swap quote --provider 1inch` (`DEFI_1INCH_API_KEY`), `swap quote --provider uniswap` (`DEFI_UNISWAP_API_KEY`), `chains assets`, and `bridge list` / `bridge details` via DefiLlama (`DEFI_DEFILLAMA_API_KEY`).
- Multi-provider command paths require explicit provider/protocol selection (`--provider` or `--protocol`); no implicit defaults.
- TaikoSwap quote/planning does not require an API key; execution uses local signer env inputs (`DEFI_PRIVATE_KEY{,_FILE}` or keystore envs) and also auto-discovers `${XDG_CONFIG_HOME:-~/.config}/defi/key.hex` when present.
- Execution commands currently available:
  - `swap plan|run|submit|status`
  - `bridge plan|run|submit|status` (Across, LiFi)
  - `approvals plan|run|submit|status`
  - `lend supply|withdraw|borrow|repay plan|run|submit|status` (Aave, Morpho)
  - `rewards claim|compound plan|run|submit|status` (Aave)
  - `actions list|status`
- Execution builder architecture is intentionally split:
  - `swap`/`bridge` action construction is provider capability based (`BuildSwapAction` / `BuildBridgeAction`) because route payloads are provider-specific.
  - `lend`/`rewards`/`approvals` action construction uses internal planners for deterministic contract-call composition.
- All execution `run` / `submit` commands can broadcast transactions.
- Execution pre-sign checks enforce bounded ERC-20 approvals by default; `--allow-max-approval` opts into larger approvals when required.
- Bridge execution pre-sign checks validate provider settlement metadata/endpoints by default; `--unsafe-provider-tx` bypasses these guardrails.
- LiFi bridge quote/plan/run support optional `--from-amount-for-gas` (source token base units reserved for destination native gas top-up).
- Bridge execution status for Across/LiFi waits for destination settlement (`/deposit/status` or `/status`) before marking bridge steps complete.
- Rewards `--assets` expects comma-separated on-chain addresses used by Aave incentives contracts.
- Aave execution has default pool-address-provider coverage for chain IDs `1`, `10`, `137`, `8453`, `42161`, and `43114`; override with `--pool-address` / `--pool-address-provider` otherwise.
- Morpho lend execution requires `--market-id` (Morpho market unique key bytes32).
- Key requirements are command + provider specific; `providers list` is metadata only and should remain callable without provider keys.
- Prefer env vars for provider keys in docs/examples; keep config file usage optional and focused on non-secret defaults.
- `--chain` supports CAIP-2, numeric chain IDs, and aliases; aliases include `mantle`, `ink`, `scroll`, `berachain`, `gnosis`/`xdai`, `linea`, `sonic`, `blast`, `fraxtal`, `world-chain`, `celo`, `taiko`/`taiko alethia`, `taiko hoodi`/`hoodi`, and `zksync`.
- Symbol parsing depends on the local bootstrap token registry; on chains without registry entries use token address or CAIP-19.
- APY values are percentage points (`2.3` means `2.3%`), not ratios.
- Morpho can emit extreme APYs in tiny markets; use `--min-tvl-usd` in ranking/filters.
- Fresh cache hits (`age <= ttl`) skip provider calls; once TTL expires, the CLI re-fetches providers and only serves stale data within `max_stale` on temporary provider failures.
- Metadata commands (`version`, `schema`, `providers list`) bypass cache initialization.
- Execution commands (`swap|bridge|approvals|lend|rewards ... plan|run|submit|status`, `actions list|status`) bypass cache initialization.
- For `lend`/`yield`, unresolved asset symbols skip DefiLlama symbol matching and fallback/provider selection where symbol-based matching would be unsafe.
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
- Keep README/AGENTS focused on current behavior; track version-to-version deltas in CHANGELOG/release notes instead of adding temporary in-progress migration notes.

## Maintenance note

- Keep `README.md`, `AGENTS.md`, and `CHANGELOG.md` aligned when commands, routing, caveats, or release-relevant behavior change.

Do not commit transient binaries like `./defi`.
