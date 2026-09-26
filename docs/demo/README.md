# Demo runbook

A runnable demo of the tic-tac-toe rollup: an in-process simnet L1, the `ttd` node driver with
the DA server, a one-shot `ttflow` covenant init, and the browser UI (privkey wallet, open games,
live match board, transfer/withdraw, exit claims). `ttd` runs in two proving modes:

- **devmode**: regular CPU build with `RISC0_DEV_MODE=1` — fake (stub) receipts, fast bundles.
- **cuda**: `--features cuda` build with `RISC0_DEV_MODE` unset — real proofs on the GPU; each
  bundle takes visibly longer than a dev stub.

Both modes run the identical flow below; only step 2 differs.

## Prerequisites

Toolchain:

- Rust (stable; nightly only for `just fmt` / `fmt-check` / `udeps`), `just`, `taplo`;
  Node/npm for the frontend
- CUDA mode only: CUDA 12.2 at `/usr/local/cuda-12.2` with `bin/` on `PATH` (nvcc), at
  build and run time

Three artifacts are build outputs, not committed files. Build each locally, or fetch it
from a CI `artifacts` run (Actions tab → `artifacts` → *Run workflow* / past runs; the
fetch commands are in [ops/deploy-web.md](../ops/deploy-web.md)):

| Artifact | Local build | CI artifact |
|---|---|---|
| guest ELF → `guest/compiled/program.elf` | `just build-guest` (rzup `risc0` toolchain) or `just build-guest-docker` (Docker only) | `program-elf` |
| wasm vendor tarballs → `web/vendor/*.tgz` | `just web-vendor` (wasm-pack + the `wasm32-unknown-unknown` rustup target) | `wasm-vendor` |
| backend ELFs (batch processor + aggregator) | none — committed to vprogs | — |

**Backend ELFs must come from the pinned vprogs rev.** Host crates resolve through the
git pins in `Cargo.toml` (currently branch `fix/settlement-watch-wedge`; the branch
name in every `vprogs-*` pin line is the one to use). The guest wire format changes
across vprogs branches, so ELFs from a checkout at any other rev — including `master` —
silently mismatch the pinned host. Clone vprogs next to this repo at the pinned branch:

```bash
git clone https://github.com/kaspanet/vprogs ../vprogs
git -C ../vprogs checkout fix/settlement-watch-wedge
```

The paths the runbook uses are then `../vprogs/zk/backend/risc0/batch-processor/compiled/program.elf`
and `../vprogs/zk/backend/risc0/batch-aggregator/compiled/program.elf`. (After any cargo
build, the same rev is also checked out under `~/.cargo/git/checkouts/vprogs-*/` —
pointing the env vars there works too and skips the clone.)

## Running

Starting the demo L1 and `ttd` fresh together keeps the run clean; the demo L1's throwaway
appdir already guarantees a fresh chain on every run. When starting over, also wipe `ttd`'s
state directory (`rm -rf ttd-data`) — it persists the previous chain's anchors and the bridge
dies on restart with "starting block no longer in chain".

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
  bytes, mined directly into the next block (claims no longer need it — they pay fees and go
  through the mempool — but it stays for ad-hoc direct mining)

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

CUDA (real proofs; `RISC0_DEV_MODE` must stay unset, and `/usr/local/cuda-12.2/bin` must be
on `PATH` at build and run time — the prover needs the CUDA tools):

```bash
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

Fresh clones need the vendored wasm tarballs first (`just web-vendor` from the repo
root, or unpack the CI `wasm-vendor` artifact into `web/vendor/`) — `npm install`
consumes them as `file:` dependencies and fails without them. Then:

```bash
cd web
npm install
npm run dev
```

The dev server proxies `/api/*` to the DA server on `127.0.0.1:9880`. `VITE_WRPC_URL` (default
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
   leaves), claim buttons appear. Claiming pays the leaf out to the wallet; the tx burns a fee
   from your own collateral and enters the mempool like any other transaction.

**Claim fees come from your own money, not the deposits.** The claim attaches one of your own
L1 UTXOs as fee collateral and burns the node's estimated top-priority fee
(`getFeeEstimate`'s priority bucket × the claim's mass) from it; the unburned remainder comes
back as change. The permission redeem script conserves delegate (deposit) inputs exactly —
they fund the payout and nothing else — pins the payout and permission-rent outputs exact, and
pins the output count, so deposit value can never burn or ride out. The demo L1's `/inject`
route is no longer used by claims; it remains available for ad-hoc direct mining. Claiming
needs the wallet to hold a UTXO larger than the estimated fee (the faucet funds this).

## Testnet-10 instead of the demo L1

The stack also runs against a real testnet-10 wRPC node. Replace the demo-L1 step and repoint
everything at the node:

- `ttd`: `TT_WRPC_URL=<tn10 wRPC url>` (`TT_NETWORK` already defaults to `tn10`), CUDA build
  with `RISC0_DEV_MODE` unset — on a shared chain, settlements must carry real proofs.
- Web: `VITE_WRPC_URL=<tn10 wRPC url>`, `VITE_NETWORK=testnet-10`.
- Fund keys by mining instead of the demo faucet:
  `cargo run --release -p vprog-tictactoe-driver --example wrpc-miner -- <tn10 wRPC url>
  <pay address>` solo-mines over plain wRPC (no local kaspad) and pays block coinbase to the
  address; testnet coinbase spends mature after roughly 1000 DAA-score. Keep the one-time
  `ttflow` init.

Claims pay fees from the claimer's own collateral (the node's feerate estimation picks the
amount), so they submit through the mempool on tn10 exactly as on the demo L1; the headless
variant is `cargo run --release -p vprog-tictactoe-driver --example claim` with the same
`TT_WRPC_URL`/`TT_NETWORK`/`TT_COVENANT_ID` env plus `TTFLOW_PLAYER_A_KEY` and, for the exit
feed, `TT_DA_URL` of any follower's DA.

The full multi-machine walkthrough (L1 node, CUDA prover, app server + web), as last
verified on tn10, is [tn10-multi-server-setup.md](tn10-multi-server-setup.md).

## Automated e2e

The whole flow headlessly — in-process simnet, init, deposits, a full match, settlement,
withdraw, exit records, two sequential claims, and payouts (dev stub proofs):

```bash
TT_E2E=1 RISC0_DEV_MODE=1 cargo test --release -p vprog-tictactoe-driver --test e2e_simnet -- --nocapture
```

## Reference

Run modes, environment variables, ports, HTTP endpoints, and failure-signature fixes:
[reference.md](reference.md).
