# Changelog

All notable user-facing changes to `defi-cli` are documented in this file.

Format:
- Keep entries under `Unreleased` until a tag is cut.
- Group notes by section in this order: `Added`, `Changed`, `Fixed`, `Docs`, `Security`.
- Keep bullets short and focused on user impact.

## [Unreleased]

### Added
- Expanded bootstrap token registry symbol support across supported chains for: `AAVE`, `CAKE`, `CRV`, `CRVUSD`, `ENA`, `ETHFI`, `EURC`, `FRAX`, `GHO`, `LDO`, `LINK`, `MORPHO`, `PENDLE`, `TAIKO`, `TUSD`, `UNI`, `USDE`, `USDS`, and `ZRO`.

### Changed
- None yet.

### Fixed
- Added missing Fraxtal bootstrap mapping for `FRAX` to the Frax system pre-deploy token contract.

### Docs
- None yet.

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
