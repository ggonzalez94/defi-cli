# Changelog

All notable user-facing changes to `defi-cli` are documented in this file.

Format:
- Keep entries under `Unreleased` until a tag is cut.
- Group notes by section in this order: `Added`, `Changed`, `Deprecated`, `Fixed`, `Docs`, `Security`.
- Keep bullets short and focused on user impact.

## [v0.5.0] - 2026-03-26

### Added
- Moonwell lending provider (Base, Optimism) — markets, rates, positions, yield opportunities/positions, and execution (supply, withdraw, borrow, repay).
- Added `--chain` filter to `protocols fees` and `protocols revenue` to filter by chain presence (e.g. `protocols fees --chain Ethereum`). Supports combined `--category` and `--chain` filtering, consistent with `dexes volume --chain` behavior.
- Added `--chain` filter to `protocols top` to rank protocols by chain-specific TVL (e.g. `protocols top --chain Ethereum`). When specified, TVL reflects the protocol's value locked on that chain rather than total TVL. Supports combined `--category` and `--chain` filtering. Output now includes `chains` count.
- Added `protocols revenue` command to rank protocols by 24h revenue (protocol-retained fees) with 7d/30d totals and 1d/7d/1m percentage changes (no API key required, uses DefiLlama revenue API). Supports `--category` filter and `--limit`.
- Added `dexes volume` command to rank DEXes by 24h trading volume with 7d/30d totals and 1d/7d/1m percentage changes (no API key required, uses DefiLlama DEX volume API). Supports `--chain` filter for chain presence and `--limit`.
- Added `protocols fees` command to rank protocols by 24h fee revenue with 7d/30d totals and 1d/7d/1m percentage changes (no API key required, uses DefiLlama fees API). Supports `--category` filter and `--limit`.
- Added `stablecoins chains` command to rank chains by total stablecoin market cap with dominant peg type and CAIP-2 chain IDs (no API key required, uses DefiLlama stablecoin chains API). Supports `--limit`.
- Added `stablecoins top` command to list top stablecoins by circulating market cap with price, chain count, and day/week/month supply changes (no API key required, uses DefiLlama stablecoins API). Supports `--peg-type` filter (e.g. `peggedUSD`, `peggedEUR`) and `--limit`.
- Added `chains gas` command to query current EVM gas prices (base fee, priority fee, gas price in gwei) with block number and EIP-1559 detection (no keys required, bypasses cache, supports `--rpc-url` override). Accepts comma-separated chains for multi-chain batch queries with parallel RPC fetching and partial-result support.
- Added `chains list` command to enumerate all supported chains with slugs, CAIP-2 identifiers, namespaces, and accepted aliases (no keys required, bypasses cache).
- Added `wallet balance` command to query native or ERC-20 token balances by address and chain, using on-chain RPC calls (no API key required).
- Cache auto-pruning: expired entries (past TTL + `max_stale`) are automatically deleted on startup to prevent unbounded `cache.db` growth.
- Added Tempo chain normalization, RPC defaults, and bootstrap stablecoin registries for mainnet (`tempo`/`presto`), Moderato testnet, and Tempo devnet.
- Added the `tempo` swap provider with on-chain quote and execution planning against the Tempo Stablecoin DEX, including `exact-input` and `exact-output` support.
- Added Tempo coverage to generic ERC-20 approval and transfer planning through shared chain/token registry support.
- Tempo native execution: `swap submit` now broadcasts Tempo type 0x76 transactions with fee-token payments and batched calls on Tempo mainnet, testnet, and devnet.
- Batched approve+swap in a single atomic Tempo transaction (no separate approval step).
- `--signer tempo` for agent wallet support via the Tempo CLI, with delegated access keys and expiry checks.
- `--fee-token` flag for execution commands (Tempo-only, defaults to USDC.e on mainnet).
- `actions estimate` now supports Tempo actions with fee-token-denominated gas estimates (`fee_unit`, `fee_token` fields).
- `StepExecutor` interface for chain-specific execution; EVM path extracted unchanged, Tempo path added.

### Changed
- Standard EVM execution now supports two signing backends: `--wallet` (OWS, recommended) and `--from-address` (local signer). OWS is the recommended default for new integrations.
- Command schema now exposes machine-readable `input_constraints` metadata so agents can detect rules such as `exactly_one_of(wallet, from_address)` without inferring them from help text.
- `swap quote --type exact-output` now supports `--provider tempo` in addition to `uniswap`.
- `swap plan` now supports Tempo execution planning, and `tempo-dex` / `tempodex` aliases normalize to the canonical `tempo` provider.
- `actions estimate` now returns fee-token-denominated estimates for Tempo actions with `fee_unit` and `fee_token` fields, instead of rejecting them.
- Tempo swap planning/quotes now validate TIP-20 currency metadata up front and return `unsupported` for non-USD assets or DEX reverts such as missing pairs, instead of reporting them as transient provider outages.

### Fixed
- Optimism USDC bootstrap address now points to native USDC (`0x0b2c...ff85`) instead of bridged USDC.e; added separate `USDC.e` entry for the bridged variant.

### Docs
- Consolidated execution auth documentation into a dedicated concept page covering both signing backends (OWS and local signer), with guides linking to it instead of inlining auth details.
- Documented Tempo chain aliases, provider support, native DEX caveats, and execution examples across README, AGENTS, and Mintlify docs.
- Updated Tempo swap examples to use supported USD TIP-20 pairs and documented that the DEX auto-routes supported pairs through quote-token relationships.

