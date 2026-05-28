# defi-cli Go → Rust Migration — Implementation Plan

> **For agentic workers:** This plan is executed by a **Workflow** (the Workflow tool). The
> workflow is the executor — it dispatches RED/GREEN/VERIFY subagents per crate/module. Steps
> use checkbox (`- [ ]`) syntax for phase tracking. The per-unit Rust + test code is generated
> at runtime by the workflow's agents from the Phase-0 module contracts; this plan locks down
> structure, ordering, the TDD cycle, and verification gates.

**Goal:** Port `defi-cli` from Go to an idiomatic Rust Cargo workspace under `rust/`, preserving
the stable machine contract, using TDD (success criteria + tests before code) for every unit.

**Architecture:** Layered Cargo workspace (~16 crates, L0→L6 topological order). Bottom-up
migration: a crate is wired into the build only when its tests + clippy + fmt are clean
(always-green invariant). The Go tree stays as the reference oracle for golden tests.

**Tech Stack:** Rust (clap, alloy, reqwest, tokio, rusqlite, serde/serde_json/serde_yaml,
indexmap, thiserror, fd-lock; dev: wiremock, assert_cmd, insta). Reference: spec at
`docs/superpowers/specs/2026-05-28-rust-migration-design.md`.

---

## File / crate structure (locked)

```
rust/
  Cargo.toml              # [workspace], members = crates/*, [workspace.dependencies] pinned
  rust-toolchain.toml     # channel = "stable"
  .gitignore              # /target
  crates/
    defi-errors/   src/lib.rs                      # L0  Code enum + Error + exit_code()
    defi-schema/   src/lib.rs                      # L0  command schema model + builder
    defi-policy/   src/lib.rs                      # L0  allowlist
    defi-id/       src/lib.rs caip.rs chain.rs amount.rs tokens.rs   # L1
    defi-model/    src/lib.rs envelope.rs domain.rs # L1
    defi-evm/      src/lib.rs address.rs abi.rs rpc.rs signer.rs     # L1
    defi-config/   src/lib.rs                      # L2  precedence flags>env>file>defaults
    defi-httpx/    src/lib.rs                      # L2  reqwest client + retry
    defi-cache/    src/lib.rs store.rs lock.rs     # L2  sqlite + file lock
    defi-registry/ src/lib.rs                      # L2  endpoints/contracts/ABIs/RPC map
    defi-out/      src/lib.rs                      # L3  json/plain render + projection
    defi-ows/      src/lib.rs                      # L3  OWS backend client
    defi-execution/ src/lib.rs action.rs store.rs planner/ signer/ evm_executor.rs
                    tempo_executor.rs estimate.rs policy.rs builder.rs  # L3
    defi-providers/ src/lib.rs traits.rs normalize.rs <one module per provider>  # L4
    defi-app/      src/lib.rs runner.rs <one module per command group>  # L5
    defi-cli/      src/main.rs                     # L6  tokio main → defi_app::run
```

**Interface contracts locked at scaffold (so parallel agents agree on signatures):**
- `defi_errors::Code` (repr i32, exact values from spec §2.2) + `defi_errors::Error{code,message,cause}` + `pub fn exit_code(err: &Result<(),Error>) -> i32`.
- `defi_model::Envelope` and all domain structs: serde field names + declaration order copied
  exactly from `internal/model/types.go`.
- `defi_providers::traits` — one trait per Go provider interface (`MarketDataProvider`,
  `LendingProvider`, `LendingPositionsProvider`, `YieldProvider`, `YieldPositionsProvider`,
  `YieldHistoryProvider`, `BridgeProvider`, `BridgeDataProvider`, `BridgeExecutionProvider`,
  `SwapProvider`, `SwapExecutionProvider`), async (`async-trait` or RPITIT).
- `defi_execution::{Action, SwapActionBuilder, BridgeActionBuilder}` — Action types live here;
  builder traits here too (breaks the Go provider↔execution cycle; providers impl them).

---

## Phase 0: Analyze & fixtures

- [ ] **Step 0.1: Build the Go reference binary**

