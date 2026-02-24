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
    defillama/                    # chain/protocol market data + bridge analytics
    across/ lifi/ bungee/         # bridge quotes
    oneinch/ uniswap/ jupiter/ fibrous/ bungee/  # swap quotes
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
docs/                             # Mintlify docs site
  docs.json                       # Mintlify docs site config
  *.mdx + concepts/ guides/ ...   # Mintlify docs content pages
README.md                         # user-facing usage + caveats
```

## Non-obvious but important

- Error output always returns a full envelope, even with `--results-only` or `--select`.
- Config precedence is `flags > env > config file > defaults`.
- `yield --providers` expects provider names (`aave,morpho,kamino`), not protocol categories.
- Lending routes by `--protocol` to direct adapters only (`aave`, `morpho`, `kamino`).
- Most commands do not require provider API keys.
- Key-gated routes: `swap quote --provider 1inch` (`DEFI_1INCH_API_KEY`), `swap quote --provider uniswap` (`DEFI_UNISWAP_API_KEY`), `chains assets`, and `bridge list` / `bridge details` via DefiLlama (`DEFI_DEFILLAMA_API_KEY`). `swap quote --provider jupiter` supports `DEFI_JUPITER_API_KEY` optionally (higher limits). `swap quote --provider fibrous` is keyless. Bungee Auto-mode quotes (`bridge quote --provider bungee`, `swap quote --provider bungee`) are keyless by default; optional dedicated-backend mode requires both `DEFI_BUNGEE_API_KEY` and `DEFI_BUNGEE_AFFILIATE`.
- Key requirements are command + provider specific; `providers list` is metadata only and should remain callable without provider keys.
- Prefer env vars for provider keys in docs/examples; keep config file usage optional and focused on non-secret defaults.
- `--chain` supports CAIP-2, numeric chain IDs, and aliases; aliases include `mantle`, `megaeth`/`mega eth`/`mega-eth`, `ink`, `scroll`, `berachain`, `gnosis`/`xdai`, `linea`, `sonic`, `blast`, `fraxtal`, `world-chain`, `celo`, `taiko`/`taiko alethia`, `zksync`, `hyperevm`/`hyper evm`/`hyper-evm`, `monad`, and `citrea`.
- Bungee Auto quote calls use deterministic placeholder sender/receiver addresses for quote-only mode (`0x000...001`).
- MegaETH bootstrap symbol parsing currently supports `MEGA`, `WETH`, and `USDT` (`USDT` maps to the chain's `USDT0` contract address on `eip155:4326`). Official Mega token list currently has no Ethereum L1 `MEGA` token entry.
- Symbol parsing depends on the local bootstrap token registry; on chains without registry entries use token address or CAIP-19.
- APY values are percentage points (`2.3` means `2.3%`), not ratios.
- Morpho can emit extreme APYs in tiny markets; use `--min-tvl-usd` in ranking/filters.
- `lend`/`yield` rows expose retrieval-first ID metadata: `provider`, `provider_native_id`, and `provider_native_id_kind`; IDs are provider-scoped and not guaranteed to be on-chain addresses.
- Bridge quotes now include `fee_breakdown` with provider-reported components (`lp_fee`, `relayer_fee`, `gas_fee`) and amount-delta consistency checks.
- Kamino direct routes currently support Solana mainnet only.
- Solana devnet/testnet aliases and custom Solana CAIP-2 references are intentionally unsupported; use Solana mainnet only.
- Fresh cache hits (`age <= ttl`) skip provider calls; once TTL expires, the CLI re-fetches providers and only serves stale data within `max_stale` on temporary provider failures.
- Cache locking uses sqlite WAL + busy timeout + lock/backoff retries to reduce `database is locked` contention under parallel runs.
- Cache initialization is best-effort; if cache path init fails (permissions/path issues), commands continue with cache disabled.
- Across may omit native USD fee fields for some routes; when missing and the input asset is a known stable token, `estimated_fee_usd` falls back to a token-denominated approximation while exact token-unit fees remain in `fee_breakdown`.
- Metadata commands (`version`, `schema`, `providers list`) bypass cache initialization.
- Amounts used for swaps/bridges are base units; keep both base and decimal forms consistent.
- Release artifacts are built on `v*` tags via `.github/workflows/release.yml` and `.goreleaser.yml`.
- `scripts/install.sh` installs the latest tagged release artifact into a writable user-space `PATH` directory by default (fallback `~/.local/bin`) and never uses sudo unless explicitly requested.
- Docs site local checks (from `docs/`): `npx --yes mint@4.2.378 validate` and `npx --yes mint@4.2.378 broken-links`.

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
- Docs sync for user-facing changes:
  1. if adding a feature/command or changing behavior, update Mintlify docs + README + CHANGELOG
  2. if changing output schema/fields/exit codes, update contract/reference docs before merge
  3. if adding providers/chains/assets/aliases/key requirements, update provider/auth and examples docs

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
- Record only the net user-facing outcome in `Unreleased`; omit intermediate implementation steps and fixes for regressions that never shipped in a release.
- Do not add changelog entries for README-only or `AGENTS.md`-only edits.
- On release, move `Unreleased` items into `## [vX.Y.Z] - YYYY-MM-DD` and update compare links at the bottom.
- If a section has no updates while editing, use `- None yet.` to keep structure stable.

## Maintenance note

- Keep `README.md`, `AGENTS.md`, `CHANGELOG.md`, and Mintlify docs (`docs/docs.json` + `docs/**/*.mdx`) aligned when commands, routing, caveats, or release-relevant behavior change.

Do not commit transient binaries like `./defi`.
