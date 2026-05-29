# defi-cli Go → Rust Migration — Final Verification + Remainder Plan

**Date:** 2026-05-28
**Branch:** `migrate-to-rust`
**Authoritative references:**
- Spec (contract + architecture): `docs/superpowers/specs/2026-05-28-rust-migration-design.md`
- Plan (phases, locked file/crate structure, TDD cycle): `docs/superpowers/plans/2026-05-28-rust-migration.md`

This document closes Phase 5 of the migration. It records the final verification status, a
per-crate completion table, the precise remainder (deferred units + Go-only tasks) with next
TDD steps, and a "how to resume" note.

---

## 1. Final verification status

Run from `rust/` (Phase 4, Step 4.1 — all five gates green):

```bash
cargo fmt --all --check                              # clean
cargo clippy --all-targets --all-features -- -D warnings  # clean
cargo test --workspace                               # 1248 passed / 0 failed
cargo test --workspace --release                     # 1248 passed / 0 failed
cargo build --workspace --release                    # ok (binary at target/release/defi)
```

Release smoke:
- `target/release/defi version` → `0.5.0` (exit 0)
- `target/release/defi providers list --results-only` → valid JSON (exit 0)

**Always-green invariant held throughout.** No `todo!()` / `unimplemented!()` stubs remain in
any crate's `src/`. The Go tree was never modified (it remains the reference oracle); the only
transient artifact is the gitignored `./defi` Go oracle binary, which is not committed.

> Note: two test-only whitespace fixes (`crates/defi-cache/src/store.rs`,
> `crates/defi-schema/src/tests.rs`) were applied by `cargo fmt --all` during Phase 4 and are
> included in the final commit. No library/contract logic changed.

---

## 2. Per-crate completion table

All 16 workspace crates are **✅ complete + tested**. Test counts are per-crate
(`cargo test -p <crate>`, includes unit + integration + (where present) golden targets; all with
0 failures). The authoritative workspace total is **1248 passed / 0 failed** in both debug and
release; per-crate counts overlap slightly because integration/doctest binaries are attributed
per crate.

| Layer | Crate | Status | Tests | Notes |
|---|---|---|---|---|
| L0 | `defi-errors` | ✅ complete | 18 | Code enum + exit-code map; `errors.As`-equivalent `find` |
| L0 | `defi-schema` | ✅ complete | 32 | serde data model + clap-free helpers; full golden round-trip |
| L0 | `defi-policy` | ✅ complete | 10 | allowlist predicate; `Code::Blocked` (16) |
| L1 | `defi-id` | ✅ complete | 108 | CAIP-2/19, chain aliases, amount normalization, bootstrap tokens |
| L1 | `defi-model` | ✅ complete | 45 | envelope + domain structs; `go_float` parity serializer |
| L1 | `defi-evm` | ✅ complete | 113 | address/ABI/RPC/signer; go-ethereum-parity goldens |
| L2 | `defi-config` | ✅ complete | 35 | flags>env>file>defaults precedence; Go duration parse |
| L2 | `defi-httpx` | ✅ complete | 25 | reqwest + retry/backoff; status→Code mapping |
| L2 | `defi-cache` | ✅ complete | 19 | sqlite + fd-lock; freshness/staleness invariant |
| L2 | `defi-registry` | ✅ complete | 39 | endpoints/contracts/ABIs/RPC map; bridge-target allowlists |
| L3 | `defi-out` | ✅ complete | 30 | json/plain render + projection; `format_go_g` parity |
| L3 | `defi-ows` | ✅ complete | 35 | OWS backend client (shell-out); policy/signer classification |
| L3 | `defi-execution` | ✅ complete | 225 | action/store/planner/signer/executors/estimate/policy/builder |
| L4 | `defi-providers` | ✅ complete | 201 | 14 adapters + traits + normalize; wiremock-backed |
| L5 | `defi-app` | ✅ complete | 283 | runner+cache flow + all command-group modules + golden CLI |
| L6 | `defi-cli` | ✅ complete | 30 | thin tokio binary; OS-boundary exit-code cast |

**Deferred units (⏸️):** none at the crate level. Two *integration-scoped* deferrals exist
inside otherwise-complete crates (`defi-app`) — see §3.

---

## 3. Deferred units (integration-scoped, inside complete crates)