Run: `cd /Users/gustavo/apps/defi-cli-worktrees/migrate-to-rust && go build -o defi ./cmd/defi`
Expected: binary `./defi` produced, exit 0. (Not committed — it's a transient oracle.)

- [ ] **Step 0.2: Generate golden fixtures (deterministic offline commands)**

For each command below, capture stdout + exit code to `rust/tests/golden/<name>.json` and
`<name>.exit`:
```
version
schema
providers list --results-only
chains list
chains list --results-only
id resolve USDC --chain 1            # if supported; else nearest deterministic id cmd
```
These are byte-stable, no network. Live-API commands are NOT golden fixtures (handled by
wiremock in their crates).

- [ ] **Step 0.3: Per-module contracts (fan-out, 1 reader per Go module)**

Each reader subagent reads its assigned Go module and emits a structured **module contract**:
public surface to reproduce, behaviors/invariants, success criteria, the specific Go `_test.go`
cases worth porting (and which to skip as internal-detail), Go→Rust dep notes, and any
contract-relevant quirks (float formatting, ordering, omitempty). One contract per crate/module
in the inventory (§ work-list below).

**Gate:** module contracts exist for every work-list item; golden fixtures captured.

---

## Phase 1: Scaffold (always-green empty workspace)

- [ ] **Step 1.1: Write workspace + toolchain files**

Create `rust/Cargo.toml` (`[workspace]`, `resolver = "2"`, `members = ["crates/*"]`,
`[workspace.dependencies]` pinning every shared dep), `rust/rust-toolchain.toml`
(`[toolchain] channel = "stable"`), `rust/.gitignore` (`/target`).

- [ ] **Step 1.2: Scaffold every crate as a compiling stub**

For each crate: `Cargo.toml` (deps from §3.1 of spec, via `workspace = true`) + full module tree
(`lib.rs` with all `pub mod` declarations + the locked interface types/trait signatures as
`todo!()`-free minimal stubs — empty structs/enums + trait method signatures). Scaffolding the
full `mod` tree up front means Phase-2 agents only edit their own module file, never `lib.rs`.

- [ ] **Step 1.3: Verify the empty workspace is green**

Run: `cd rust && cargo build --workspace && cargo fmt --all --check && cargo clippy --all-targets -- -D warnings`
Expected: builds clean, no warnings. (Stubs compile; no tests yet.)

- [ ] **Step 1.4: Commit scaffold**

```bash
git add rust && git commit -m "chore(rust): scaffold cargo workspace (empty, green)"
```

**Gate:** `cargo build --workspace` green; clippy/fmt clean.

---

## Phase 2: TDD migration (layered topological fan-out)

Process layers **L0 → L6 in sequence**. Within a layer, units run **in parallel** (disjoint
files). Each unit (crate, or module of a large crate) runs this **TDD micro-cycle**:

- [ ] **RED — write success criteria + failing tests**
  Agent input: the module contract + golden fixtures + locked interfaces. Output: idiomatic Rust
  tests (port *meaningful* Go cases via `wiremock`; add fresh spec-driven tests; wire golden
  fixtures for offline commands). Skip internal-detail tests that would harm code quality.
  Verify they FAIL: `cargo test -p <crate> <module>` → compile error or assertion failure.

- [ ] **GREEN — implement until green**
  Implement the unit's real code. Loop until ALL pass:
  `cargo test -p <crate>` && `cargo clippy -p <crate> --all-targets -- -D warnings`
  && `cargo fmt -p <crate> --check`. Bounded retries.

- [ ] **VERIFY — adversarial check**
  Separate agent: are the tests meaningful (not tautological)? Does output match the Go golden
  byte-for-byte where applicable? Are contract invariants (ordering, exit codes, float format)
  actually asserted? If it finds a gap, feed back one more GREEN pass.

- [ ] **Non-convergence → defer**
  If a unit cannot go green within bounded retries, leave it as a compiling stub, record it in
  the remainder list with the blocker, and continue. The workspace stays green.

**Layer order & units:**
- [ ] **L0:** `defi-errors`, `defi-schema`, `defi-policy`
- [ ] **L1:** `defi-id` (caip, chain, amount, tokens), `defi-model` (envelope, domain), `defi-evm` (address, abi, rpc, signer)
- [ ] **L2:** `defi-config`, `defi-httpx`, `defi-cache` (store, lock), `defi-registry`
- [ ] **L3:** `defi-out`, `defi-ows`, `defi-execution` (action, store, planner, signer, evm_executor, tempo_executor, estimate, policy, builder)
- [ ] **L4:** `defi-providers` — modules: traits, normalize, then per provider: defillama, aave, morpho, moonwell, kamino, across, lifi, tempo, bungee, taikoswap, uniswap, jupiter, fibrous, oneinch, yieldutil
- [ ] **L5:** `defi-app` — modules: runner+cache flow, then per command group: providers, chains, lend, yield, swap, bridge, transfer, approvals, rewards, actions, wallet, protocols, stablecoins, dexes, version, schema
- [ ] **L6:** `defi-cli` (`main.rs` → `defi_app::run`)

Commit after each layer goes green: `git add rust && git commit -m "feat(rust): port <layer> (TDD, green)"`.

**Gate per layer:** `cargo test --workspace` passes for all completed crates; clippy/fmt clean.

---

## Phase 3: Integration (golden parity end-to-end)

- [ ] **Step 3.1: End-to-end golden diff**
  Run the same offline command set through the Rust binary (`cargo run -p defi-cli -- <args>`)
  and diff stdout + exit code against the Phase-0 Go golden fixtures. Add an integration test
  (`rust/tests/golden_cli.rs` with `assert_cmd` + `insta`) asserting parity for every covered
  command, including `--results-only`, `--select`, and an error case (full-envelope-on-error).

- [ ] **Step 3.2: Fix any parity drift**
  Common culprits: float formatting, map key ordering, omitempty, timestamp/request_id
  nondeterminism (inject a fixed clock / request id via env or flag for golden runs, matching
  how the Go runner is made deterministic in its tests).

**Gate:** golden parity tests pass for all covered commands.

---

## Phase 4: Final verification

- [ ] **Step 4.1: Full workspace verification**

Run, expecting all clean:
```bash
cd rust
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --workspace --release
cargo build --workspace --release
```

- [ ] **Step 4.2: Coverage report**
  Produce a per-crate ✅ complete / ⏸️ deferred table with test counts.

**Gate:** fmt/clippy/test/build all clean; coverage table produced.

---

## Phase 5: Remainder plan

- [ ] **Step 5.1: Write the remainder plan**
  `docs/superpowers/plans/2026-05-28-rust-migration-remainder.md` — for every deferred unit:
  the blocker, what's needed to finish, and the next TDD steps. Also note Go-only items still
  pending (live-API command coverage strategy, exotic signing parity, docs/README/CHANGELOG
  sync, `.goreleaser`/install script updates, CI workflow for Rust).

- [ ] **Step 5.2: Commit**
```bash
git add rust docs && git commit -m "docs(rust): final verification + remainder plan"
```

---

## Work-list (module inventory)

| # | Go module | LOC | Target | Layer |
|---|---|---|---|---|
| 1 | internal/errors | 69 | defi-errors | L0 |
| 2 | internal/schema | 535 | defi-schema | L0 |
| 3 | internal/policy | 25 | defi-policy | L0 |
| 4 | internal/id | 867 | defi-id | L1 |
| 5 | internal/model | 418 | defi-model | L1 |
| 6 | (go-ethereum usage) | — | defi-evm | L1 |
| 7 | internal/config | 404 | defi-config | L2 |
| 8 | internal/httpx | 156 | defi-httpx | L2 |
| 9 | internal/cache + fsutil | 281 | defi-cache | L2 |
| 10 | internal/registry | 522 | defi-registry | L2 |
| 11 | internal/out | 145 | defi-out | L3 |
| 12 | internal/ows | 284 | defi-ows | L3 |
| 13 | internal/execution | 5,562 | defi-execution (8 modules) | L3 |
| 14 | internal/providers | 8,374 | defi-providers (15 modules) | L4 |
| 15 | internal/app | 5,856 | defi-app (16 modules) | L5 |
| 16 | cmd/defi | 12 | defi-cli | L6 |

## Self-review notes
- **Spec coverage:** every spec §2 contract item maps to a crate (envelope→model, exit
  codes→errors, render→out, ids/amounts→id, behavioral invariants→config/cache/app). Every
  spec §5.1 inventory row appears in the work-list. Verification gates match spec §6 done-defn.
- **Placeholders:** none — runtime-generated code is intentional (workflow-executed), structure
  and gates are concrete.
- **Type consistency:** interface names locked once in "File/crate structure"; reused verbatim
  in Phase-2 unit lists and the providers trait list.
