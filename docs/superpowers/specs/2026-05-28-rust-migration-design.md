# defi-cli Go → Rust Migration — Design Spec

**Date:** 2026-05-28
**Branch:** `migrate-to-rust`
**Status:** Approved (design), pending implementation plan
**Scope decision:** Full end-to-end port ("boil the ocean"). A single workflow run is not
expected to finish all 39k LOC clean; the **always-green invariant** + **remainder plan**
keep whatever completes correct and the rest honestly tracked.

---

## 1. Goal

Port `defi-cli` (an agent-first DeFi CLI) from Go to idiomatic Rust **without changing the
machine contract**. The deliverable is an executable, deterministic, TDD-driven **workflow**
(Workflow tool orchestration) that performs the migration module-by-module: success criteria
and tests are written **before** the Rust implementation for each unit.

Source size: ~39,269 LOC of Go across 128 files (73 non-test, 55 test), 14 providers, a full
on-chain execution/signing engine, a sqlite cache, CAIP parsing, and a large CLI surface (~26
top-level/grouped commands).

## 2. The non-negotiable contract (success oracle)

The port is "correct" iff it preserves the stable machine contract. These are the success
criteria the tests assert against — derived from the contract, **not** from Go internals.

### 2.1 Envelope (`internal/model/types.go`)
```
{ version, success, data?, error, warnings?, meta{ request_id, timestamp, command,
  providers?, cache{status,age_ms,stale}, partial } }
```
- `EnvelopeVersion = "v1"`.
- `data` omitted when empty; `error` always present (null on success); `warnings`/`providers`
  omitted when empty.
- Error output **always returns the full envelope**, even with `--results-only`/`--select`.

### 2.2 Exit codes (`internal/errors/errors.go`) — stable map
| Code | Meaning | | Code | Meaning |
|---|---|---|---|---|
| 0 | success | | 14 | stale |
| 1 | internal | | 15 | partial (strict) |
| 2 | usage | | 16 | blocked |
| 10 | auth | | 20 | action plan |
| 11 | rate limited | | 21 | action sim |
| 12 | unavailable | | 22 | action policy |
| 13 | unsupported | | 23 | action timeout |
| | | | 24 | signer |

Unknown/untyped error → `1` (internal). `ExitCode(nil) == 0`.

### 2.3 Rendering (`internal/out/render.go`)
- **JSON**: 2-space indent; struct field **declaration order** preserved. serde gives this for
  free for structs; ordered maps need `indexmap`/`serde_json` `preserve_order`.
- **Plain**: for maps, keys are **sorted alphabetically**, emitted as `k=v` space-joined, one
  line per slice element; empty slice prints `[]`; scalars print their JSON form.
- `--results-only`: render `data` only (json or plain) — but errors still print full envelope.
- `--select f1,f2`: project named top-level fields over object or array-of-objects (`project`/
  `projectMap`), preserving requested order.

### 2.4 IDs & amounts (`internal/id/`)
- `--chain` accepts CAIP-2, numeric chain IDs, and a fixed alias set (tempo/presto/moderato,
  mantle, megaeth, ink, scroll, berachain, gnosis/xdai, linea, sonic, blast, fraxtal,
  world-chain, celo, taiko, zksync, hyperevm, monad, citrea, …).
- Amounts carry both `amount_base_units` and `amount_decimal` + `decimals`, kept consistent.
- Symbol parsing uses the local bootstrap token registry; unresolved symbols fall through to
  symbol filters / require address or CAIP-19.

### 2.5 Behavioral invariants (from AGENTS.md, must be preserved)
- Config precedence: `flags > env > config file > defaults`.
- Cache: fresh hit (`age <= ttl`) skips provider calls; expired re-fetches; stale served only
  within `max_stale` on temporary provider failure. Metadata + execution commands bypass cache
  init.
- Multi-provider paths require explicit `--provider`; no implicit defaults.
- Key-gated routes stay callable as metadata without keys (`providers list`).
- APY values are percentage points (2.3 == 2.3%), not ratios.

## 3. Target Rust architecture

A Cargo **workspace under `rust/`** (alongside the Go tree, which stays as the reference oracle
until the user decides to delete it). Idiomatic layered crates — *not* a 1:1 file
transliteration. Rust forbids dependency cycles, so the Go provider↔execution coupling is
broken via traits.

