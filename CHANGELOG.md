# Changelog

All notable user-facing changes to `defi-cli` are documented in this file.

Format:
- Keep entries under `Unreleased` until a tag is cut.
- Group notes by section in this order: `Added`, `Changed`, `Fixed`, `Docs`, `Security`.
- Keep bullets short and focused on user impact.

## [Unreleased]

### Added
- Added TaikoSwap provider support for `swap quote` using on-chain quoter contract calls (no API key required).
- Added swap execution workflow commands: `swap plan`, `swap run`, `swap submit`, and `swap status`.
- Added bridge execution workflow commands: `bridge plan`, `bridge run`, `bridge submit`, and `bridge status` (Across and LiFi providers).
- Added approvals workflow commands: `approvals plan`, `approvals run`, `approvals submit`, and `approvals status`.
- Added lend execution workflow commands under `lend supply|withdraw|borrow|repay ... plan|run|submit|status` (Aave and Morpho).
- Added rewards execution workflow commands under `rewards claim|compound ... plan|run|submit|status` (Aave).
- Added action persistence and inspection commands: `actions list` and `actions show`.
- Added local signer support for execution with env/file/keystore key sources.
- Added Taiko Hoodi chain alias and token registry entries (`USDC`, `USDT`, `WETH`) for deterministic asset parsing.
- Added planner unit tests for approvals, Aave lend/rewards flows, and LiFi bridge action building.
- Added centralized execution registry data in `internal/registry` for endpoint, contract, and ABI references.
- Added nightly execution-planning smoke workflow (`nightly-execution-smoke.yml`) and script.
- Added `lend positions` to query account-level lending positions by address for Aave and Morpho with `--type all|supply|borrow|collateral`.
- Added `yield history` to query historical yield-provider series with `--metrics apy_total,tvl_usd`, `--interval hour|day`, `--window`/`--from`/`--to`, and optional `--opportunity-ids`.
- Added `actions estimate` to compute per-step gas projections for persisted actions using `eth_estimateGas` and EIP-1559 fee cap/tip resolution.

### Changed
- BREAKING: Morpho `yield opportunities` now returns vault-level opportunities (`provider_native_id_kind=vault_address`) sourced from Morpho vault/vault-v2 data instead of Morpho market IDs.
- BREAKING: `yield opportunities` removed subjective fields (`risk_level`, `risk_reasons`, `score`) and removed the `--max-risk` flag.
- BREAKING: `yield opportunities --sort` now supports only objective keys (`apy_total|tvl_usd|liquidity_usd`) and defaults to `apy_total`.
- `yield opportunities` now returns `backing_assets` with full per-opportunity backing composition.
- Yield liquidity/TVL sourcing is now provider-native and consistent: Aave (`size.usd`, `borrowInfo.availableLiquidity.usd`), Morpho vaults (`totalAssetsUsd`, vault liquidity fields), and Kamino (`totalSupplyUsd`, `max(totalSupplyUsd-totalBorrowUsd,0)`).
- BREAKING: Lend and rewards commands now use `--provider` as the selector flag; `--protocol` has been removed.
- `providers list` now includes TaikoSwap execution capabilities (`swap.plan`, `swap.execute`) alongside quote metadata.
- `providers list` now includes LiFi bridge execution capabilities (`bridge.plan`, `bridge.execute`).
- `providers list` now includes Across bridge execution capabilities (`bridge.plan`, `bridge.execute`).
- `providers list` now includes Morpho lend execution capabilities (`lend.plan`, `lend.execute`).
- Added execution-specific exit codes (`20`-`24`) for plan/simulation/policy/timeout/signer failures.
- Added execution config/env support for action store paths.
- Execution command cache/action-store policy now covers `swap|bridge|approvals|lend|rewards ... plan|run|submit|status`.
- Removed implicit defaults for multi-provider command paths; `--provider` must be set explicitly where applicable.
- Added bridge gas-top-up request support via `--from-amount-for-gas` for LiFi quote/plan/run flows.
- Bridge execution now tracks LiFi destination settlement status before finalizing bridge steps.
- Bridge execution now tracks Across destination settlement status before finalizing bridge steps.
- Aave execution registry defaults now include PoolAddressesProvider mappings for Base, Arbitrum, Optimism, Polygon, and Avalanche in addition to Ethereum.
- Execution `run`/`submit` commands now propagate command timeout/cancel context through on-chain execution.
- Morpho lend execution now requires explicit `--market-id` to avoid ambiguous market selection.
- Execution `run`/`submit` commands no longer require `--yes`; command intent now gates execution.
- Unified execution action-construction dispatch under a shared ActionBuilder registry while preserving existing command semantics.
- Execution commands now use `--from-address` as the single signer-address guard; `--confirm-address` has been removed.
- Execution `run` commands now default sender to signer address when `--from-address` is omitted.
- Execution `run`/`submit` commands now support `--private-key` as a one-off local signer override (highest precedence).
- Local signer `--key-source auto` now discovers `${XDG_CONFIG_HOME:-~/.config}/defi/key.hex` when present.
- Missing local-signer key errors now include a simple default key-file hint (`~/.config/defi/key.hex`, with `XDG_CONFIG_HOME` override note).
- Local signer key/keystore file loading no longer hard-fails on non-`0600` file permissions.
- Execution endpoint defaults for Across/LiFi settlement polling and Morpho GraphQL planning are now centralized in `internal/registry`.
- Default chain RPC metadata is now centralized in `internal/registry/rpc.go`; execution/quote flows use shared chain defaults when `--rpc-url` is not provided.
- Execution pre-sign validation now enforces bounded ERC-20 approvals by default and validates TaikoSwap router/selector invariants before signing.
- Execution `run`/`submit` commands now expose `--allow-max-approval` and `--unsafe-provider-tx` overrides for advanced/provider-specific flows.
- `swap quote` (on-chain providers) and `swap plan`/`swap run` now support `--rpc-url` to override chain default RPCs per invocation.
- Swap execution planning now validates sender/recipient fields as EVM addresses before route planning.
- Uniswap `swap quote` now requires a real `--from-address` swapper input instead of using a deterministic placeholder address.
- `lend positions` now emits non-overlapping type rows for automation: `supply` (non-collateral), `collateral` (posted collateral), and `borrow` (debt).
- `providers list` for Aave/Morpho/Kamino now advertises `yield.history` capability metadata.