These are not broken or untested code — the per-module logic is implemented and unit-tested. The
deferral is purely the final *wiring* / *whole-document parity* step, which is integration work
left for a follow-up pass. Each is tracked here with the precise blocker and next TDD steps.

### 3.1 ⏸️ `defi-app::cli::run()` — live/cache-backed command routing

**What's done:** Every command-group module is implemented and green at the module level
(`providers`, `chains`, `lend`, `yield`, `swap`, `bridge`, `transfer`, `approvals`, `rewards`,
`actions`, `wallet`, `protocols`, `stablecoins`, `dexes`, `version`, `schema`). The cache flow
(`runner::run_cached_command`, `emit_success`, `render_error`), provider selection, and
exit-code mapping are all implemented and tested.

**What's deferred:** `defi-app/src/cli.rs::dispatch()` currently only routes the deterministic
*offline* surface needed for the golden oracle:
- `providers list`
- `chains list`
- `assets resolve`
- `schema [path]` (partial tree — see §3.2)

All live/cache-backed groups (`lend`, `yield`, `swap`, `bridge`, `transfer`, `approvals`,
`rewards`, `actions`, `wallet`, `protocols`, `stablecoins`, `dexes`, `chains gas`) have working
module functions but are **not yet matched in `dispatch()`**. Invoking them today returns the
"unknown command" usage error (exit 2).

**Blocker:** the full clap argument surface (per-group flags, enums, input-mode `--input-json` /
`--input-file`, `--rpc-url`, provider selectors) has not been wired into the arg parser
(`cli.rs::Parsed`), and the cache `Store` / provider client construction has not been threaded
into `dispatch()`. This is mechanical glue, not new domain logic.

**Next TDD steps:**
1. RED: extend `defi-app/tests/golden_cli.rs` (or add `tests/live_cli.rs` with `wiremock` +
   injected `--rpc-url` / base-URL env seams already present on every provider `Client`) to
   assert envelope shape + exit codes for one command per group against mocked providers. Use the
   provider modules' existing `set_base_url`/`set_endpoint` test seams; do NOT hit live APIs.
2. GREEN: flesh out `cli.rs::Parsed` to parse each group's flags, construct the provider/cache
   plumbing, and add the `match cmd.as_slice()` arms calling the already-green module functions
   through `runner::run_cached_command` (cached groups) or directly (execution groups, which
   bypass cache init per spec §2.5).
3. VERIFY: confirm cache-state transitions (fresh hit / TTL re-fetch / stale fallback / stale
   budget) appear in `meta.cache`, that execution commands bypass cache init, and that
   `--provider` is required on multi-provider paths (no implicit default).

**Contract to preserve:** spec §2.1 (envelope), §2.2 (exit codes), §2.5 (cache + multi-provider
+ key-gating invariants), §2.3 (`--results-only`/`--select`).

### 3.2 ⏸️ `defi-app::schema` — whole-document `schema.json` golden parity

**What's done:** The cobra-style command-tree walk (`Build`/`serialize`/`collectFlags`) is ported
over a clap-independent `CommandNode`/`FlagSpec` model with full unit-test parity: alphabetical
flag ordering, inherited-vs-local scope, hidden-flag/subcommand dropping, metadata propagation,
enum inference, and **byte-for-byte golden parity of the `version` and `schema` subtrees** against
`rust/tests/golden/schema.json`.

**What's deferred:** whole-document parity. The Go `schema.json` fixture is the full 19-command
tree (~958,687 bytes); the Rust `schema` command currently emits only the partial
`defi`/`schema`/`version` subtree (~8 KB) because the complete clap command tree is not yet
populated at runner wiring time. `golden_cli.rs` therefore asserts `schema` only at the
**structural/envelope level** (version, success, error=null, `data.path`, `data.use`, top-level
key declaration order, exit 0, stdout) — explicitly *not* byte-for-byte against the Go golden.

**Blocker:** depends on §3.1 — the full command tree (every group + flag, with metadata) must be
declared as `CommandNode`s, which is the same wiring work as routing.

**Next TDD steps:**
1. RED: add a `defi-app` test that builds the *complete* `CommandNode` tree and asserts the
   serialized document is byte-for-byte equal to `rust/tests/golden/schema.json` (after the
   existing volatile-field normalization for the envelope wrapper).
