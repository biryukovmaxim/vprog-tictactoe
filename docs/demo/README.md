# Web demo runbook

A single-page browser UI over the tic-tac-toe rollup: privkey wallet, open games, live match
board, transfer/withdraw actions, and exit claims. The page talks to two servers: the `ttd` DA
HTTP server (rollup state) and the L1 node over wRPC (wallet, carrier submission).

## Current limits (read first)

**Live settlement is currently rejected, so the claim loop cannot run live.** Runner-driven dev
settlements against a real kaspad fail: the vprogs bridge derives the aggregator's
`new_seq_commit` under a per-lane convention that diverges from the node's mergeset-global
convention, so the derived value does not equal the block header root and the node rejects the
settlement transaction's script. Until that is fixed in vprogs:

- `/api/state` never reports a settlement (`settled` stays absent)
- `/api/exits` stays empty: no settlement, no permission tree, nothing to claim
- the web claim flow cannot run against a live node
- the e2e claim tail times out at its first store wait (the tail itself is verified red-first
  and awaits the same fix)

What does work live: `ttd` boot, every DA endpoint, web build and serve, deposits, create, join,
turns (all eight scenario carriers were accepted on L1), and transfer/withdraw submission. The
unit gates all pass: `just check`, `just test`, `cd web && npm run test && npm run build`.

## Running

### 1. Prerequisites

- A [vprogs](../../vprogs) clone next to this repo, with backend ELFs built
  (`zk/backend/risc0/batch-processor/compiled/program.elf` and
  `zk/backend/risc0/batch-aggregator/compiled/program.elf`)
- Rust nightly, `just`, `taplo`; Node/npm for the frontend
- Guest ELF: `just build-guest`

### 2. L1 node

Any Kaspa node with the covenant forks active, reachable over wRPC (borsh). The default endpoint
is `ws://127.0.0.1:17210`; point both `TT_WRPC_URL` (`ttd`) and `VITE_WRPC_URL` (web) at it, and
set `TT_NETWORK` / `VITE_NETWORK` to match its network (`ttd` defaults to `tn10`, the web to
`simnet`).

The e2e test spins its simnet rig in-process (`driver/tests/e2e_simnet.rs`); a live demo needs a
standalone node.

### 3. Run `ttd`

```bash
TT_WRPC_URL=ws://127.0.0.1:17210 \
TT_NETWORK=simnet \
TT_PRIVATE_KEY=<32-byte-hex-key> \
TT_BATCH_ELF=../vprogs/zk/backend/risc0/batch-processor/compiled/program.elf \
TT_AGGREGATOR_ELF=../vprogs/zk/backend/risc0/batch-aggregator/compiled/program.elf \
TT_DA_BIND=127.0.0.1:9880 \
cargo run -p vprog-tictactoe-node
```

- `TT_DA_BIND` (default `127.0.0.1:9880`): DA HTTP server bind address
- `TT_WEB_DIR`: optional directory of built frontend assets (`cd web && npm run build`, then
  `web/dist`) served from the same port
- The startup log prints the lane id; once serving, `GET /api/state` reports the covenant id

### 4. Bootstrap the config

The web UI cannot send the genesis-gated config `Init`. Run `ttflow` once to initialize the
covenant (it then plays a scripted match; stop it after `Init` lands or let it play out):

```bash
TT_WRPC_URL=ws://127.0.0.1:17210 \
TT_NETWORK=simnet \
TT_LANE_ID=<lane id from the ttd log> \
TT_COVENANT_ID=<covenant id from /api/state> \
TTFLOW_PRIVATE_KEY=<32-byte-hex-operator-key> \
cargo run -p vprog-tictactoe-driver
```

### 5. Run the web frontend

```bash
cd web
npm install
npm run dev
```

The dev server proxies `/api/*` to the DA server on `127.0.0.1:9880`. `VITE_WRPC_URL` and
`VITE_NETWORK` override the L1 endpoint and network for the in-page wallet.

### 6. Fund and play

1. Open the dev URL, paste a 32-byte hex private key, press Load. The key bar shows the derived
   address; the key lives in memory only and is cleared on reload.
2. Send L1 funds to that displayed address from your simnet wallet. Carrier fees and deposits
   spend from this L1 balance.
3. Create or join a game (the confirmation line shows the covenant deposit address for the
   deposit carrier), play turns on the board, and transfer or withdraw from the actions column.
   Claim buttons appear once `/api/exits` holds settled leaves, which awaits the settlement fix
   above.

### 7. Full automated loop

```bash
TT_E2E=1 cargo test --release -p vprog-tictactoe-driver --test e2e_simnet -- --nocapture
```

Spins an in-process simnet and plays a whole match through settlement, withdraw, and the claim
tail. Currently fails at the claim tail on the vprogs settlement defect above; every scenario
carrier before it is accepted.