### Fixed
- Improved bridge execution error messaging to clearly distinguish quote-only providers from execution-capable providers.

### Docs
- Documented bridge/lend/rewards/approvals execution flows, signer env inputs, command behavior, and exit codes in `README.md`.
- Updated `AGENTS.md` with expanded execution command coverage and caveats.
- Updated `docs/act-execution-design.md` implementation status to reflect the shipped Phase 2 surface.
- Clarified execution builder architecture split (provider-backed route builders for swap/bridge vs internal planners for lend/rewards/approvals) in `AGENTS.md` and execution design docs.
- Added `lend positions` usage and caveats to `README.md`, `AGENTS.md`, and Mintlify lending command reference.
- Documented `yield history` usage, flags, and provider caveats across README and Mintlify yield/lending references.
- Updated yield docs/reference examples to remove risk-based flags and document `backing_assets` plus objective `tvl_usd`/`liquidity_usd` semantics.

### Security
- None yet.

## [v0.3.1] - 2026-02-25

### Added
- None yet.

### Changed
- Added `swap quote --slippage-pct` to optionally override Uniswap max slippage percent; default behavior remains provider auto slippage.
- Added `swap quote --type` with `exact-input|exact-output` modes plus explicit `--amount-out`/`--amount-out-decimal` for exact-output requests.
- Swap quote rows now include `trade_type`; `uniswap` supports `exact-output` while other swap providers currently return `unsupported` for non-default types.
- EVM exact-output swaps without `--provider` now default to `uniswap` (instead of defaulting to `1inch` and failing unsupported).

### Fixed
- Fixed `swap quote --provider uniswap` live quote compatibility by adding required request fields (`swapper`, `autoSlippage`) and accepting string-encoded `gasFeeUSD` values from Trade API responses.

### Docs
- Release pipeline now syncs `docs-live` to each `v*` tag so Mintlify production docs can track the latest release instead of unreleased `main`.

### Security
- None yet.

## [v0.3.0] - 2026-02-24

### Added
- Added Solana canonical ID parsing for both chain references (`solana:<reference>`) and token IDs (`solana:<reference>/token:<mint>`).
- Added direct Kamino support for `lend markets`, `lend rates`, and `yield opportunities` on Solana mainnet.
- Added direct Jupiter swap quotes on Solana; `DEFI_JUPITER_API_KEY` remains optional for higher limits.
- Added Bungee Auto quote support for both `bridge quote` and `swap quote` (keyless by default).
- Added the `fibrous` swap provider for `base`, `hyperevm`, and `citrea` without an API key.
- Added MegaETH alias normalization (`megaeth`, `mega eth`, `mega-eth`) to canonical chain ID `eip155:4326`.
- Expanded chain normalization/bootstrap coverage for `hyperevm` (`eip155:999`), `monad` (`eip155:143`), and `citrea` (`eip155:4114`).
- Expanded bootstrap token symbol/address coverage across supported EVM chains, including MegaETH/HyperEVM/Fraxtal updates.
- Added provider-scoped identifier metadata to lending/yield rows: `provider`, `provider_native_id`, and `provider_native_id_kind`.
- Added `fee_breakdown` on bridge quotes with component fees (`lp_fee`, `relayer_fee`, `gas_fee`), totals, and amount-delta consistency metadata.

