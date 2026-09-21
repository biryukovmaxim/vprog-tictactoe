# Demo reference

Lookup tables for the demo runbook ([README.md](README.md)) and the repo's `run-demo` agent
skill: run modes, environment variables, ports, HTTP endpoints, and failure signatures. The
runbook explains the flow; this file is the knob-by-knob reference.

## Run modes (`ttd`)

| Mode | Role | `TT_PROVE` | `RISC0_DEV_MODE` | Cargo features |
|---|---|---|---|---|
| **Execution** | Follower / DA / local exec | `0` (default) | `1` (default) | default |
| **Dev Prover** | Fast local proving (CPU stubs) | `1` | `1` | default |
| **Production Prover** | Verifiable STARK proving (CUDA) | `1` | `0` | `--features cuda` |

## Node (`ttd`)

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

## Driver (`ttflow`)

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
| `TTFLOW_TRANSFER_AMOUNT` | In-rollup transfer from player A to B mid-scenario, in sompis (0 skips the step) | `0` |
| `TTFLOW_PLAYER_A_KEY` | Fixed player-A key (32-byte hex) so the claim example can reuse the run's exit-leaf owner | random |
| `TTFLOW_STEP_DELAY_MS` | Delay between scenario steps in milliseconds | `2000` |
| `TTFLOW_TURN_TTL` | Turn TTL in DAA-score units | `10000` |

## Demo L1 and web

| Variable | Description | Default |
|---|---|---|
| `TT_DEMO_L1_INTERVAL_MS` | Demo L1 mining interval | `1000` |
| `VITE_WRPC_URL` | L1 wRPC URL for the in-page wallet | `ws://127.0.0.1:17210` |
| `VITE_NETWORK` | Network for the in-page wallet (`simnet`, `testnet-10`, ...) | `simnet` |

## Ports and endpoints

| Process | Address | Serves |
|---|---|---|
| demo L1 | `ws://127.0.0.1:17210` | wRPC (borsh) for `ttd`, `ttflow`, and the web wallet |
| demo L1 | `http://127.0.0.1:9890` | `GET /faucet/{address}` (pays 10 KAS; 503 for the first ~10 blocks until the coinbase matures), `POST /inject` (body is raw borsh tx bytes, mined into the next block; claims no longer use it — they pay fees through the mempool) |
| `ttd` DA | `http://127.0.0.1:9880` | DA HTTP API below, plus optional static `TT_WEB_DIR` |

DA API (all `GET`, JSON):

| Endpoint | Returns |
|---|---|
| `/api/state` | lane, covenant id, settlement status |
| `/api/config` | `{"initialized": bool}` |
| `/api/games?status=open\|playing\|finished&after={id}&limit=N` | `{"games": [...]}`, each with `state_name` (`Open`, `Playing`, ...); status filter + cursor pagination |
| `/api/games/{id}` | one game |
| `/api/accounts/{user-id-hex}` | one account |
| `/api/exits` | settled exit leaves (claimable) |

## Failure signatures and fixes

| Symptom | Cause | Fix |
|---|---|---|
| Faucet returns 503 | coinbase not mature yet (first ~10 blocks) | wait a few seconds, retry |
| `ttd` prints an issuer address, then exits with "no spendable UTXO for bootstrap" | issuer key unfunded | `GET /faucet/<issuer-address>` **twice**, then restart `ttd` |
| `ttd` later backs off with "no spendable fee UTXO" | issuer underfunded (single payout) | faucet the issuer again |
| `ttflow` fails at Init with "transaction submission failed after retries" | operator key unfunded | faucet its logged operator address **twice**, rerun |
| `ttd` restart exits with "starting block no longer in chain" | data dir holds anchors from a previous chain | `rm -rf ttd-data` (or the `TT_DATA_DIR`), restart demo L1 and `ttd` fresh together |
| CUDA link fails with `undefined symbol: ngpus()/select_gpu(int)` | sppark's build script cached a no-nvcc result | delete `target-cuda/release/build/sppark-*`, `target-cuda/release/deps/libsppark-*`, `target-cuda/release/.fingerprint/sppark-*`, rebuild with `/usr/local/cuda-12.2/bin` on `PATH` |
| Web console shows a rejected `submitTx` when claiming | a real error: claims are ordinary fee-paying mempool txs (delegate pool below the leaf, or the wallet holds no UTXO covering the estimated fee) | check the delegate pool covers the leaf and the wallet holds fee collateral; faucet the wallet |
| `17210`/`9890`/`9880` already in use | stale demo L1 / `ttd` from a previous run | kill them, wipe the data dir, restart fresh |
| `ttflow` Init rejected | covenant already initialized (Init is genesis-gated) | check `GET /api/config` for `{"initialized": true}`; skip init |
| `ttd` sits silent for a long stretch after compiling | debug builds sync very slowly | run with `--release` |
