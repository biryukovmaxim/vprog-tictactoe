# Demo runbook

A runnable demo of the tic-tac-toe rollup: an in-process simnet L1, the `ttd` node driver with
the DA server, a one-shot `ttflow` covenant init, and the browser UI (privkey wallet, open games,
live match board, transfer/withdraw, exit claims). `ttd` runs in two proving modes:

- **devmode**: regular CPU build with `RISC0_DEV_MODE=1` — fake (stub) receipts, fast bundles.
- **cuda**: `--features cuda` build with `RISC0_DEV_MODE` unset — real proofs on the GPU; each
  bundle takes visibly longer than a dev stub.

Both modes run the identical flow below; only step 2 differs.

## Prerequisites

- A [vprogs](../../vprogs) clone next to this repo, with backend ELFs built
  (`zk/backend/risc0/batch-processor/compiled/program.elf` and
  `zk/backend/risc0/batch-aggregator/compiled/program.elf`)
- Rust nightly, `just`, `taplo`; Node/npm for the frontend
- Guest ELF: `just build-guest`
- CUDA mode only: CUDA 12.2 at `/usr/local/cuda-12.2` (nvcc is not on the default PATH — see
  step 2)

## Running

Start steps 1 and 2 fresh together: the vprogs bridge cannot join an already-live lane, and the
demo L1's throwaway appdir guarantees a fresh chain on every run. When starting over, also wipe
`ttd`'s state directory (`rm -rf ttd-data`) — it persists the previous chain's anchors and the
bridge dies on restart with "starting block no longer in chain".

### 1. Demo L1

```bash
cargo run -p vprog-tictactoe-driver --example demo-l1
```

Runs an in-process simnet kaspad on fixed ports and mines a block per second (override with
`TT_DEMO_L1_INTERVAL_MS`, default 1000). Every run starts from a fresh chain (throwaway appdir),
so there is nothing to reset between demos. It serves:

- wRPC (borsh) at `ws://127.0.0.1:17210` — what `ttd` and the web point at
- `GET http://127.0.0.1:9890/faucet/{address}` — pays 10 KAS to the address from the miner's
  coinbase wallet. Returns 503 for the first ~10 blocks until the coinbase matures; wait a few
  seconds and retry.
- `POST http://127.0.0.1:9890/inject` — the request body is raw borsh-serialized transaction
  bytes, mined directly into the next block (see the zero-fee note in step 5)

### 2. Run `ttd` (prover + DA server)

Devmode (CPU, fake receipts):

```bash
TT_WRPC_URL=ws://127.0.0.1:17210 \
TT_NETWORK=simnet \
TT_PROVE=1 \
RISC0_DEV_MODE=1 \
TT_PRIVATE_KEY=<32-byte-hex-operator-key> \
TT_BATCH_ELF=../vprogs/zk/backend/risc0/batch-processor/compiled/program.elf \
TT_AGGREGATOR_ELF=../vprogs/zk/backend/risc0/batch-aggregator/compiled/program.elf \
TT_DA_BIND=127.0.0.1:9880 \
cargo run -p vprog-tictactoe-node
```

CUDA (real proofs; build once, then keep `/usr/local/cuda-12.2/bin` on `PATH` at runtime too —
the prover needs the CUDA tools, and `RISC0_DEV_MODE` must stay unset):

```bash
export PATH=/usr/local/cuda-12.2/bin:$PATH
CARGO_TARGET_DIR=target-cuda cargo build --release --features cuda -p vprog-tictactoe-node

TT_WRPC_URL=ws://127.0.0.1:17210 \
TT_NETWORK=simnet \
TT_PROVE=1 \
TT_PRIVATE_KEY=<32-byte-hex-operator-key> \
TT_BATCH_ELF=../vprogs/zk/backend/risc0/batch-processor/compiled/program.elf \
TT_AGGREGATOR_ELF=../vprogs/zk/backend/risc0/batch-aggregator/compiled/program.elf \
TT_DA_BIND=127.0.0.1:9880 \
target-cuda/release/ttd
```

- `TT_PROVE=1` enables the prover and settler workers; without them nothing settles and no exits
  appear. `TT_DA_BIND` (default `127.0.0.1:9880`) is the DA HTTP server; `TT_WEB_DIR` optionally
  serves a built frontend (`cd web && npm run build`, then `web/dist`) from the same port.