```
rust/
  Cargo.toml                       # [workspace] + pinned shared deps (workspace.dependencies)
  rust-toolchain.toml              # pinned stable toolchain
  crates/
    defi-errors/    # Code enum + typed Error → exit codes (thiserror). deps: —
    defi-schema/    # machine-readable command schema (serde). deps: —
    defi-policy/    # command allowlist. deps: —
    defi-id/        # CAIP-2/19, chain aliases, amount normalization, bootstrap tokens.
                    #   deps: alloy-primitives, ruint/num-bigint
    defi-model/     # envelope + all domain structs (serde, declaration-order). deps: serde, chrono
    defi-evm/       # alloy wrappers: address parse/validate, ABI encode, RPC client, signing.
                    #   deps: alloy stack
    defi-config/    # defaults/file/env/flags precedence. deps: serde, serde_yaml, defi-errors
    defi-httpx/     # reqwest client + retry/backoff. deps: reqwest, tokio
    defi-cache/     # sqlite cache + file lock. deps: rusqlite(bundled), fd-lock, defi-errors
    defi-registry/  # endpoints/contracts/ABIs (sol!) + default RPC map. deps: defi-id, defi-evm
    defi-out/       # json/plain render + projection. deps: defi-model, defi-config, indexmap
    defi-ows/       # Open Wallet Standard backend client. deps: defi-httpx, defi-evm
    defi-execution/ # Action types + planners + signer + evm/tempo executors + estimate + policy;
                    #   defines SwapActionBuilder/BridgeActionBuilder traits (cycle break).
                    #   deps: defi-evm, defi-model, defi-id, defi-registry, defi-cache
    defi-providers/ # 14 adapters as modules + provider traits + normalize; impl builder traits.
                    #   deps: defi-model, defi-id, defi-httpx, defi-registry, defi-evm, defi-execution
    defi-app/       # command wiring (clap), provider routing, cache flow. deps: all libs
    defi-cli/       # thin binary, tokio main → app. deps: defi-app, tokio
```

### 3.1 Dependency mapping (Go → Rust)
| Go | Rust |
|---|---|
| cobra / pflag | `clap` (derive) |
| go-ethereum (abi, rlp, crypto, types, ethclient) | `alloy` (alloy-primitives, alloy-sol-types, alloy-consensus, alloy-signer-local, alloy-provider/alloy-rpc-*) |
| tempoxyz/tempo-go (type 0x76 tx) | bespoke encoder on alloy primitives + shell-out to `tempo` CLI for `--signer tempo` |
| modernc.org/sqlite | `rusqlite` (bundled feature) |
| gofrs/flock | `fd-lock` |
| gopkg.in/yaml.v3 | `serde_yaml` |
| net/http + retry (`internal/httpx`) | `reqwest` + manual/tower retry; async via `tokio` |
| math/big, holiman/uint256 | `alloy-primitives` `U256` / `ruint`; arbitrary → `num-bigint` |
| encoding/json | `serde` + `serde_json` (+ `indexmap` for ordered maps) |
| testing/httptest | `wiremock`; CLI golden via `assert_cmd` + `insta` |

### 3.2 Topological build layers (drives the fan-out order)
- **L0** (no internal deps): `defi-errors`, `defi-schema`, `defi-policy`
- **L1**: `defi-id`, `defi-model`, `defi-evm`
- **L2**: `defi-config`, `defi-httpx`, `defi-cache`, `defi-registry`
- **L3**: `defi-out`, `defi-ows`, `defi-execution`
- **L4**: `defi-providers`
- **L5**: `defi-app`
- **L6**: `defi-cli`

Within a layer, crates are independent → parallelize. Across layers → sequential. Large crates
(`defi-providers` ≈ 8.4k LOC over 14 adapters; `defi-execution` ≈ 5.6k; `defi-app` ≈ 5.9k) are
split into per-module pipeline items writing **disjoint files**; their `lib.rs`/`mod.rs` trees
are scaffolded up front so parallel agents never edit the same file.

## 4. TDD method (criteria → tests → code)

For each unit, in order: (1) write the success criteria, (2) write failing tests, (3) implement
until green. Test sources, priority order:

1. **Golden CLI fixtures (primary oracle).** Build the Go binary now; capture stdout + exit
   code for every deterministic offline command (`version`, `schema`, `providers list`,
   `chains list`, `id resolve`, amount/CAIP parsing, `--select`/`--results-only` variants). The
   Rust CLI must match **byte-for-byte** (`assert_cmd` + `insta`). Offline & deterministic.
2. **Ported behavioral tests.** The 55 Go `_test.go` files are mostly `httptest`-mocked adapter
   tests. Re-express the **meaningful** ones in Rust with `wiremock` (deterministic, offline).
   **Skip** pure-internal-detail tests that would calcify poor shape into the new code.
