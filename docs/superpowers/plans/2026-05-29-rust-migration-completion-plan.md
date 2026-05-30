# defi-cli Rust Migration — Current State & Completion Plan (to 100%)

> **For agentic workers:** This is the authoritative "where we are / what's left" document for the
> Go→Rust port. It supersedes the optimistic framing in `2026-05-28-rust-migration-remainder.md`
> §3 (which understated the app-layer gap as "mechanical glue"). Steps use checkbox (`- [ ]`)
> syntax. Execute with the RED→GREEN→VERIFY TDD cycle; the Go binary (`go build -o defi ./cmd/defi`,
> gitignored/transient) is the golden oracle; `wiremock` provides offline determinism.

**Goal:** Take the Rust port from "library layer complete + 5 CLI commands wired" to "100% migrated
and functional" — every Go command runs end-to-end in the Rust binary with byte-identical contract
output and exit codes, verified, with the Go tree retired.

**Architecture:** The 16-crate layered workspace under `rust/` is built and green. What remains is
overwhelmingly in the **application layer** (`defi-app`): turning ~60 CLI invocations into
provider/RPC calls → envelopes (read commands) and into action build → persist → broadcast →
status (execution commands), plus full `schema` parity, signing-byte parity gaps, and cutover.

**References:** spec `docs/superpowers/specs/2026-05-28-rust-migration-design.md`; original plan
`docs/superpowers/plans/2026-05-28-rust-migration.md`; prior remainder
`docs/superpowers/plans/2026-05-28-rust-migration-remainder.md`.

---

## 1. Executive summary

The **domain/library layer is genuinely done and tested**; the **application/command layer is
mostly unbuilt**. Independently verified on 2026-05-29:

- ✅ `cargo fmt --all --check` clean, `cargo clippy --all-targets --all-features -- -D warnings`
  clean, `cargo test --workspace` = **1248 passed / 0 failed**. 62,435 LOC across 16 crates, no
  `todo!()`/`unimplemented!()` stubs. Go tree untouched.
- ⚠️ The **binary runs only 5 of 66 real commands** end-to-end. Everything else returns
  `unknown command` (exit 2).

**Honest completion estimate:** by command surface, **~8% functional** (5/66 commands wired). By
code volume the library is ~90% of the LOC and is done, but the user-visible CLI is far from a
drop-in replacement. "All crates green" ≠ "migrated and functional."

---

## 2. Current state (verified)

### 2.1 Crate layer — ✅ complete & green
All 16 crates compile and pass tests (debug + release). Library capabilities confirmed present:
provider adapters (14, wiremock-tested, 201 tests), execution engine (planners/signer/executors,
225 tests), `defi-evm` (alloy signing/ABI with go-ethereum byte-parity goldens, 113),
`defi-id`/`defi-model`/`defi-config`/`defi-cache`/`defi-out`/`defi-registry`/`defi-ows`/
`defi-errors`/`defi-schema`/`defi-policy`. The cache flow (`runner::run_cached_command`), provider
selection, exit-code mapping, and rendering all exist and are unit-tested.

### 2.2 Command surface — the real gap

Go has **70 leaf commands** (66 real + `help` + 4 `completion`). Rust binary status today:

**Legend:** ✅ wired & working · 🟡 handler exists, not wired · 🟠 only helpers/fetch exist (handler
missing) · 🔴 not started.

