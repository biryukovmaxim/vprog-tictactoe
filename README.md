# vprog-tictactoe

Tic-tac-toe with stakes as a [vprogs](../vprogs) guest program on Kaspa: two players lock a
stake into a pot, play a multi-round match, and a verifiable program settles the pot to the
winner (a draw splits it back). Everything except the game's own actions, accounts, and rules
is reused from the vprogs framework and the Kaspa stack.

## Layout

| Crate | What it is |
|---|---|
| `guest/` | RISC0 guest program and the shared wire library (host and zkVM) |
| `node/` (`ttd`) | Runner driver + DA server: `/api/state`, `/api/config`, `/api/games`, `/api/accounts/:id`, `/api/exits` |
| `driver/` (`ttflow`) | Scripted scenario issuer, demo L1 (in-process simnet with faucet), e2e test |
| `encoder-wasm/` | Build/sign surface for the browser over the guest encoders and app-kit |
| `web/` | Vite/React frontend: privkey wallet, board, transfer/withdraw, exit claims |

## Run the demo

The full runbook with funding, live limits, and troubleshooting is
[`docs/demo/README.md`](docs/demo/README.md). Short version:

1. **Demo L1** — `cargo run -p vprog-tictactoe-driver --example demo-l1`: in-process simnet,
   wRPC on `ws://127.0.0.1:17210`, faucet on `http://127.0.0.1:9890`.
2. **Node + DA** — run `ttd` (`cargo run -p vprog-tictactoe-node`) with `TT_PROVE=1
   RISC0_DEV_MODE=1`, the backend ELFs, and a funded operator key; DA on `127.0.0.1:9880`.
3. **Init** — run `ttflow` (`cargo run -p vprog-tictactoe-driver`) once for the
   genesis-gated config `Init` (it then plays a scripted match).
4. **Web** — `cd web && npm install && npm run dev`; paste a 32-byte hex key, fund it from
   the faucet, create/join a game, play, transfer, withdraw, and claim settled exits.

### Testnet-10

The same stack runs against a testnet-10 wRPC node instead of the demo L1: `TT_WRPC_URL` /
`VITE_WRPC_URL` pointed at the node, `VITE_NETWORK=testnet-10`, keys funded from a tn10
faucet, and the same one-time `ttflow` init. Differences that matter:

- Settlements must carry real proofs: use the CUDA build (`--features cuda`, `RISC0_DEV_MODE`
  unset). Dev stub receipts are for the local simnet demo only.
- Claims pay fees from the claimer's own collateral (feerate-estimated) and submit through
  the mempool on tn10 exactly as on the demo L1.
- Start `ttd` against a fresh lane (wipe `ttd-data` when starting over): the bridge cannot
  join an already-live lane.

This path is configured but not yet exercised end to end; the verified flow is the simnet
runbook above.

## Dev

Requires the [vprogs](../vprogs) clone next to this repo, a Rust stable toolchain (nightly
only for `just fmt`), `just`, `taplo`, and a Node toolchain for the frontend.

```bash
just check        # clippy, warnings denied
just test         # workspace + guest tests
just build-guest  # compile guest ELF to guest/compiled/program.elf
cd web && npm run test && npm run build
```

Environment variables, run modes, ports, and failure signatures: see the
[demo reference](docs/demo/reference.md).
