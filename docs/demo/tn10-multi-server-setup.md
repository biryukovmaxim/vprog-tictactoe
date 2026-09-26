# tn10 multi-server setup (prover / L1 node / app server)

Brings the tic-tac-toe rollup up on testnet-10 across three machines: fresh lane, fresh
covenant, real CUDA proofs, playable from a browser, claims included.

| Role | Machine | Runs | Needs |
|---|---|---|---|
| L1 node | any (e.g. `nyc`) | rusty-kaspa `kaspad` | open p2p `:16211` + wRPC borsh `:17210` |
| Prover | CUDA GPU (e.g. RTX 5080 16 GB) | `ttd` (`--features cuda`) | reaches L1 wRPC; CUDA 12.x toolkit |
| App server | any | `ttd` (keyless exec follower, DA + game) + `ttflow`/`claim` + web UI | reaches L1 wRPC; serves `:9880` + `:5173` |

Everything below assumes bash, Rust stable, and git on all machines. The L1 node is
the only one with long-lived state; prover and app server start from empty dirs.

Branches and pins:

- vprog-tictactoe: `master` (everything verified here has merged; host crates resolve
  through the git pins in `Cargo.toml`).
- vprogs: clone, then check out **the branch pinned in this repo's `Cargo.toml`**
  (currently `fix/settlement-watch-wedge`) — host crates and the guest resolve
  through those git pins, and the clone is only needed for the backend ELFs, which are
  committed to the repo (no build step). ELFs from any other rev (including `master`)
  mismatch the pinned host crates.
- rusty-kaspa (L1 only): the commit this repo's `Cargo.toml` pins for rusty-kaspa
  (master has been mid-refactor; do not assume it builds).

Full environment-variable reference: `docs/demo/reference.md`.

## 1. L1 node (rusty-kaspa)

```bash
git clone https://github.com/kaspanet/rusty-kaspa.git
cd rusty-kaspa && git checkout <rusty-kaspa rev from vprog-tictactoe/Cargo.toml>
cargo build --release -p kaspad
./target/release/kaspad --utxoindex --testnet --netsuffix=10 \
  --listen=0.0.0.0 --rpclisten=0.0.0.0 --rpclisten-borsh=0.0.0.0 --disable-upnp \
  --externalip=<this-machine-public-ip>
```

Notes: tn10 DNS seeds are dead — if this node must sync from scratch, connect it to
an existing peer with `--connect=<peer>:16211`. wRPC borsh on `:17210` is what every
other machine talks to (`ws://<L1-ip>:17210`). IBD of tn10 from zero is slow
(hours); a kept-warm node is worth having. Go kaspad does NOT work (p2p protocol 5
is dropped by rusty peers).

## 2. Prover (CUDA machine)

```bash
git clone https://github.com/kaspanet/vprogs.git
git -C vprogs checkout fix/settlement-watch-wedge   # the branch pinned in Cargo.toml
git clone https://github.com/biryukovmaxim/vprog-tictactoe.git
cd vprog-tictactoe
```

