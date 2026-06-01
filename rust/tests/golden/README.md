# Golden fixtures (Phase 0 oracle)

These fixtures are the **primary success oracle** for the Go -> Rust migration. They were
captured from the pre-retirement Go reference binary (version `0.5.0`) for deterministic,
**offline** commands. The Rust CLI must reproduce them **byte-for-byte** after the volatile-field
normalization described below.

> The Go reference tree has been retired. Re-capture these fixtures only from a tagged historical
> checkout or from another explicitly approved oracle build, then re-run the commands in the
> "Commands" table.

## File layout

For each command `<slug>`:

- `rust/tests/golden/<slug>.json` — captured stdout (success) or stderr (error envelope).
- `rust/tests/golden/<slug>.exit` — process exit code (single integer + newline).

## Commands

| slug | command | stream | exit | shape |
|---|---|---|---|---|
| `version` | `defi version` | stdout | 0 | raw string (`0.5.0\n`), NOT an envelope |
| `version-long` | `defi version --long` | stdout | 0 | raw string, NOT an envelope |
| `schema` | `defi schema` | stdout | 0 | full envelope (`data` = schema object) |
| `providers-list` | `defi providers list --results-only` | stdout | 0 | `data` array only (no envelope) |
| `chains-list` | `defi chains list` | stdout | 0 | full envelope (`data` = chain array) |
| `chains-list-results-only` | `defi chains list --results-only` | stdout | 0 | `data` array only (no envelope) |
| `assets-resolve-usdc` | `defi assets resolve --symbol USDC --chain 1` | stdout | 0 | full envelope (`data` = resolution object) |
| `assets-resolve-usdc-results-only` | `defi assets resolve --symbol USDC --chain 1 --results-only` | stdout | 0 | `data` object only (no envelope) |
| `error-usage-missing-asset` | `defi assets resolve --chain 1` | **stderr** | 2 | full envelope (`success=false`, `error` set, `data=[]`) |
| `error-usage-missing-asset-results-only` | `defi assets resolve --chain 1 --results-only` | **stderr** | 2 | full envelope — proves `--results-only` is **ignored on error** |
| `error-usage-bad-chain` | `defi assets resolve --symbol USDC --chain notarealchain` | **stderr** | 2 | full envelope (`success=false`, usage_error) |

## Stream contract (must preserve)

- **Success** output goes to **stdout**.
- **Error** envelopes go to **stderr** (and exit non-zero), and are always the **full envelope**
  even under `--results-only`/`--select`. The two `error-usage-missing-asset*` fixtures are byte
  identical (modulo volatile fields), which encodes this invariant.

## Volatile-field normalization

Before comparing a captured-vs-produced **JSON envelope** fixture, blank the following JSON paths
to a fixed sentinel on BOTH sides (the Go capture and the Rust output), then compare. Apply the
identical normalization in the Rust golden tests.

Normalizable JSON paths (only present in full-envelope fixtures — i.e. everything except the
`*-results-only`, `providers-list`, and `version*` fixtures):

```
meta.request_id     # random 128-bit hex per run        -> "<request_id>"
meta.timestamp      # RFC3339 wall-clock time per run    -> "<timestamp>"
meta.cache.age_ms   # cache age; 0 for bypass cmds, but  -> 0
                    #   normalize to be robust for cache-backed commands
```

Additional paths that are volatile in the **general** contract and MUST be normalized by any
golden test that captures live/cache-backed commands later (none of the current Phase-0 fixtures
contain them, but list them so the Rust normalizer is complete):

```
meta.providers[].latency_ms     # per-provider request latency        -> 0
*.fetched_at  /  *.*fetched_at* # any field literally named fetched_at -> "<fetched_at>"
                                #   (LendMarket/LendRate/SwapQuote/BridgeQuote/etc.)
```

### Normalization rules (precise)

1. Parse the fixture as JSON. If parsing fails, treat it as a **raw-string** fixture
   (`version`, `version-long`) — compare verbatim, no normalization. NOTE: the embedded version
   number is release-dependent; Rust golden tests for `version*` should compare against the
   Rust crate version, not the literal Go `0.5.0` bytes (these fixtures document shape/format:
   `"<version>\n"` and `"<version> (commit: <c>, built: <b>)\n"`).
2. If parsed JSON is an **object** containing a `meta` key (full envelope), set:
   `meta.request_id = "<request_id>"`, `meta.timestamp = "<timestamp>"`,
   `meta.cache.age_ms = 0`, and (if present) every `meta.providers[i].latency_ms = 0`.
3. Recursively, for any object key named exactly `fetched_at` (or matching `*fetched_at*`),
   set its value to `"<fetched_at>"`.
4. Compare the normalized JSON with **2-space indent and struct/declaration field order
   preserved** (do NOT sort keys for JSON comparison — declaration order is part of the
   contract). For value equality you may compare parsed structures; for byte-stable rendering
   tests, re-serialize with the same 2-space indent + preserve_order settings the CLI uses.
5. `.exit` fixtures: compare the integer exit code exactly.

### Why these and only these are volatile

Across repeated runs of the captured commands, only `meta.request_id` and `meta.timestamp`
changed. `meta.cache.age_ms` was `0` for all (these commands bypass the cache:
`cache.status == "bypass"`), but it is listed as normalizable so the same normalizer works for
cache-backed commands added later. `meta.providers[].latency_ms` and `*fetched_at*` do not appear
in any Phase-0 fixture (all are offline metadata commands) but are part of the general volatile
set and are included for completeness.