- **Fund the `TT_PRIVATE_KEY` issuer before the covenant can bootstrap.** An unfunded first
  start prints the issuer address (`settlement mode ... issuer kaspasim:...`) and exits with
  "no spendable UTXO for bootstrap" — open the faucet URL for that address **twice**, then start
  `ttd` again. Two payouts matter: the covenant bootstrap consumes one output entirely, and the
  settlement worker pays every settlement fee from the remaining ones (an underfunded issuer
  shows up later as "no spendable fee UTXO" backoffs).
- The startup log prints the lane id; once serving, `GET /api/state` reports the covenant id.
- CUDA build trap: if the link fails with `undefined symbol: ngpus()/select_gpu(int)`, sppark's
  build script cached a no-nvcc result — delete
  `target-cuda/release/build/sppark-*`, `target-cuda/release/deps/libsppark-*`, and
  `target-cuda/release/.fingerprint/sppark-*`, then rebuild with nvcc on `PATH`.

### 3. Initialize the covenant (`ttflow`, once)

The web UI cannot send the genesis-gated config `Init`. Run `ttflow` once to initialize the
covenant (it then plays a scripted match; stop it after `Init` lands or let it play out). The
operator key must be funded first: `ttflow` logs
`operator address (fund this before running): kaspasim:...` on startup and fails at Init with
"transaction submission failed after retries" while unfunded — open the faucet URL for that
address twice, then rerun.

```bash
TT_WRPC_URL=ws://127.0.0.1:17210 \
TT_NETWORK=simnet \
TT_LANE_ID=<lane id from the ttd log> \
TT_COVENANT_ID=<covenant id from /api/state> \
TTFLOW_PRIVATE_KEY=<32-byte-hex-operator-key> \
cargo run -p vprog-tictactoe-driver
```

### 4. Web frontend

```bash
cd web
npm install
npm run dev
```

The dev server proxies `/api/*` to the DA server on `127.0.0.1:9880`. `VITE_DEMO_L1` sets the
demo L1 base URL (default `http://127.0.0.1:9890`); `VITE_WRPC_URL` (default
`ws://127.0.0.1:17210`) and `VITE_NETWORK` (default `simnet`) select the L1 node for the
in-page wallet.

### 5. Fund, play, withdraw, claim

1. Open the dev URL, paste a 32-byte hex private key, press Load. The key bar shows the derived
   address; the key lives in memory only and is cleared on reload.
2. Fund it from the faucet: open `http://127.0.0.1:9890/faucet/<displayed-address>` (pays 10
   KAS; 503 until the coinbase matures, see step 1). Carrier fees and deposits spend from this
   L1 balance.
3. Create or join a game (the confirmation line shows the covenant deposit address for the
   deposit carrier), play turns on the board, and transfer or withdraw from the actions column.
4. Once a settlement lands (`GET /api/state` reports it, `GET /api/exits` lists the settled
   leaves), claim buttons appear. Claiming pays the leaf out to the wallet; the tx is mined via
   the demo L1's `/inject` (below).

**Claims are zero-fee by protocol.** Attaching a fee burns value inside the covenant spend and
fails the script engine, and the relay floor rejects zero-fee txs from the mempool — so a claim
can never enter the mempool. The web app first tries the wallet `submitTx`, and when that is
rejected it automatically POSTs the built claim to the demo L1's `/inject`, which mines it
directly into the next block. **A rejected `submitTx` in the browser console is this expected
fallback, not a bug.** This requires the demo L1 to be reachable at `VITE_DEMO_L1`.

## Testnet-10 instead of the demo L1

The stack also runs against a real testnet-10 wRPC node. Replace the demo-L1 step and repoint
everything at the node:

- `ttd`: `TT_WRPC_URL=<tn10 wRPC url>` (`TT_NETWORK` already defaults to `tn10`), CUDA build
  with `RISC0_DEV_MODE` unset — on a shared chain, settlements must carry real proofs.
- Web: `VITE_WRPC_URL=<tn10 wRPC url>`, `VITE_NETWORK=testnet-10`, and no `VITE_DEMO_L1`.
- Fund keys from a tn10 faucet instead of the demo faucet; keep the one-time `ttflow` init.

