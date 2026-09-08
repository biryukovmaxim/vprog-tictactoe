# vprog-tictactoe

Tic-tac-toe with stakes as a [vprogs](../vprogs) guest program on Kaspa: two players lock a stake,
play a multi-round match, and the verifiable program settles the pot to the winner (or splits it
back on a draw). Reuses the vprogs framework and Kaspa stack for everything except the program's
own actions, accounts and game rules.

> **Status: guest crate, node daemon with DA server, scenario driver, and web frontend landed.**
> The workspace and web build and test clean (`just check`, `just test`, and `cd web && npm run
> test && npm run build`).
> The checklist below is the source of truth for what works.

## Architecture

- **Guest wire library (`guest/`)**: Single source of truth for action encoding/decoding, resource
  layouts, lock views, and deterministic resource-id derivations. Compiles for both host and zkVM.
- **Node daemon (`node/`, `ttd`)**: Environment-only runner driver with zero payload knowledge,
  plus the DA HTTP server. Loads guest ELFs, delegates deposit derivation via
  `delegate_entry_spk_hash`, drives execution or proving loops, and serves rollup state, games,
  accounts, and exit views.
- **Scenario driver (`driver/`, `ttflow`)**: Standalone issuer sample composing guest action encoders,
  `vprogs-zk-backend-risc0-app-kit` payload assembly, and L1 wallet carrier submission.
- **Web encoder (`encoder-wasm/`)**: Pure build/sign surface for the browser over the guest
  encoders and app-kit; wasm-pack output is vendored at `web/src/wasm`.

```
┌──────────────────────────────────────────────────────────────────┐
│ web/          frontend: board UI, simple privkey wallet,         │
│               submits action carrier txs to L1 (Kaspa) directly  │
├──────────────────────────────────────────────────────────────────┤
│ encoder-wasm/ pure build/sign surface for the web over guest     │
│ encoders + app-kit; vendored to web/src/wasm                     │
├──────────────────────────────────────────────────────────────────┤
│ driver/       `ttflow`: scripted scenario issuer; composes guest │
│               encoders, app-kit payload assembly, and wallet     │
├──────────────────────────────────────────────────────────────────┤
│ node/ (`ttd`) vprogs runner driver + DA server: env-only runner  │
│               daemon; zero payload knowledge                     │
├──────────────────────────────────────────────────────────────────┤
│ guest/        RISC0 guest program & shared wire library:         │
│               runtime/  lock/signer dispatchers, genesis key,    │
│                         ix wire framing                          │
│               program/  resources (config, user, game), actions  │
│                         (encoders & apply fns), deposit policy   │
├──────────────────────────────────────────────────────────────────┤
│ ../vprogs    framework dependencies: runtime battery, zk-abi,    │
│              runner, app-kit, l1-wallet, l1-bridge               │
└──────────────────────────────────────────────────────────────────┘
```

## The game

- A **match** is a fixed number of rounds, chosen by the creator when opening the game. X/O
  assignment alternates every round: the creator (seat 0) plays their chosen mark in even
  rounds, the joiner (seat 1) the other mark in odd ones. X opens every round.
- The match ends early on a clinch: once one seat's round wins exceed the opponent's wins plus
  the rounds left, the remaining rounds are skipped. Final outcome by seat: `First` or
  `Second`, or `Draw` when all rounds end even.
- **Stake**: each player locks one stake into the pot (`2 × stake`) at create/join. The winning
  seat takes the pot on settlement; a draw splits it back exactly. Funds are rollup account
  balances: entry (deposit) creates/funds accounts, exit (withdraw) returns to L1, transfer
  moves between accounts.
- **Turns**: a turn on the mover's own ply lands immediately; otherwise it queues as a
  pre-commit that auto-plays when the mover's turn arrives (pre-commits on since-taken cells
  are dropped). One transaction carries a **list of actions** applied in order, so live moves
  plus pre-commits pack into a single tx: a whole match can play out in about one tx per
  player.
- **Timeout**: when the to-move player lets `last_move_at + turn_ttl` (config) elapse on the
  DAA-score clock, anyone can claim the expiry: the round is forfeited to the opponent (board
  resets, early clinch still applies), and the claim restarts the clock for the next round.

