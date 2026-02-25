# Changelog

All notable user-facing changes to `defi-cli` are documented in this file.

Format:
- Keep entries under `Unreleased` until a tag is cut.
- Group notes by section in this order: `Added`, `Changed`, `Fixed`, `Docs`, `Security`.
- Keep bullets short and focused on user impact.

## [Unreleased]

### Added
- None yet.

### Changed
- None yet.

### Fixed
- None yet.

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

[Unreleased]: https://github.com/ggonzalez94/defi-cli/compare/v0.3.0...HEAD
[v0.3.0]: https://github.com/ggonzalez94/defi-cli/compare/v0.2.0...v0.3.0
[v0.2.0]: https://github.com/ggonzalez94/defi-cli/compare/v0.1.1...v0.2.0
[v0.1.1]: https://github.com/ggonzalez94/defi-cli/compare/v0.1.0...v0.1.1
[v0.1.0]: https://github.com/ggonzalez94/defi-cli/releases/tag/v0.1.0
