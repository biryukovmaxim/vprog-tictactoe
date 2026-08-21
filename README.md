# vprog-tictactoe

Tic-tac-toe with stakes as a [vprogs](../vprogs) guest program on Kaspa: two players lock a stake,
play a multi-round match, and the verifiable program settles the pot to the winner (or splits it
back on a draw). Reuses the vprogs framework and Kaspa stack for everything except the program's
own actions, accounts and game rules.

> **Status: scaffold.** Tooling, repo rules and empty crate skeletons (`node/`, `driver/`,
> `guest/`, all edition 2024); no program logic has landed yet.
> The checklist below is the source of truth for what works.

## Architecture (planned)

```
┌──────────────────────────────────────────────────────────────────┐
│ web/          frontend: board UI, simple privkey wallet,         │
│               submits action carrier txs to L1 (Kaspa) directly  │
├──────────────────────────────────────────────────────────────────┤
│ node/ (`ttd`) vprogs runner driver + DA server: the data the     │
│               frontend renders (games, accounts, trust ladder)   │
├──────────────────────────────────────────────────────────────────┤
│ guest/        RISC0 guest program:                               │
│               runtime/  framework (ported from vprogs            │
│                         runtime-processor: resource IDs, locks,  │
│                         lifecycle, ix wire format)               │
│               program/  tic-tac-toe: accounts + game resources,  │
│                         entry/exit/transfer + game actions       │
├──────────────────────────────────────────────────────────────────┤
│ ../vprogs    everything else, used as-is via path dependency:    │
│              zk-abi, batch/aggregator guests, runner, L1 bridge  │
└──────────────────────────────────────────────────────────────────┘
```

## The game

- A **match** is a fixed number of rounds (default 3). X/O assignment alternates every round:
  the creator (seat 0) plays X in even rounds, the joiner (seat 1) in odd rounds.
- All rounds are always played. `outcome`: `Pending | First | Second | Draw` by seat.
- **Stake**: each player locks one stake into the pot (`2 × stake`) at create/join. On the last
  round's completion the seat with more round wins takes the pot; equal wins split it back
  exactly. Funds are rollup account balances: entry (deposit) creates/funds accounts, exit
  (withdraw) returns to L1, transfer moves between accounts.
- One transaction carries a **list of actions** applied in order (e.g. deposit + create game).

## Status

- [ ] Guest crate: `runtime/` framework port
- [ ] Guest crate: `program/` accounts (config Init, deposit, transfer, withdraw)
- [ ] Guest crate: `program/` game (create, join, turn; settlement and split)
- [ ] Guest tests: rules engine, wire round-trips, dev-mode flow tests
- [ ] Node `ttd`: runner driver + DA server
- [ ] Driver `ttflow`: scripted scenarios
- [ ] Web frontend: wallet, board, trust ladder (optimistic → L2 → settled → confirmed)
- [ ] Env-gated L1 e2e against a local testnet-10 fork node

## Running

Requires the [vprogs](../vprogs) clone checked out next to this repo, a Rust nightly toolchain,
`just`, `taplo`, and for the frontend a Node toolchain. Commands activate as the corresponding
crates land:

```bash
just check     # clippy, warnings denied
just test      # workspace + guest tests
```

## Documentation

Public docs live under `docs/` as they are written; internal specs and plans are not part of the
repository.