## Status

- [x] Guest crate: structure + batteries wiring (runtime-processor lib via path dep)
- [x] Guest crate: `program/` accounts (config Init, deposit, transfer, withdraw)
- [x] Guest crate: `program/` game (create, join, turn with pre-commit queues; settlement and
      split with early clinch; per-creator sequential game ids from the user's
      `games_started` counter)
- [ ] Guest tests: rules engine, wire round-trips, dev-mode flow tests
- [ ] Guest ELF: Docker reproducible build, genesis env flow (local `just build-guest` works)
- [ ] vprogs feature: app-level custom journal data via closures (prerequisite for indexes)
- [x] Node DA: exit-record store with spent marks; open-game and player-event indexes
  - HTTP server: `/api/state`, `/api/config`, `/api/games`, `/api/accounts/:id`, `/api/exits`
  - Settled exit views with Merkle paths and spend state for DA queries
- [x] Node `ttd`: runner driver (execution and proving modes, devmode stub and GPU proving) with
      the DA server above
- [x] Driver `ttflow`: scripted scenarios
- [x] Env-gated L1 e2e against a local simnet (in-process rig; all scenario carriers accepted —
      the appended claim tail awaits a vprogs settlement fix, see `docs/demo/README.md`)
- [x] Web frontend: DA client, privkey wallet, match board, transfer/withdraw/claim actions,
      activity trust chips (`docs/demo/` has the runbook and current live limits)
- [ ] Env-gated L1 e2e against a local testnet-10 fork node

## Running

Requires the [vprogs](../vprogs) clone checked out next to this repo, a Rust nightly toolchain,
`just`, `taplo`, and for the frontend a Node toolchain. Commands activate as the corresponding
crates land:

```bash
just check        # clippy, warnings denied
just test         # workspace + guest tests
just build-guest  # compile guest ELF to guest/compiled/program.elf
```

Run the `ttd` node driver (default network `tn10`):

```bash
TT_WRPC_URL=ws://127.0.0.1:17210 \
TT_PRIVATE_KEY=<32-byte-hex-key> \
TT_BATCH_ELF=../vprogs/zk/backend/risc0/batch-processor/compiled/program.elf \
TT_AGGREGATOR_ELF=../vprogs/zk/backend/risc0/batch-aggregator/compiled/program.elf \
cargo run -p vprog-tictactoe-node
```

The daemon also serves the DA HTTP API on `TT_DA_BIND` (default `127.0.0.1:9880`); set
`TT_WEB_DIR` to serve a built web frontend from the same port. The full demo runbook, including
the simnet wiring and current live limits, lives in `docs/demo/`.

Run the `ttflow` scenario driver against a running node and lane:

```bash
TT_WRPC_URL=ws://127.0.0.1:17210 \
TT_LANE_ID=1 \
TT_COVENANT_ID=<32-byte-hex-covenant-id> \
TTFLOW_PRIVATE_KEY=<32-byte-hex-operator-funding-key> \
cargo run -p vprog-tictactoe-driver
```

Run the web frontend dev server (proxies `/api/*` to the `ttd` DA server on `127.0.0.1:9880`):

```bash
cd web && npm install && npm run dev    # also: npm run test, npm run build
```

The wallet layer talks to L1 via a locally built `kaspa-wasm` package vendored at
`web/src/kaspa-pkg` (npm only ships 0.13.x). `VITE_WRPC_URL` (default
`ws://127.0.0.1:17210`) and `VITE_NETWORK` (default `simnet`) select the node.

## Run modes

| Mode | Role | `TT_PROVE` | `RISC0_DEV_MODE` | Cargo features |
|---|---|---|---|---|
| **Execution** | Follower / DA / local exec | `0` (default) | `1` (default) | default |
| **Dev Prover** | Fast local proving (CPU stubs) | `1` | `1` | default |
| **Production Prover** | Verifiable STARK proving (CUDA) | `1` | `0` | `--features cuda` |

## Environment variables

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

## Documentation

Public docs live under `docs/`: `docs/demo/` is the web demo runbook (rig, env vars, funding,
current live limits). Internal specs and plans are not part of the repository.