The guest zkVM ELF is gitignored — build it once with `just build-guest-docker` (Docker,
no local RISC Zero toolchain) or `just build-guest` (local rzup `risc0` toolchain, see
https://dev.risczero.com); both write `guest/compiled/program.elf`. The CI `artifacts`
workflow ships the same file (`program-elf` artifact) if you would rather fetch than
build.

Build the CUDA node (`target-cuda` keeps it separate from CPU builds):

```bash
PATH=/usr/local/cuda/bin:$PATH CARGO_TARGET_DIR=target-cuda \
  cargo build --release --features cuda -p vprog-tictactoe-node
```

Generate a fresh issuer key (do NOT reuse old funded keys on tn10 — the dust storm
bloats old addresses past the wRPC 60 s timeout) and derive its address:

```bash
openssl rand -hex 32 > ttd.key
# prints "operator address (fund this before running): kaspatest:..." then fails — expected
timeout 5 env TT_NETWORK=tn10 TT_WRPC_URL=ws://<L1-ip>:17210 TT_LANE_ID=1 \
  TT_COVENANT_ID=0000000000000000000000000000000000000000000000000000000000000000 \
  TTFLOW_PRIVATE_KEY=$(cat ttd.key) cargo run --release -p vprog-tictactoe-driver
```

Fund it by mining (no local node needed; ~1 block/s single-threaded; tn10 coinbase
maturity is 1000 DAA ≈ 2 min — mine ~3 min, then expect the first bootstrap attempt
to fail with "no spendable UTXO" until the coinbases mature):

```bash
cargo build --release -p vprog-tictactoe-driver --examples
TT_NETWORK=tn10 ./target/release/examples/wrpc-miner ws://<L1-ip>:17210 <issuer-address>
```

Run the prover (`RISC0_DEV_MODE` must be UNSET for real proofs — note the repo's
`.cargo/config.toml` defaults it to `1`, so run the binary directly with `env -u`):

```bash
mkdir -p $HOME/tt-run/TN && cat > $HOME/tt-run/TN/env <<EOF
TT_WRPC_URL=ws://<L1-ip>:17210
TT_NETWORK=tn10
TT_MIN_CONFIRMATIONS=10
TT_PROVE=1
TT_DATA_DIR=$HOME/tt-run/TN/data
TT_PROGRAM_ELF=$PWD/guest/compiled/program.elf
TT_BATCH_ELF=$PWD/../vprogs/zk/backend/risc0/batch-processor/compiled/program.elf
TT_AGGREGATOR_ELF=$PWD/../vprogs/zk/backend/risc0/batch-aggregator/compiled/program.elf
EOF
set -a; . $HOME/tt-run/TN/env; set +a
export TT_PRIVATE_KEY=$(cat ttd.key)
nohup env -u RISC0_DEV_MODE ./target-cuda/release/ttd > $HOME/tt-run/TN/ttd.log 2>&1 &
```

Success looks like: `lane id=… mode=Fresh`, `covenant <hex> ready (bootstrap tx …)`,
`DA server listening`. If it panics with "no spendable UTXO for bootstrap", the
coinbases are not mature yet — restart it a minute later (Fresh mode rolls a new
lane id on restart; that is fine). The same issuer key doubles as the `ttflow`
operator key later, saving a funding round.

Anchors for the app server (after bootstrap):

```bash
cat $HOME/tt-run/TN/data/vprun-state.json   # lane_id, covenant_id, bootstrap_txid, bootstrap_block_hash
```

## 3. App server (DA + execution + frontend)

Same clones as the prover (vprogs sibling for the backend ELFs, guest ELF build).
No CUDA needed:

```bash
cargo build --release -p vprog-tictactoe-node -p vprog-tictactoe-driver --examples
```

Join as a keyless exec follower using the prover's anchors:

```bash
mkdir -p $HOME/tt-data && nohup env \
  TT_WRPC_URL=ws://<L1-ip>:17210 TT_NETWORK=tn10 TT_MIN_CONFIRMATIONS=10 TT_PROVE=0 \
  TT_LANE_ID=<lane_id> TT_COVENANT_ID=<covenant_id> \
  TT_BOOTSTRAP_TXID=<bootstrap_txid> TT_START_FROM=<bootstrap_block_hash> \
  TT_DATA_DIR=$HOME/tt-data \
  TT_PROGRAM_ELF=$PWD/guest/compiled/program.elf \
  TT_BATCH_ELF=$PWD/../vprogs/zk/backend/risc0/batch-processor/compiled/program.elf \
  TT_AGGREGATOR_ELF=$PWD/../vprogs/zk/backend/risc0/batch-aggregator/compiled/program.elf \
  ./target/release/ttd > $HOME/ttd.log 2>&1 &
curl -s http://127.0.0.1:9880/api/state    # readiness: covenant id present
```

Frontend (proxies `/api` to the local DA on `:9880`; fresh clones need the wasm vendor
tarballs first — `just web-vendor`, or unpack the CI `wasm-vendor` artifact into
`web/vendor/`):

```bash
cd web && npm install && npm run dev -- --host 0.0.0.0   # http://<app-server>:5173
```

## 4. Play a match and claim

Fund one more key per fixed player you want to claim with (players deposit from
their own L1 keys; player B is ephemeral and its leaf stays unclaimed). Reuse the
issuer key as the `ttflow` operator. Miner + address-derivation as in §2.

```bash
env TT_WRPC_URL=ws://<L1-ip>:17210 TT_NETWORK=tn10 \
  TT_LANE_ID=<lane_id> TT_COVENANT_ID=<covenant_id> \
  TT_DA_URL=http://127.0.0.1:9880 \
  TTFLOW_PRIVATE_KEY=$(cat issuer.key) \
  TTFLOW_PLAYER_A_KEY=$(cat player-a.key) \
  TTFLOW_TRANSFER_AMOUNT=25000000 \
  ./target/release/ttflow
```

Init is genesis-gated (first run only). Steps: Init, deposits, CreateGame,
JoinGame, transfer, turns, Withdraw — ~1 min. The prover settles with a CUDA proof
in a few minutes; watch `ttd.log` on the prover, then:

```bash
curl -s http://127.0.0.1:9880/api/exits    # claimable leaves
env TT_WRPC_URL=ws://<L1-ip>:17210 TT_NETWORK=tn10 TT_COVENANT_ID=<covenant_id> \
  TT_DA_URL=http://127.0.0.1:9880 TTFLOW_PLAYER_A_KEY=$(cat player-a.key) \
  ./target/release/examples/claim           # one leaf per invocation; rerun for the next
```

Claims burn an estimated fee from the player's own UTXOs and pay the leaf to the
player's address; spend marks appear in `/api/exits` after ~10 confirmations.
Useful reads: `/api/state`, `/api/games?status=finished`, `/api/exits`,
`/api/accounts/<user-id-hex>`.

## 5. Teardown / reset

Kill `ttd` (app server and prover), then `rm -rf` each `TT_DATA_DIR`. On-chain
artifacts (lane, covenant, settlements) are immutable and stay on tn10; a new run
with empty data dirs boots a fresh lane and covenant. Nothing else needs cleanup —
the L1 node keeps syncing.