| Command(s) | Count | Status | What exists in `defi-app` today |
|---|---|---|---|
| `version`, `providers list`, `chains list`, `assets resolve`, `schema` (partial tree) | 5 | ✅ | wired in `cli.rs::route()` |
| `protocols top\|categories\|fees\|revenue` | 4 | 🟡 | `run_top/run_categories/run_fees/run_revenue` |
| `stablecoins top\|chains` | 2 | 🟡 | `run_top/run_chains` |
| `dexes volume` | 1 | 🟡 | `run_volume` |
| `chains gas` | 1 | 🟡 | `run_gas` (+ multi-chain `resolve_gas_targets`) |
| `lend positions`, `yield positions`, `wallet balance` | 3 | 🟠 | only `fetch_*` data fns; no envelope+cache handler |
| `lend markets\|rates`, `yield opportunities\|history`, `swap quote`, `bridge quote\|list\|details`, `chains top\|assets` | 11 | 🟠 | only request-parse/validate/sort/limit/dedupe helpers |
| `swap plan\|submit\|status` | 3 | 🔴/🟠 | only `parse_swap_request`, identity/intent helpers |
| `bridge plan\|submit\|status` | 3 | 🟠 | only `build_bridge_request`, identity/intent helpers |
| `lend supply\|withdraw\|borrow\|repay × plan\|submit\|status` | 12 | 🟠 | only `lend_verb_intent` + builders |
| `yield deposit\|withdraw × plan\|submit\|status` | 6 | 🟠 | only `yield_verb_intent` + builders |
| `rewards claim\|compound × plan\|submit\|status` | 6 | 🟠 | only `build_rewards_*_request`, intent helpers |
| `approvals plan\|submit\|status` | 3 | 🟠 | only `build_approval_request`, intent helpers |
| `transfer plan\|submit\|status` | 3 | 🟠 | only `build_transfer_request`, intent helpers |
| `actions list\|show\|estimate` | 3 | 🟠 | only `resolve_action_id`, parse/classify helpers |
| `completion bash\|zsh\|fish\|powershell`, `help` | 5 | 🔴 | none (clap can generate natively) |

**Totals:** ✅ 5 · 🟡 8 (handler-ready) · 🟠 38 (helpers only) · 🔴 ~14 (execution-status/exec + completion). The **arg parser** (`cli.rs::Parsed`) is hand-rolled and only recognizes global flags + a few command flags — per-group flags, enums, `--input-json`/`--input-file`, `--rpc-url`, provider selectors, and the execution flag surface are **not** parsed yet.

### 2.3 Other verified gaps
- **`schema`** emits only the `version`+`schema` subtrees (~8 KB) vs the full 19-command Go tree
  (~959 KB). Golden test asserts structure only, not byte parity.
- **Tempo 0x76 tx** signer recovers to the EOA but the on-wire RLP byte layout is **not** pinned
  against a `tempo-go` oracle.
- **OWS** backend is unit-tested with a **mocked** command runner; no end-to-end test against a
  real/emulated `ows`/`tempo` CLI.
- **Determinism seam:** `cli.rs` uses `Utc::now()` + a hashed counter for `request_id`/`timestamp`;
  golden tests normalize these. No injectable clock/id flag yet (fine for golden parity, but
  app-level live tests will want the provider base-URL/`--rpc-url` seams that already exist).

---

## 3. Definition of "100% migrated and functional"

Acceptance criteria for declaring the migration complete:

1. **Command parity:** all 66 real Go commands run end-to-end in the Rust binary. For each: output
   (after documented volatile-field normalization) and exit code match the Go oracle — verified by
   golden tests (offline/deterministic commands) or `wiremock`-backed app-level tests (live
   commands).
2. **`schema` parity:** the full command tree serializes byte-for-byte against the Go `schema.json`
   (incl. flag ordering, inherited-flag scope, hidden dropping, metadata: `mutation`, `auth`,
   `required`, `enum`, `format`, `input_modes`, request/response hints).
3. **Execution parity:** every `plan|submit|status` + `actions` path builds/persists/broadcasts
   actions correctly, with **signing byte-parity** — EVM EIP-1559 (✅ done), Tempo 0x76 (pinned vs
   `tempo-go`), and OWS (verified vs a real/emulated CLI).
4. **Invariants enforced & tested:** config precedence (flags>env>file>defaults), cache flow
   (fresh-hit skip / TTL re-fetch / stale-within-budget / metadata+execution bypass), multi-provider
   `--provider` requirement, key-gating, `--results-only`/`--select`, error-always-full-envelope to
   stderr, stable exit codes.
5. **Quality bar:** `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D
   warnings`, `cargo test --workspace`, `cargo test --workspace --release` all clean. No
   `unwrap`/`expect`/`panic` in non-test lib code.
6. **Cutover:** README/AGENTS/CLAUDE.md/CHANGELOG + Mintlify docs updated; `.goreleaser.yml` +
   `scripts/install.sh` + release/CI workflows build & ship the Rust binary; Go CI kept green until
   retirement; then the Go tree (`internal/`, `cmd/`, `go.mod`, `go.sum`) removed.

---

## 4. Gap workstreams (TDD, ordered by dependency)

Each workstream is RED→GREEN→VERIFY. Reuse the existing tested helpers/adapters — do **not**
rewrite domain logic. The shared enabler (WS0) unblocks everything.

