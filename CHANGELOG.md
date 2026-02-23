# Changelog

All notable user-facing changes to `defi-cli` are documented in this file.

Format:
- Keep entries under `Unreleased` until a tag is cut.
- Group notes by section in this order: `Added`, `Changed`, `Fixed`, `Docs`, `Security`.
- Keep bullets short and focused on user impact.

## [Unreleased]

### Added
- Added MegaETH mainnet chain normalization (`megaeth`, `mega eth`, `mega-eth`) with canonical ID mapping to `eip155:4326`.
- Expanded bootstrap token registry symbol support across supported chains for: `AAVE`, `CAKE`, `CRV`, `CRVUSD`, `ENA`, `ETHFI`, `EURC`, `FRAX`, `GHO`, `LDO`, `LINK`, `MORPHO`, `PENDLE`, `TAIKO`, `TUSD`, `UNI`, `USDE`, `USDS`, and `ZRO`.
- Added Bungee Auto-mode quoting as a bridge provider (`bridge quote --provider bungee`) and swap provider (`swap quote --provider bungee`).
- Added HyperEVM alias normalization (`hyperevm`/`hyper-evm`) for quote workflows.
- Added HyperEVM bootstrap token parsing for quote-friendly symbols (`USDC`, `WHYPE`).

### Changed
- Added MegaETH bootstrap token parsing for `MEGA`, `WETH`, and `USDT` (mapped to MegaETH's `USDT0` contract address) to improve lending/bridge symbol workflows.
- Expanded CAIP-19 parsing to include HyperEVM quote assets with canonical `erc20` handling.
- Bungee quote routing now uses deterministic placeholder sender/receiver addresses for quote-only requests.
- Bungee quote providers now support optional dedicated-backend routing when both `DEFI_BUNGEE_API_KEY` and `DEFI_BUNGEE_AFFILIATE` are configured.

### Fixed
- Added missing Fraxtal bootstrap mapping for `FRAX` to the Frax system pre-deploy token contract.
- Commands now continue with cache disabled when cache initialization fails, instead of returning an internal error.

### Docs
- Updated README and AGENTS MegaETH chain alias coverage and bootstrap token caveats.
- Documented Bungee Auto-mode provider usage and new custom-chain alias coverage in README/AGENTS.
- Documented optional Bungee dedicated-backend environment variables and fallback behavior when only one value is set.

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
