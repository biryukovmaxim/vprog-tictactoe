# vprog-tictactoe

Tic-tac-toe with stakes as a [vprogs](../vprogs) guest program on Kaspa: two players lock a stake,
play a multi-round match, and the verifiable program settles the pot to the winner (or splits it
back on a draw). Reuses the vprogs framework and Kaspa stack for everything except the program's
own actions, accounts and game rules.

> **Status: guest crate landed.** The guest's account-model baseline and staked game build and
> test clean (`just check`, `just test`): resource wire formats, lock/signer set, ix decoding,
> the config/deposit/transfer/withdraw actions, and the game actions (create, join, turn with
> pre-commit queues, early-clinch settlement), consuming vprogs' runtime-processor lib as the
> battery. The turn timeout action, node, driver and web have not landed yet.
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
│               runtime/  this app's runtime choices: lock/signer  │
│                         dispatchers over battery variant impls,  │
│                         env-provided genesis key, ix framing     │
│               program/  tic-tac-toe: kinds, domains, resource-   │
│                         id derivations (config/user/game),       │
│                         account + game resources (user carries   │
│                         game counters), actions (wire + apply),  │
│                         deposit-policy impl                      │
├──────────────────────────────────────────────────────────────────┤
│ ../vprogs    everything else, used as-is via path dependency:    │
│              runtime-processor lib as the reusable battery       │
│              (lock/signer + deposit-policy traits + variant      │
│              impls, auth, lifecycle, tx parsing; branch          │
│              guest-batteries), zk-abi, runner, L1 bridge         │
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

## Status

- [x] Guest crate: structure + batteries wiring (runtime-processor lib via path dep)
- [x] Guest crate: `program/` accounts (config Init, deposit, transfer, withdraw)
- [x] Guest crate: `program/` game (create, join, turn with pre-commit queues; settlement and
      split with early clinch; per-creator sequential game ids from the user's
      `games_started` counter)
- [ ] Guest tests: rules engine, wire round-trips, dev-mode flow tests
- [ ] Guest ELF: Docker reproducible build, genesis env flow (local `just build-guest` works)
- [ ] vprogs feature: app-level custom journal data via closures (prerequisite for indexes)
- [ ] Node DA: indexer logic to find open games (rides on the vprogs feature above)
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