### WS0 — CLI arg parser + dispatch skeleton + plumbing  ·  size L  ·  blocks all
- [ ] **Replace the hand-rolled parser with clap (derive).** Model the full command tree + every
  group's flags/enums/input-modes in `defi-app` (clap is already a workspace dep). Keep a clap-free
  seam if desired, but the schema tree (WS6) should be derivable from the same source of truth.
- [ ] **Thread plumbing into dispatch:** construct provider clients (with base-URL/`--rpc-url`
  seams), the `defi-cache::Store`, and the `defi-execution::Store`/signer/backends; pass `Settings`.
- [ ] **Cache routing:** route read commands through `runner::run_cached_command`; ensure metadata
  + execution commands bypass cache init (spec §2.5).
- [ ] **Tests:** parser unit tests for each group's flags (required/enum/conflict/`--input-json`
  precedence); a dispatch smoke test that every known command path resolves to a handler (not
  `unknown command`).
- **Acceptance:** no real Go command returns `unknown command`; each routes to a handler (which may
  still be a typed `Unsupported` where the Go CLI itself is, e.g. kamino positions).

### WS1 — Read commands: market-data (handlers ready)  ·  size S  ·  after WS0
Commands: `protocols top|categories|fees|revenue`, `stablecoins top|chains`, `dexes volume`,
`chains gas`. Handlers (`run_*`) already exist.
- [ ] RED: app-level `wiremock` tests (DefiLlama/RPC base-URL injected) asserting full envelope +
  `meta.providers[]` + exit code per command.
- [ ] GREEN: wire each to its `run_*` via `run_cached_command`; parse flags (`--category`, `--chain`,
  `--limit`, `--peg-type`, multi-chain `chains gas`).
- [ ] VERIFY: cache-state transitions appear in `meta.cache`; `chains gas` multi-chain returns an
  array and rejects `--rpc-url` with multiple chains.

### WS2 — Read commands: lending/yield/swap/bridge data  ·  size L  ·  after WS0
Commands: `lend markets|rates|positions`, `yield opportunities|positions|history`, `swap quote`,
`bridge quote|list|details`, `chains top|assets`, `wallet balance`.
- [ ] **Write the missing handlers** (`run_*` returning `Envelope`) that call the (already-tested)
  provider adapters and apply the existing helpers (sort/limit/dedupe/filter/normalize). For
  positions/balance, build envelope+cache around the existing `fetch_*` fns.
- [ ] RED: per-command `wiremock` app tests (inject provider base URLs / `--rpc-url`); cover
  multi-provider `--provider` requirement, key-gating (`chains assets`, `bridge list/details`,
  `swap quote --provider 1inch/uniswap`), `--min-tvl-usd`, exact-output routing.
- [ ] GREEN: implement handlers + flag parsing; route through cache where applicable.
- [ ] VERIFY: envelope/field-order/exit-code parity vs Go oracle (offline-capable cases as goldens);
  APY as percentage points; base+decimal amount consistency.

### WS3 — Execution: plan  ·  size L  ·  after WS0, WS2 (quote reuse)
Commands: `swap plan`, `bridge plan`, `lend {supply,withdraw,borrow,repay} plan`,
`yield {deposit,withdraw} plan`, `rewards {claim,compound} plan`, `approvals plan`, `transfer plan`.
- [ ] **Write `*_plan` handlers** that compose actions via the existing builders
  (`BuildSwapAction`/`BuildBridgeAction` capability path; internal planners for
  lend/yield/rewards/approvals/transfer), persist to the action `Store`, and render the action
  envelope. Wire `--wallet` (OWS-first) / `--from-address` (local) identity, `--input-json`/
  `--input-file`, `--rpc-url`, pre-sign guardrails (bounded approvals, bridge target validation).
- [ ] RED: app tests asserting action shape, step calldata (reuse `defi-evm` ABI goldens),
  identity-constraint errors, `--allow-max-approval`/`--unsafe-provider-tx` gating.
- [ ] GREEN/VERIFY: per provider (Aave/Morpho/Moonwell; Across/LiFi; Uniswap/1inch/TaikoSwap/Tempo).

