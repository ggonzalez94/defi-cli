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
- Added MegaETH chain alias normalization (`megaeth`, `mega eth`, `mega-eth`) to canonical ID `eip155:4326`, plus bootstrap symbol parsing for `MEGA`, `WETH`, and `USDT` (`USDT0` on MegaETH).
- Expanded bootstrap token-registry symbol parsing across supported chains for: `AAVE`, `CAKE`, `CRV`, `CRVUSD`, `ENA`, `ETHFI`, `EURC`, `FRAX`, `GHO`, `LDO`, `LINK`, `MORPHO`, `PENDLE`, `TAIKO`, `TUSD`, `UNI`, `USDE`, `USDS`, and `ZRO`.
- Added bootstrap token mappings for additional commonly quoted assets on supported EVM chains: `USD1`, `WBTC`, `CBBTC`, `PAXG`, `PEPE`, `SHIB`, `OP`, `USDT0`, `BTCB`, `WBNB`, `ARB`, `PYUSD`, `WAVAX`, `WLFI`, `XAUT`, and `PENGU`.
- Added Bungee auto-mode quoting support for bridge and swap (`bridge quote --provider bungee`, `swap quote --provider bungee`).
- Added `fibrous` swap provider integration (`swap quote --provider fibrous`) for `base`, `hyperevm`, and `citrea` without requiring an API key.
- Added chain normalization and bootstrap symbol coverage for `hyperevm` (`eip155:999`), `monad` (`eip155:143`), and `citrea` (`eip155:4114`).
- Added HyperEVM bootstrap token parsing for quote-friendly symbols (`USDC`, `WHYPE`).
- Added stable execution identifiers on lending/yield rows (`provider_native_id`) plus provider-supplied address fields where available (`market_address`, `vault_address`, `pool_address`).
- Added bridge quote `fee_breakdown` output with component fees (`lp_fee`, `relayer_fee`, `gas_fee`), aggregate totals, and amount-delta consistency metadata.

### Changed
- `swap quote` now defaults provider by chain family (`1inch` for EVM chains, `jupiter` for Solana).
- Added explicit chain-family validation across providers so unsupported EVM/Solana combinations fail with clear `unsupported` errors.
- DefiLlama lending fallback protocol matcher now recognizes `kamino`.
- Solana chain parsing is now mainnet-only; `solana-devnet`, `solana-testnet`, and custom Solana CAIP-2 references are rejected.
- Expanded CAIP-19 parsing to include HyperEVM quote assets with canonical `erc20` handling.
- Bungee quote routing now uses deterministic placeholder sender/receiver addresses for quote-only requests.
- Bungee quote providers now support optional dedicated-backend routing when both `DEFI_BUNGEE_API_KEY` and `DEFI_BUNGEE_AFFILIATE` are configured.
- Across quote normalization now reads provider `outputAmount` when available and fills `estimated_fee_usd` from stable-asset token-fee approximation when native USD fee fields are omitted.

### Fixed
- Tightened direct lending asset matching to prefer canonical token address/mint over symbol-only matches, reducing false positives on similarly named assets.
- Improved Kamino reserve collection performance and reliability by fetching per-market metrics concurrently while preserving deterministic output ordering.
- Fixed missing Fraxtal bootstrap mapping for `FRAX` to the Frax system pre-deploy token contract.
- Corrected HyperEVM canonical mainnet mapping to `eip155:999` across chain normalization and provider routing.
- Corrected Monad bootstrap token addresses for `WMON` and `USDC` to match the official Monad token list.
- Commands now continue with cache disabled when cache initialization fails, instead of returning an internal error.
- Fixed Fibrous route response decoding to handle nested token objects and nullable gas values.
- Disabled Fibrous `monad` routing while Monad route responses are unstable.
- Reduced sqlite cache lock contention under parallel runs by adding open-time lock coordination, sqlite busy-timeout, and retry/backoff on busy/locked operations.

### Docs
- Updated README and AGENTS guidance for Solana usage, Kamino/Jupiter providers, and API key semantics.

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