### Security
- Bridge `submit` now validates canonical Across/LiFi execution targets on covered source chains before signing, while keeping `--unsafe-provider-tx` as the explicit provider-payload override.

## [v0.4.0] - 2026-03-07

### Added
- Execution command surface: plan, submit, and status workflows for `swap` (TaikoSwap), `bridge` (Across, LiFi), `lend supply|withdraw|borrow|repay` (Aave, Morpho), `yield deposit|withdraw` (Aave, Morpho), `rewards claim|compound` (Aave), `approvals`, and `transfer`.
- Local signer support for execution (`--private-key`, `DEFI_PRIVATE_KEY_FILE`, keystore, or auto-discovered `~/.config/defi/key.hex`).
- Structured request input for `plan` and `submit` commands via `--input-json` and `--input-file` (use `-` to read JSON from stdin).
- Action inspection commands: `actions list`, `actions show`, and `actions estimate` (per-step EIP-1559 gas projections).
- `lend positions` to query account-level lending positions (Aave, Morpho) with `--type all|supply|borrow|collateral`.
- `yield positions` to query account-level yield deposit positions (Aave, Morpho).
- `yield history` for historical yield series with `--metrics apy_total,tvl_usd`, `--interval hour|day`, and `--window`/`--from`/`--to`.
- TaikoSwap provider for `swap quote` using on-chain quoter contract calls (no API key required).
- Taiko Hoodi chain alias and token registry entries (`USDC`, `USDT`, `WETH`).
- Execution-specific exit codes (`20`-`24`) for plan/simulation/policy/timeout/signer failures.
- `schema` output now includes inherited flags, typed defaults, `required`/`enum`/`format` metadata, command auth, and request/response structure hints.
- Nightly execution-planning smoke tests (`nightly-execution-smoke.yml`).

### Changed
- BREAKING: Morpho `yield opportunities` now returns vault-level opportunities (`provider_native_id_kind=vault_address`) instead of Morpho market IDs.
- BREAKING: `yield opportunities` removed subjective fields (`risk_level`, `risk_reasons`, `score`) and the `--max-risk` flag; now returns `backing_assets` with per-opportunity composition.
- BREAKING: `yield opportunities --sort` supports only objective keys (`apy_total|tvl_usd|liquidity_usd`, default `apy_total`).
- BREAKING: Lend and rewards commands now use `--provider` instead of `--protocol`.
- `yield` and `lend` command surfaces are explicitly split by intent: `yield` for passive deposit/withdraw, `lend` for loan lifecycle (`supply|withdraw|borrow|repay`).
- Morpho execution split by product: `yield deposit|withdraw` targets vaults (`--vault-address`), `lend` targets Morpho Blue markets (`--market-id`).
- `providers list` now includes execution capabilities (`*.plan`, `*.execute`) for all execution-capable providers.
- Yield liquidity/TVL sourcing is now provider-native: Aave, Morpho vaults, and Kamino each report native values.
- `lend positions --type all` returns disjoint rows: `supply`, `collateral`, and `borrow`.
- Execution pre-sign checks enforce bounded ERC-20 approvals by default; `--allow-max-approval` opts in to larger approvals.
- Bridge execution waits for destination settlement (Across, LiFi) before marking steps complete.
- LiFi bridge quote/plan support `--from-amount-for-gas` for destination gas top-up.
- `swap quote` and execution commands support `--rpc-url` to override chain default RPCs.
- Aave execution auto-resolves pool addresses on Ethereum, Optimism, Polygon, Base, Arbitrum, and Avalanche.

### Fixed
- Fixed execution read-after-write consistency for sequential on-chain steps (approval then contract call).
- Fixed `actions estimate` for multi-step same-chain actions using `eth_simulateV1` with per-step fallback.
- Fixed execution timeout budgeting so waits derive from per-step stages instead of the provider request timeout.
- Fixed `approvals submit` and `bridge submit` to short-circuit completed actions before requesting signer inputs.
- Fixed Across/LiFi bridge planning to reject provider payloads with invalid target addresses.
- Fixed Aave rewards compound planning to reject recipient/sender mismatches.
- Improved bridge execution error messaging to distinguish quote-only from execution-capable providers.

### Docs
- Documented full execution surface (plan/submit/status), signer setup, and execution exit codes across README, AGENTS.md, and Mintlify docs.
- Added `lend positions`, `yield positions`, `yield history`, and `yield deposit|withdraw` to docs and command references.
- Updated yield docs to reflect objective metrics (`backing_assets`, `tvl_usd`, `liquidity_usd`) and removed risk-based flags.
- Clarified bridge settlement timeout guidance (`--step-timeout` vs `--timeout`) and `--allow-max-approval` behavior.

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

[Unreleased]: https://github.com/ggonzalez94/defi-cli/compare/v0.4.0...HEAD
[v0.4.0]: https://github.com/ggonzalez94/defi-cli/compare/v0.3.1...v0.4.0
[v0.3.1]: https://github.com/ggonzalez94/defi-cli/compare/v0.3.0...v0.3.1
[v0.3.0]: https://github.com/ggonzalez94/defi-cli/compare/v0.2.0...v0.3.0
[v0.2.0]: https://github.com/ggonzalez94/defi-cli/compare/v0.1.1...v0.2.0
[v0.1.1]: https://github.com/ggonzalez94/defi-cli/compare/v0.1.0...v0.1.1
[v0.1.0]: https://github.com/ggonzalez94/defi-cli/releases/tag/v0.1.0
