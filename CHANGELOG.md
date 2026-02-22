# Changelog

All notable user-facing changes to `defi-cli` are documented in this file.

Format:
- Keep entries under `Unreleased` until a tag is cut.
- Group notes by section in this order: `Added`, `Changed`, `Fixed`, `Docs`, `Security`.
- Keep bullets short and focused on user impact.

## [Unreleased]

### Added
- Added `chains assets` to return chain-level TVL broken down by asset, with optional `--asset` filtering.

### Changed
- DefiLlama provider capability metadata now marks `chains.assets` as requiring `DEFI_DEFILLAMA_API_KEY`.
- Expanded EVM chain normalization support for `--chain` across lending, yield, bridge, swap, and asset helpers: Mantle, Ink, Scroll, Berachain, Gnosis, Linea, Sonic, Blast, Fraxtal, World Chain, Celo, and zkSync Era.
- Added bootstrap token registry entries (`USDC`/`USDT`/`WETH` where available) for new production chains with verified provider coverage (Gnosis, Sonic, zkSync Era, Mantle, Celo, Ink, Linea, Scroll).
- Added first-class Taiko bootstrap asset parsing (`USDC`, `WETH`) from official network contract addresses, plus `taiko alethia` alias normalization.
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

[Unreleased]: https://github.com/ggonzalez94/defi-cli/compare/v0.1.1...HEAD
[v0.1.1]: https://github.com/ggonzalez94/defi-cli/compare/v0.1.0...v0.1.1
[v0.1.0]: https://github.com/ggonzalez94/defi-cli/releases/tag/v0.1.0