### Changed
- `swap quote` now defaults by chain family: `1inch` on EVM and `jupiter` on Solana.
- Lending and yield routes now use direct protocol adapters only; DefiLlama fallback routing was removed.
- Removed `spark` from lending protocol routing.
- Added explicit chain-family validation so unsupported EVM/Solana provider combinations fail with clear `unsupported` errors.
- Solana parsing is now mainnet-only; `solana-devnet`, `solana-testnet`, and custom Solana CAIP-2 references are rejected.
- Bungee quote mode now uses deterministic placeholder sender/receiver addresses for quote-only requests.
- Bungee dedicated backend mode now activates only when both `DEFI_BUNGEE_API_KEY` and `DEFI_BUNGEE_AFFILIATE` are set.
- Across quote normalization now uses provider `outputAmount` when available and fills missing USD fees with stable-asset approximations when needed.

### Fixed
- Tightened direct lending asset matching to prioritize canonical token address/mint over symbol-only matches.
- Improved Kamino reserve collection reliability and performance by fetching per-market metrics concurrently while keeping deterministic output order.
- Fixed missing Fraxtal bootstrap mapping for `FRAX` to the official Frax system predeploy contract.
- Corrected HyperEVM canonical mapping to `eip155:999` across normalization and provider routing.
- Corrected Monad bootstrap addresses for `WMON` and `USDC` to match the official Monad token list.
- Fixed Fibrous route decoding for nested token objects and nullable gas values.
- Disabled Fibrous `monad` routing while Monad upstream route responses remain unstable.
- Commands now continue with cache disabled when cache initialization fails, instead of returning internal errors.
- Reduced sqlite cache lock contention in parallel runs using lock coordination, busy-timeout, and retry/backoff handling.

### Docs
- Launched a Mintlify docs site (`docs/docs.json` + structured MDX guides, concepts, and reference pages).
- Updated README and AGENTS docs for Solana support, Kamino/Jupiter routing, and provider API key behavior.
- Added docs CI checks (`mint validate`, `mint broken-links`) for docs-related changes.
- Simplified docs information architecture and refreshed branding/header assets.

### Security
- None yet.

## [v0.2.0] - 2026-02-22

### Added
- Added `chains assets` to return chain-level TVL broken down by asset, with optional `--asset` filtering.
- Added broader EVM `--chain` support across lending, yield, bridge, swap, and asset helpers.
- Added full support for Mantle, Ink, Scroll, Gnosis, Linea, Sonic, Celo, and zkSync Era, including bootstrap token registry coverage (`USDC`/`USDT`/`WETH`, where available) and verified provider coverage.
- Added partial support for Berachain, Blast, Fraxtal, and World Chain via chain normalization and routing support.
- Added partial Taiko support with first-class bootstrap asset parsing for `USDC` and `WETH` from official network contracts, plus `taiko alethia` alias normalization.

### Changed
- DefiLlama provider capability metadata now marks `chains.assets` as requiring `DEFI_DEFILLAMA_API_KEY`.
- `version`, `schema`, and `providers list` now bypass cache initialization so metadata commands are not blocked by cache path failures.

### Fixed
- Preserved provider statuses, warnings, and `meta.partial=true` in strict-mode partial-result error envelopes (exit code `15`).
- Prevented unsafe DefiLlama symbol matching when asset symbols are unresolved, including skipping unsafe lending fallback/provider selection paths.

### Docs
- Documented `chains assets` API key requirement alongside other key-gated commands in README and AGENTS.
- Documented metadata-command cache bypass and unresolved-symbol fallback caveats in `README.md`/`AGENTS.md`.

### Security
- None yet.

## [v0.1.1] - 2026-02-21

### Changed
- Installer now defaults to writable user-space `PATH` locations and does not use `sudo` unless explicitly requested.

### Docs
- Refreshed README structure and feature overview.

## [v0.1.0] - 2026-02-21

### Added
- First tagged release of `defi-cli`.
- Core command surface for chains, protocols, assets, lending, yield, bridge, and swap workflows.
- Stable JSON envelope output and deterministic exit code contract for automation.

### Changed
- Project/module path migrated to `github.com/ggonzalez94/defi-cli`.

[Unreleased]: https://github.com/ggonzalez94/defi-cli/compare/v0.3.1...HEAD
[v0.3.1]: https://github.com/ggonzalez94/defi-cli/compare/v0.3.0...v0.3.1
[v0.3.0]: https://github.com/ggonzalez94/defi-cli/compare/v0.2.0...v0.3.0
[v0.2.0]: https://github.com/ggonzalez94/defi-cli/compare/v0.1.1...v0.2.0
[v0.1.1]: https://github.com/ggonzalez94/defi-cli/compare/v0.1.0...v0.1.1
[v0.1.0]: https://github.com/ggonzalez94/defi-cli/releases/tag/v0.1.0