Two gaps, both known: exit claims cannot be mined on tn10 (zero-fee by protocol, rejected by
the relay floor; the demo L1's `/inject` route does not exist on a real node), and the bridge
cannot join an already-live lane — start `ttd` on a fresh lane. This path is configured but
not yet exercised end to end.

## Automated e2e

The whole flow headlessly — in-process simnet, init, deposits, a full match, settlement,
withdraw, exit records, claim, and payout (dev stub proofs):

```bash
TT_E2E=1 RISC0_DEV_MODE=1 cargo test --release -p vprog-tictactoe-driver --test e2e_simnet -- --nocapture
```

## Current limits

- **One claim per exit root**: once a root's first claim spends its settlement outpoint, the
  remaining leaves are not claimable — sequential claims need the continuation root (post-merge
  work).
- **Claims are zero-fee and direct-mined** via `/inject`; they never go through the mempool (see
  step 5).
- **The bridge cannot join an already-live lane** (it mis-anchors without authoritative tip
  seeding, still to land in vprogs): start the demo L1 and `ttd` fresh together — the demo L1's
  fresh-chain design guarantees this.
- **Busy-DAG divergence remains possible**: on mixed lanes / orphan mergesets the vprogs bridge
  can still diverge on merge_idx conventions. The demo L1 is quiet and linear, so it should not
  appear; if a settlement is rejected with a seq-commit script failure, that is where to look
  next.

## Reference

### Run modes (`ttd`)

| Mode | Role | `TT_PROVE` | `RISC0_DEV_MODE` | Cargo features |
|---|---|---|---|---|
| **Execution** | Follower / DA / local exec | `0` (default) | `1` (default) | default |
| **Dev Prover** | Fast local proving (CPU stubs) | `1` | `1` | default |
| **Production Prover** | Verifiable STARK proving (CUDA) | `1` | `0` | `--features cuda` |

### Node (`ttd`)

| Variable | Description | Default |
|---|---|---|
| `TT_WRPC_URL` | WebSocket RPC URL of the Kaspa node | *(required)* |
| `TT_PRIVATE_KEY` | 32-byte hex secret key for fees and node actions | *(required)* |
| `TT_NETWORK` | Network identifier (`tn10`, `simnet`, `devnet`, `mainnet`) | `tn10` |
| `TT_PROGRAM_ELF` | Path to compiled guest program ELF | `guest/compiled/program.elf` |
| `TT_BATCH_ELF` | Path to compiled batch-processor ELF | *(required)* |
| `TT_AGGREGATOR_ELF` | Path to compiled batch-aggregator ELF | *(required)* |
| `TT_DATA_DIR` | Directory for persistent state and RocksDB | `./ttd-data` |
| `TT_LANE_ID` | Execution lane identifier | generated if omitted |
| `TT_COVENANT_ID` | 32-byte hex covenant ID for catchup mode | none |
| `TT_BOOTSTRAP_TXID` | 32-byte hex anchor transaction ID | none |
| `TT_START_FROM` | 32-byte hex starting block hash | none |
| `TT_SEED_DEPTH` | Depth below sink for bridge catchup scan | `500` |
| `TT_PROVE` | Enable prover and settler worker | `0` |
| `TT_START_MODE` | Start mode (`fresh`, `resume`, `catchup`) | auto |
| `TT_DA_BIND` | Bind address for the DA HTTP server | `127.0.0.1:9880` |
| `TT_WEB_DIR` | Optional static web directory served by the DA server | none |

### Driver (`ttflow`)

| Variable | Description | Default |
|---|---|---|
| `TT_WRPC_URL` | WebSocket RPC URL of the Kaspa node | *(required)* |
| `TT_LANE_ID` | Target execution lane identifier | *(required)* |
| `TT_COVENANT_ID` | 32-byte hex covenant ID | *(required)* |
| `TTFLOW_PRIVATE_KEY` | 32-byte hex operator funding key | *(required)* |
| `TT_NETWORK` | Network identifier | `tn10` |
| `TTFLOW_GENESIS_KEY` | 32-byte hex key for config `Init` auth | dev genesis scalar 3 |
| `TTFLOW_STAKE` | Stake per player in sompis | `50000000` (0.5 KAS) |
| `TTFLOW_ROUNDS` | Rounds per match | `1` |
| `TTFLOW_DEPOSIT_AMOUNT` | Deposit amount per player in sompis | `100000000` (1.0 KAS) |
| `TTFLOW_STEP_DELAY_MS` | Delay between scenario steps in milliseconds | `2000` |
| `TTFLOW_TURN_TTL` | Turn TTL in DAA-score units | `10000` |

### Demo L1 and web

| Variable | Description | Default |
|---|---|---|
| `TT_DEMO_L1_INTERVAL_MS` | Demo L1 mining interval | `1000` |
| `VITE_DEMO_L1` | Demo L1 base URL (faucet + `/inject`) | `http://127.0.0.1:9890` |
| `VITE_WRPC_URL` | L1 wRPC URL for the in-page wallet | `ws://127.0.0.1:17210` |
| `VITE_NETWORK` | Network for the in-page wallet (`simnet`, `testnet-10`, ...) | `simnet` |