### WS4 — Execution: submit + status  ·  size L  ·  after WS3
Commands: `... submit`, `... status` for all groups; `actions list|show|estimate`.
- [ ] **Write submit/status handlers**: load persisted action steps, sign via the selected backend
  (local / OWS `--wallet` + `DEFI_OWS_TOKEN` / `--signer tempo`), broadcast, and poll status
  (incl. bridge destination settlement for Across/LiFi). `actions list/show/estimate` over the Store
  (`estimate` returns EIP-1559 native gas for EVM, fee-token for Tempo).
- [ ] RED: tests with mocked RPC/OWS command-runner (the `defi-ows`/executor seams already exist)
  for broadcast, status transitions, settlement waits, estimate fields.
- [ ] **Tempo 0x76 byte-parity (WS4a):** build a `tempo-go` oracle, capture signed 0x76 bytes for a
  fixed key/calls/fee-token, add a byte-for-byte parity test, reconcile RLP layout.
- [ ] **OWS e2e (WS4b):** test against a real/emulated `ows`/`tempo` CLI to confirm the actual
  arg/JSON contract (unit tests currently mock the runner).

### WS5 — Determinism & full golden parity sweep  ·  size M  ·  after WS1–WS4
- [ ] Extend `golden_cli.rs` to cover the full deterministic offline surface; add `wiremock`-backed
  app tests for live commands. Diff every command against the Go oracle (normalized) and record any
  residual drift (float formatting, ordering, omitempty/None).

### WS6 — Full `schema` tree parity  ·  size M  ·  after WS0
- [ ] RED: assert the complete serialized schema equals `rust/tests/golden/schema.json`
  (envelope-normalized).
- [ ] GREEN: derive every command/flag node (ideally from the WS0 clap source of truth) with full
  metadata; match cobra `VisitAll` alphabetical flag order, inherited-flag scope, hidden dropping.
- [ ] VERIFY: byte-for-byte vs Go `schema` output.

### WS7 — Cutover (Go-only)  ·  size M  ·  after WS5, WS6
- [ ] `completion`/`help`: enable clap-generated completions + help; confirm acceptable parity.
- [ ] Docs: README, AGENTS.md/CLAUDE.md ("First 5 minutes" + folder structure → `rust/`),
  CHANGELOG (`Unreleased` → Changed: Rust reimplementation, no contract change), Mintlify
  build/install pages (+ `mint validate`/`broken-links`/`a11y`).
- [ ] Release: `.goreleaser.yml` → Rust build matrix (linux/darwin × amd64/arm64) keeping artifact
  name `defi`; update `scripts/install.sh` asset resolution; update `.github/workflows/release.yml`
  (keep `docs-live` sync).
- [ ] CI: add `rust-ci.yml` (fmt/clippy/test debug+release on ubuntu+macos); keep Go CI until
  retirement.
- [ ] Retire Go: once Rust CI is green and parity is signed off, remove `internal/`, `cmd/`,
  `go.mod`, `go.sum`, Go workflows.

---

## 5. Sequenced roadmap

```
WS0 (parser + dispatch + plumbing)        ← unblocks everything
 ├─ WS1 (market-data reads, handlers ready)   quick win, proves the pipeline
 ├─ WS2 (lend/yield/swap/bridge reads)
 │    └─ WS3 (execution: plan)
 │         └─ WS4 (execution: submit/status, + Tempo/OWS parity)
 ├─ WS6 (full schema tree)  ← can start after WS0 clap tree exists
 └─ WS5 (full golden/wiremock parity sweep)  ← after WS1–WS4
WS7 (cutover: docs/release/CI/retire Go)  ← last, after WS5 + WS6
```

Order of value: **WS0 → WS1** makes the CLI demonstrably functional for read data fast; **WS2** covers
the most-used queries; **WS3/WS4** complete execution (the hardest, with signing parity); **WS6**
restores `schema`; **WS5** is the parity gate; **WS7** ships it and retires Go.

---

## 6. 100% Definition-of-Done checklist

- [ ] All 66 real Go commands route to a handler (none return `unknown command`).
- [ ] Every command: contract output + exit code parity vs Go oracle (golden or wiremock), tested.
- [ ] `schema` full-tree byte parity.
- [ ] Execution plan/submit/status for all groups; signing byte-parity (EVM ✅, Tempo 0x76, OWS e2e).
- [ ] Invariants enforced & tested: config precedence, cache flow, multi-provider, key-gating,
  `--results-only`/`--select`, error→full-envelope-on-stderr, exit codes.
