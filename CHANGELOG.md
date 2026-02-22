# Changelog

All notable user-facing changes to `defi-cli` are documented in this file.

Format:
- Keep entries under `Unreleased` until a tag is cut.
- Group notes by section in this order: `Added`, `Changed`, `Fixed`, `Docs`, `Security`.
- Keep bullets short and focused on user impact.

## [Unreleased]

### Added
- Solana chain support in canonical ID parsing (`solana:<reference>`) and Solana CAIP-19 token parsing (`solana:<reference>/token:<mint>`).
- Direct Kamino adapter for `lend markets`, `lend rates`, and `yield opportunities` on Solana mainnet.
- Direct Jupiter swap adapter for Solana quotes, with optional `DEFI_JUPITER_API_KEY` support.

### Changed
- `swap quote` now defaults provider by chain family (`1inch` for EVM chains, `jupiter` for Solana).
- Added explicit chain-family validation across providers so unsupported EVM/Solana combinations fail with clear `unsupported` errors.
- DefiLlama lending fallback protocol matcher now recognizes `kamino`.

### Fixed
- Tightened direct lending asset matching to prefer canonical token address/mint over symbol-only matches, reducing false positives on similarly named assets.
- Improved Kamino reserve collection performance and reliability by fetching per-market metrics concurrently while preserving deterministic output ordering.

### Docs
- Updated README and AGENTS guidance for Solana usage, Kamino/Jupiter providers, and API key semantics.

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
