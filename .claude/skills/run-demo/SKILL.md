---
name: run-demo
description: Use when starting, stopping, or resetting the tic-tac-toe demo stack (demo L1 simnet, ttd node as execution follower, dev CPU prover, or CUDA prover, ttflow init, web UI), also against testnet-10; when funding keys from the faucet, playing or scripting a match, or inspecting DA data (/api/state, /api/games, /api/exits); or when startup fails with "no spendable UTXO for bootstrap", "starting block no longer in chain", or faucet 503.
---

# Run the demo stack

Operational procedure for the tic-tac-toe rollup demo: bring the stack up in any mode, drive
a match, and read the data. Narrative flow and funding walkthrough: `docs/demo/README.md`.
Every environment variable, port, endpoint, and failure signature: `docs/demo/reference.md`.

## Components

| Piece | Start | Provides |
|---|---|---|
| demo L1 | `cargo run -p vprog-tictactoe-driver --example demo-l1` | in-process simnet: wRPC `ws://127.0.0.1:17210`, faucet + `/inject` on `:9890`; fresh chain every run |
| `ttd` | `cargo run -p vprog-tictactoe-node` | node + prover + settler + DA on `:9880` |
| `ttflow` | `cargo run -p vprog-tictactoe-driver` | one-time genesis-gated covenant `Init`, then one scripted match |
| web | `cd web && npm run dev` | browser UI (optional; agents do not need it) |

## Pick a mode

| Want | `TT_PROVE` | `RISC0_DEV_MODE` | Build | L1 |
|---|---|---|---|---|
| Follow / serve data only | `0` | `1` | default | demo L1 |
| Fast local proving (default) | `1` | `1` | default | demo L1 |
| Real proofs (CUDA) | `1` | unset | `--features cuda`, `CARGO_TARGET_DIR=target-cuda`, `/usr/local/cuda-12.2/bin` on `PATH` | demo L1 |
| testnet-10 | `1` | unset | cuda | `TT_WRPC_URL=<tn10>`; keys funded from a tn10 faucet; claims cannot mine there (known gap) |

Full env blocks per mode: runbook steps 2-3. All variables and defaults: `docs/demo/reference.md`.

## Startup order (simnet)

1. Kill stale processes and wipe node state: nothing may listen on `17210`/`9890`/`9880`
   (a previous run's demo L1 or `ttd`), then `rm -rf ttd-data` (or the `TT_DATA_DIR` in
   use). The bridge cannot join an already-live lane, and stale anchors kill restarts with
   "starting block no longer in chain".
2. Start the demo L1 and `ttd` fresh together, as background processes with log files
   (`run_in_background`, redirect output). Keys are any 32-byte hex: `openssl rand -hex 32`;
   never commit them. Reusing one key for `TT_PRIVATE_KEY` and `TTFLOW_PRIVATE_KEY` saves a
   funding round. Debug builds can sit silent for a long time after compiling: prefer
   `--release` when in doubt.
3. First `ttd` start with an unfunded key prints the issuer address and exits with "no
   spendable UTXO for bootstrap": expected. `curl http://127.0.0.1:9890/faucet/<issuer-address>`
   **twice** (503 for the first ~10 blocks is coinbase maturity: wait and retry), then
   restart `ttd`. Two payouts matter: bootstrap consumes one output entirely, settlements
   pay fees from the rest.
4. Readiness: `curl -s http://127.0.0.1:9880/api/state` returns JSON with the covenant id.
   The lane id is the `== ttd node: lane=... ==` line in `ttd`'s log.
5. Run `ttflow` once with `TT_LANE_ID`, `TT_COVENANT_ID` (from step 4), and a funded
   `TTFLOW_PRIVATE_KEY`. Skip it if `curl -s 127.0.0.1:9880/api/config` already answers
   `{"initialized":true}`: Init is genesis-gated, a rerun is rejected. It logs the operator
   address on startup; if Init fails with "transaction submission failed after retries",
   faucet that address twice and rerun.

## Play and inspect (no browser)

- Scripted match (init, deposits, create, join, turns, withdraw): run `ttflow` again any
  time; knobs `TTFLOW_STAKE`, `TTFLOW_ROUNDS`, `TTFLOW_DEPOSIT_AMOUNT`, `TTFLOW_STEP_DELAY_MS`.
  The scripted game is created and immediately joined, so `?status=open` is empty on a
  fresh stack: to see an open game, interrupt `ttflow` in the step window after CreateGame
  (default 2 s per step), or create one from the web UI.
- Whole flow headlessly, settlement and claim included:
  `TT_E2E=1 RISC0_DEV_MODE=1 cargo test --release -p vprog-tictactoe-driver --test e2e_simnet -- --nocapture`
- Data, DA on `:9880` (all `GET`, JSON): `/api/state`, `/api/config`,
  `/api/games?status=open&after={id}&limit=N`, `/api/games/{id}`,
  `/api/accounts/{user-id-hex}`, `/api/exits` (settled leaves, claimable).
- Claims burn a fee (2M sompi) from the delegate pool and submit through the ordinary
  `submitTx`/mempool path on any node; a rejected `submitTx` is a real error (short delegate
  pool, or a node fee policy above the script's 10M burn cap).

## Reset / teardown

Kill `ttd` before the demo L1. Wipe `ttd-data` whenever the demo L1 restarts (its chain is
new each run). Full restart: kill both, wipe, redo the sequence above.

## Something broke

Symptom, cause, and fix table: `docs/demo/reference.md`, section "Failure signatures and
fixes". Check it before improvising.
