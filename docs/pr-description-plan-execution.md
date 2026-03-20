# PR Description: Retrieval to Plan + Execute

## Intent
This PR moves `defi-cli` from a retrieval-only tool to a tool that can execute on-chain actions.

The core design is an explicit two-step flow:
- first `plan`: build and persist an action without broadcasting
- then `submit`: execute a previously planned action

This lets agents and humans move from quote/data discovery to deterministic planning and then controlled execution, while keeping the same automation contract:
- stable JSON envelopes
- deterministic exit codes
- canonical chain/asset/amount handling

## What’s New
- Added execution lifecycle commands across DeFi domains: `plan`, `submit`, `status`.
- Added persisted action tracking with `actions list` and `actions show`.
- Added local signer support for execution (`env`, `file`, `keystore`, plus one-off `--private-key`).
- Added execution support for:
  - bridge: `across`, `lifi`
  - lend: `aave`, `morpho`, `moonwell`
  - rewards: `aave`
  - swap: `taikoswap` (same interface as Univ3)
  - approvals: native ERC-20 approvals
This is the initial execution-capable set; more providers will be added under the same command patterns.

## API Surface
### Execution command matrix
- `swap plan|submit|status`
- `bridge plan|submit|status` (provider: `across|lifi`)
- `approvals plan|submit|status`
- `lend supply|withdraw|borrow|repay plan|submit|status` (provider: `aave|morpho|moonwell`)
- `rewards claim|compound plan|submit|status` (provider: `aave`)
- `actions list|show`

### Required selectors
- Multi-provider/multi-domain flows require explicit `--provider` selection.
- No implicit defaults on these paths, to avoid accidental mistakes and prevent future provider additions from changing behavior.

### Key execution flags
- signer and keys: `--signer`, `--key-source`, `--private-key`
- execution controls: `--simulate`, `--step-timeout`, `--poll-interval`
- gas controls: `--gas-multiplier`, `--max-fee-gwei`, `--max-priority-fee-gwei`
- safety overrides: `--allow-max-approval`, `--unsafe-provider-tx`
- routing extras: `--rpc-url` (execution + on-chain quotes), `--from-amount-for-gas` (LiFi)

## Signing and Simulation
- Signing backend today is `local` only (`--signer local`).
- Signer wiring is abstracted, so additional signer backends can be added in future releases without changing the command model.
- `submit` executes a previously planned action by `--action-id`.
- `submit` supports an optional `--from-address` signer-address guard.
- `--simulate` is enabled by default and runs preflight transaction simulation before broadcast; disabling it is opt-in (`--simulate=false`).
- `--step-timeout` and `--poll-interval` control confirmation waiting behavior during execution.

### Local signer precedence
When `--private-key` is not passed, auto key discovery is:
1. `DEFI_PRIVATE_KEY`
2. `DEFI_PRIVATE_KEY_FILE`
3. `${XDG_CONFIG_HOME:-~/.config}/defi/key.hex` (if present)
4. `DEFI_KEYSTORE_PATH` + password env/file

## How to Use
### Story 1: Plan now, execute later (Bridge)
```bash
# Step 1: create and persist a bridge plan
defi bridge plan --provider across --from 1 --to base --asset USDC --amount 1000000 --from-address 0xYourEOA --results-only

# Step 2: later, execute the saved plan
defi bridge submit --action-id <action_id> --results-only

# Step 3: check lifecycle state
defi bridge status --action-id <action_id> --results-only
```

### Story 2: Plan and submit a lending action
```bash
# Aave supply plan (chain name)
defi lend supply plan --provider aave --chain base --asset USDC --amount 1000000 --from-address 0xYourEOA --results-only

# Morpho supply later submit (numeric chain id + explicit market-id)
defi lend supply submit --action-id <action_id> --results-only
```

### Story 3: Plan and submit rewards in separate steps
```bash
# Plan first (captures sender intent)
defi rewards claim plan --provider aave --chain base --from-address 0xYourEOA --assets 0x... --reward-token 0x... --results-only

# Submit later by action id
defi rewards claim submit --action-id <action_id> --results-only
```

### Inspect actions
```bash
defi actions list --results-only
defi actions show --action-id <action_id> --results-only
```