2. GREEN: populate every command/flag node (reuse `root_persistent_flags()`, `version_node()`,
   `schema_node()` as the pattern) with the same metadata the Go cobra annotations carry
   (`mutation`, `auth`, `required`, `enum`, `format`, `input_modes`, request/response hints).
3. VERIFY: diff against the Go binary's `schema` output; confirm flag ordering (cobra `VisitAll`
   alphabetical), inherited-flag scope, and hidden-node dropping match exactly.

---

## 4. Remaining Go-only tasks (not part of the always-green Rust workspace)

These are real follow-ups required before the Rust port can fully replace the Go binary. None of
them are blocking the always-green invariant; they are net-new work beyond the migrated library
surface.

### 4.1 Live-API command coverage strategy

Live commands (anything that calls a real provider/RPC) cannot be golden-tested deterministically
(spec §7). The strategy, already proven by the L4 adapter tests, is:
- **Per-adapter wiremock tests** (offline, deterministic) — already complete in `defi-providers`
  (201 tests) and `defi-execution` (225 tests). Every provider `Client` exposes a
  `set_base_url`/`set_endpoint`/`set_now` seam, and every RPC-backed path is mocked via wiremock.
- **App-level live routing tests** — deferred with §3.1: once `dispatch()` routes the live
  groups, add app-level `wiremock` tests (inject base URLs / `--rpc-url`) asserting the full
  envelope (not just the adapter return), `meta.providers[]` status, `meta.cache` transitions,
  and exit codes. Keep these offline.
- **Optional smoke job** — a manually-triggered (not on every CI run) job that hits a small set
  of real endpoints, mirroring the Go `nightly-execution-smoke.yml`, to catch upstream drift.
  Allowed to be best-effort / non-blocking.

### 4.2 Exotic signing parity (Tempo 0x76, OWS, alloy tx-byte parity)

- **alloy EIP-1559 tx-byte parity** — DONE and verified at the byte level in `defi-evm::signer`
  and `defi-execution::evm_executor` (EIP-2718 `0x02` type byte, chain-id binding, recover-to-
  address, RFC-6979 determinism) against a freshly built go-ethereum v1.16.8 oracle. ABI calldata
  goldens (ERC20/Aave/Morpho-tuple/Multicall3/Comptroller) match `abi.Pack` byte-for-byte.
- **Tempo type-0x76 transactions** — the signer contract (`defi-execution::signer::TempoTx`,
  `tempo_executor`) recovers to the key EOA and the batched-call construction
  (`build_tempo_calls`) matches Go `decodeHex`/value semantics, but the **exact tempo-go RLP byte
  layout** for the on-wire 0x76 envelope is intentionally scoped to `tempo_executor` and has NOT
  been pinned against a tempo-go oracle. Next TDD step: build a tempo-go reference, capture signed
  0x76 tx bytes for a fixed key + calls + fee-token, and add a byte-for-byte parity test;
  reconcile any RLP field-ordering/encoding differences.
- **OWS (`--signer tempo` / `--wallet`)** — the `defi-ows` client (shell-out to `ows`/`tempo`
  CLIs) and the `defi-execution::evm_executor::OwsSubmitBackend` are implemented and unit-tested
  with an injectable command runner (arg-vector, `OWS_PASSPHRASE` env, policy-denial vs signer
  classification, `tx_hash` parsing). Next step (integration): an end-to-end test against a real
  or emulated `ows` binary to confirm the actual CLI arg/JSON contract, since the unit tests mock
  the command runner. Also wire `--signer tempo` / `--wallet` flags into `dispatch()` (§3.1).

### 4.3 Docs / README / CHANGELOG sync

The user-facing surface still documents only the Go binary. Once the Rust binary is the shipped
artifact:
- **README.md** — update build/install/usage to the Rust binary (`cargo build`/`cargo install`
  or release artifact), keeping the agent-first JSON-contract caveats.
- **AGENTS.md / CLAUDE.md** — update the "First 5 minutes" build commands and folder structure to
  reflect `rust/`; note Go→Rust crate mapping. Keep the contract caveats (they are unchanged by
  the port — that is the whole point).
- **CHANGELOG.md** — add an `Unreleased` entry under `Changed` noting the Rust reimplementation
  (no contract change) per the changelog workflow in AGENTS.md.
