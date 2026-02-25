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
- Added action persistence and inspection commands: `actions list` and `actions status`.
- Added local signer support for execution with env/file/keystore key sources.
- Added Taiko Hoodi chain alias and token registry entries (`USDC`, `USDT`, `WETH`) for deterministic asset parsing.
- Added planner unit tests for approvals, Aave lend/rewards flows, and LiFi bridge action building.
- Added centralized execution registry data in `internal/registry` for endpoint, contract, and ABI references.
- Added nightly execution-planning smoke workflow (`nightly-execution-smoke.yml`) and script.

### Changed
- `providers list` now includes TaikoSwap execution capabilities (`swap.plan`, `swap.execute`) alongside quote metadata.
- `providers list` now includes LiFi bridge execution capabilities (`bridge.plan`, `bridge.execute`).
- `providers list` now includes Across bridge execution capabilities (`bridge.plan`, `bridge.execute`).
- `providers list` now includes Morpho lend execution capabilities (`lend.plan`, `lend.execute`).
- Added execution-specific exit codes (`20`-`24`) for plan/simulation/policy/timeout/signer failures.
- Added execution config/env support for action store paths and Taiko RPC overrides.
- Execution command cache/action-store policy now covers `swap|bridge|approvals|lend|rewards ... plan|run|submit|status`.
- Removed implicit defaults for multi-provider command paths; `--provider`/`--protocol` must be set explicitly where applicable.
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
- Local signer `--key-source auto` now discovers `${XDG_CONFIG_HOME:-~/.config}/defi/key.hex` when present.
- Local signer key/keystore file loading no longer hard-fails on non-`0600` file permissions.

### Fixed
- Improved bridge execution error messaging to clearly distinguish quote-only providers from execution-capable providers.

### Docs
- Documented bridge/lend/rewards/approvals execution flows, signer env inputs, command behavior, and exit codes in `README.md`.
- Updated `AGENTS.md` with expanded execution command coverage and caveats.
- Updated `docs/act-execution-design.md` implementation status to reflect the shipped Phase 2 surface.
- Clarified execution builder architecture split (provider-backed route builders for swap/bridge vs internal planners for lend/rewards/approvals) in `AGENTS.md` and execution design docs.

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

[Unreleased]: https://github.com/ggonzalez94/defi-cli/compare/v0.2.0...HEAD
[v0.2.0]: https://github.com/ggonzalez94/defi-cli/compare/v0.1.1...v0.2.0
[v0.1.1]: https://github.com/ggonzalez94/defi-cli/compare/v0.1.0...v0.1.1
[v0.1.0]: https://github.com/ggonzalez94/defi-cli/releases/tag/v0.1.0