- [ ] `fmt`/`clippy -D warnings`/`test`/`test --release` all clean; no `unwrap`/`panic` in lib code.
- [ ] Docs (README/AGENTS/CLAUDE/CHANGELOG/Mintlify) updated.
- [ ] `.goreleaser` + `install.sh` + release/CI build & ship the Rust binary; Rust CI green.
- [ ] Go tree retired.

---

## 7. How to execute

Each workstream maps cleanly onto the same RED→GREEN→VERIFY workflow used for the initial port. WS0
must be done first (and is best done by a single focused agent, since the clap tree is the shared
source of truth). WS1–WS6 can fan out per command/group (disjoint files in `defi-app`, sequential
within the crate to avoid cargo races, as before). WS7 is a small sequential pass. Recommend running
WS0 + WS1 first as one workflow to get a demonstrably functional read-only CLI, review, then proceed.

---

## 8. Deferred to human sign-off

The WS7 cutover landed only the **safe, additive** half: a parallel `rust-ci.yml`
(fmt/clippy/test debug+release/build on ubuntu+macos), a CHANGELOG `Unreleased → Changed` note,
a README "Rust port (preview)" pointer, and this subsection. The Go tree, Go CI, release pipeline,
and canonical docs are **unchanged and still authoritative**.

The remaining cutover steps are **destructive or release-affecting** and must NOT run until a human
has signed off on full parity (WS5 + WS6 green, Tempo 0x76 + OWS byte-parity confirmed). Each step
below is exact and reversible-by-revert.

### 8.1 Swap the release build to Rust (`.goreleaser.yml`)
- [ ] Replace the GoReleaser `builds:` block with a Rust matrix (linux/darwin × amd64/arm64) that
  produces a single artifact still named `defi`. Options: drive `cargo build --release` per target
  via a `before.hooks` + prebuilt `builds[].builder: prebuilt` block, or migrate to
  `cargo-dist`/`cargo zigbuild` cross-compilation. Keep archive naming
  (`defi_<version>_<os>_<arch>.tar.gz`, Windows `.zip`) and `checksums.txt` identical so
  `scripts/install.sh` asset resolution keeps working.
- [ ] Update `scripts/install.sh` only if archive/asset names change (target triples vs goos/goarch);
  otherwise leave it untouched.
- [ ] Verify locally with `goreleaser release --snapshot --clean` (or the cargo-dist equivalent) that
  every target archive contains a runnable `defi` and checksums match.

### 8.2 Update `.github/workflows/release.yml`
- [ ] Point the tagged-release job at the Rust build path (toolchain + `rust/` working dir) while
  keeping: artifact name `defi`, the GitHub Releases upload, and the `docs-live` force-sync that runs
  **only** for stable (non-prerelease) tags.
- [ ] Keep `rust-ci.yml` as the PR/push gate; do not delete `ci.yml` (Go CI) in this step.

### 8.3 Retire the Go tree
- [ ] Only after the Rust release pipeline has cut at least one verified tag: remove `internal/`,
  `cmd/`, `go.mod`, `go.sum`, `.github/workflows/ci.yml`, and
  `.github/workflows/nightly-execution-smoke.yml` (or port the nightly smoke to Rust first).
- [ ] Remove the transient `go build -o defi` oracle references from agent docs.

### 8.4 Rewrite AGENTS.md / CLAUDE.md and Mintlify docs
- [ ] Rewrite "First 5 minutes" and the folder-structure block to describe `rust/` (16-crate
  workspace) instead of the Go layout; update build/test commands to `cargo` equivalents.
- [ ] Update README "Install / Build from source" + "Go install" sections to the Rust toolchain and
  remove the "preview" framing once Rust is the shipped binary.
- [ ] Update Mintlify build/install pages and re-run `npx --yes mint@4.2.378 validate`,
  `broken-links`, and `a11y` from `docs/`.

### 8.5 Sign-off gate (must all be true before 8.1–8.4)
- [ ] WS5 full golden/wiremock parity sweep green (no unexplained drift).
- [ ] WS6 `schema` full-tree byte parity green.
- [ ] Tempo 0x76 (WS4a) and OWS e2e (WS4b) byte/contract parity confirmed.
- [ ] `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --workspace`, `cargo test --workspace --release` all clean on `rust-ci.yml`.
- [ ] Human reviewer explicitly approves retiring Go.