- **Mintlify docs (`docs/docs.json` + `docs/**/*.mdx`)** — only the build/install pages need
  edits; command/contract reference pages are unchanged because the machine contract is preserved.
  Run the docs-site checks from `docs/` (`mint validate` / `broken-links` / `a11y`).

### 4.4 `.goreleaser` / install script updates

- **`.goreleaser.yml`** — currently builds the Go binary cross-platform. Replace (or add a parallel
  pipeline) to build the Rust binary for the same target matrix
  (linux/darwin × amd64/arm64). Options: `cargo-dist`, `cross`, or a hand-rolled matrix in the
  release workflow. Keep the artifact name (`defi`) and the release-asset naming that
  `scripts/install.sh` expects, or update both together.
- **`scripts/install.sh`** — the macOS/Linux installer downloads the latest tagged release asset.
  If artifact naming changes with the Rust build, update the asset-name resolution accordingly.
  Preserve: writable user-space install dir (fallback `~/.local/bin`), never-sudo default.
- **`.github/workflows/release.yml`** — currently GoReleaser-driven on `v*` tags (and force-syncs
  `docs-live` for stable releases). Update to invoke the Rust release pipeline; keep the
  `docs-live` sync behavior.

### 4.5 Rust CI workflow

`.github/workflows/ci.yml` currently runs Go (`go test`/`go vet`/`go build`) on
ubuntu+macos. Add a Rust CI job (or a new `.github/workflows/rust-ci.yml`) mirroring the Phase-4
gates, on the same OS matrix:

```yaml
# sketch — rust-ci.yml
on: { push: { branches: ["**"] }, pull_request: {} }
jobs:
  rust:
    strategy: { fail-fast: false, matrix: { os: [ubuntu-latest, macos-latest] } }
    runs-on: ${{ matrix.os }}
    defaults: { run: { working-directory: rust } }
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable          # honors rust-toolchain.toml (channel=stable)
        with: { components: rustfmt, clippy }
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --all-targets --all-features -- -D warnings
      - run: cargo test --workspace
      - run: cargo test --workspace --release
      - run: cargo build --workspace --release
```

Keep the Go CI job until the Go tree is retired, so both stay green during the transition.

---

## 5. How to resume

1. **Read the contract first.** Start with the spec
   (`docs/superpowers/specs/2026-05-28-rust-migration-design.md`) §2 (non-negotiable contract:
   envelope, exit codes, rendering, ids/amounts, behavioral invariants) and the plan
   (`docs/superpowers/plans/2026-05-28-rust-migration.md`) for the locked file/crate structure and
   TDD cycle. **Do not change the machine contract.** Preserve: envelope shape; stable exit codes;
   JSON 2-space indent with struct field *declaration* order; plain-output map keys sorted
   *alphabetically*; base-unit + decimal amounts; CAIP ids.

2. **Pick up the deferrals in order.** §3.1 (wire `dispatch()` for live/cache groups) unblocks
   §3.2 (whole-document `schema.json` parity) and §4.1 (app-level live routing tests). Each follows
   the same RED → GREEN → VERIFY micro-cycle, with the Go binary (`go build -o defi ./cmd/defi`,
   gitignored/transient) as the golden oracle and `wiremock` for offline determinism.

3. **Honor the always-green invariant.** Run the Phase-4 gates (§1) before every commit:
   `cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings &&
   cargo test --workspace && cargo test --workspace --release`. No `unwrap`/`expect`/`panic` in
   non-test lib code; errors via `Result` + `defi_errors::Error`.

4. **Golden normalization.** When adding golden tests, follow `rust/tests/golden/README.md`: blank
   the volatile fields (`meta.request_id`, `meta.timestamp`, `meta.cache.age_ms`,
   `meta.providers[].latency_ms`, any `*fetched_at*`) on both sides, then compare with
   declaration-order preserved (do NOT sort JSON keys — ordering is part of the contract).
   `version`/`version --long` are raw strings, not envelopes (compare shape, not the literal
   release version).

5. **Go-only follow-ups** (§4) are net-new and can proceed independently once the Rust binary is
   the shipping artifact: live-API coverage, Tempo 0x76 / OWS integration parity, docs/README/
   CHANGELOG sync, `.goreleaser`/install-script swap, and the Rust CI workflow.