3. **Fresh spec-driven unit tests.** Envelope shape, exit-code mapping, field ordering, plain
   key-sorting, projection, CAIP round-trips, config precedence, cache freshness/staleness.

### 4.1 Always-green invariant
The workspace compiles and `cargo test` passes at **every** checkpoint. A crate is wired into
the build only once its own tests pass **and** `cargo clippy -D warnings` + `cargo fmt --check`
are clean. Units that cannot converge stay compiling stubs and are recorded in the **remainder
plan** — "done" never means a broken tree.

## 5. Workflow shape (deterministic, layered, topological)

```
Phase 0  Analyze    fan-out: 1 reader per Go module → structured "module contract"
                    (public surface, behaviors, success criteria, which Go tests are worth
                     porting, dep-mapping notes). Plus: build Go binary + generate golden fixtures.
Phase 1  Scaffold   workspace + all crate manifests + full mod trees as compiling stubs +
                    pinned deps + CI config. Tree green & empty.
Phase 2  Migrate    layered topological pipeline (L0→L6). Within a layer, units run in parallel
                    (disjoint files). Per unit: RED (criteria+tests) → GREEN (implement until
                    cargo test -p + clippy + fmt pass) → VERIFY (adversarial: tests meaningful?
                    output matches Go golden?). Loop-until-green w/ bounded retries; non-
                    converging units → stub + remainder list.
Phase 3  Integrate  golden end-to-end diff of Rust CLI vs Go binary across the command surface;
                    wire defi-app + defi-cli.
Phase 4  Verify     full workspace: cargo test, clippy -D warnings, fmt --check; tests also pass
                    under --release. Report green/deferred per crate.
Phase 5  Remainder  honest written plan for anything deferred (live-API commands, exotic signing).
```

The workflow iterates over the **known module inventory** (deterministic work-list), not a
budget loop, so behavior is reproducible.

### 5.1 Module inventory (work-list)
| Go module | LOC | Target crate / module | Layer |
|---|---|---|---|
| internal/errors | 69 | defi-errors | L0 |
| internal/schema | 535 | defi-schema | L0 |
| internal/policy | 25 | defi-policy | L0 |
| internal/id | 867 | defi-id | L1 |
| internal/model | 418 | defi-model | L1 |
| (go-ethereum usage) | — | defi-evm | L1 |
| internal/config | 404 | defi-config | L2 |
| internal/httpx | 156 | defi-httpx | L2 |
| internal/cache (+fsutil) | 281 | defi-cache | L2 |
| internal/registry | 522 | defi-registry | L2 |
| internal/out | 145 | defi-out | L3 |
| internal/ows | 284 | defi-ows | L3 |
| internal/execution | 5,562 | defi-execution (store, planner, signer, evm_executor, tempo_executor, estimate, policy, actionbuilder) | L3 |
| internal/providers | 8,374 | defi-providers (aave, morpho, moonwell, kamino, defillama, across, lifi, tempo, bungee, taikoswap, uniswap, jupiter, fibrous, oneinch, yieldutil, normalize, types) | L4 |
| internal/app | 5,856 | defi-app (per command group: providers, chains, lend, yield, swap, bridge, transfer, approvals, rewards, actions, wallet, protocols, stablecoins, dexes, version, schema, runner/cache flow) | L5 |
| cmd/defi | 12 | defi-cli | L6 |

## 6. Definition of done

- `rust/` workspace **compiles**; `cargo test` **passes**; `cargo clippy --all-targets -- -D
  warnings` **clean**; `cargo fmt --all --check` **clean**; tests also pass under `--release`.
- Golden tests prove byte-stable contract parity for all covered commands.
- Every crate is either ✅ complete+tested or ⏸️ documented in the remainder plan — never
  silently broken.
- Design spec + implementation plan committed; Go tree untouched (reference oracle).

## 7. Key risks

- **Scale**: 39k LOC incl. crypto + on-chain execution. One run likely won't finish all of it
  clean; the always-green invariant + remainder plan keep this honest.
- **alloy ≠ go-ethereum** API-for-API; signing/ABI/RLP need golden parity on encoded tx bytes.
- **Tempo 0x76 + OWS** are bespoke; covered by shell-out parity + fixtures.
- **Live-API commands** can't be golden-tested deterministically; covered by wiremock +
  structural assertions only.
- **Float formatting**: Go `float64` JSON vs Rust `f64` serialization must match (e.g. trailing
  zeros, integer-valued floats). Golden tests catch drift; may need a custom serializer.
