//! Match column: header round/phase, to-move line, board, tally, stake/pot,
//! and the outcome banner. Clicks submit one single-Turn carrier each, over
//! the same wallet/pickUtxo/fee-pad pipeline the create/join entries use.

import { useState } from 'react';
import { FEE_ESTIMATE, kas, pickUtxo, sharedClient } from './composition';
import type { DaGame } from './da';
import type { Identity } from './KeyBar';
import { Board } from './Board';
import { isMyTurn, outcomeLine, seatOf, toMoveMark, type ActivityWitness } from './match';
import { useDa, useGame } from './state';
import type { RpcClient } from './kaspa-pkg/kaspa.js';
import { NETWORK } from './wallet';
import { network_params, turn_tx, UtxoCandidate } from './wasm/vprog_tictactoe_encoder_wasm.js';

/// One Turn carrier: signs `turn_tx` for `cell` and reports the activity row
/// with the pre-submit board as the on-L2 witness. Turns move no L2 balance,
/// so the single-UTXO gate only needs to cover the carrier fee.
async function submitTurn(opts: {
  identity: Identity;
  client: RpcClient;
  lane: string;
  game: DaGame;
  cell: number;
  onActivity: (label: string, txid: string, witness?: ActivityWitness) => void;
  onNeedsFunding: (needed: bigint) => void;
}): Promise<void> {
  const { identity, client, lane, game, cell, onActivity, onNeedsFunding } = opts;
  const utxos = await identity.wallet.l1Utxos(client);
  const picked = pickUtxo(utxos, FEE_ESTIMATE);
  if (!picked) {
    onNeedsFunding(FEE_ESTIMATE);
    throw new Error(`insufficient L1 funds — fund ${identity.wallet.address}`);
  }
  const opponent = game.players[0] === identity.userIdHex ? game.players[1] : game.players[0];
  if (!opponent) throw new Error('waiting for the opponent to join');
  const bytes = turn_tx(
    identity.privkeyHex,
    network_params(NETWORK),
    new UtxoCandidate(picked.txid_hex, picked.index, picked.amount, picked.spk_hex, picked.spk_version),
    identity.wallet.address,
    lane,
    game.id,
    identity.userIdHex,
    opponent,
    cell,
  );
  const txid = await identity.wallet.submitTx(client, bytes);
  onActivity(`turn ${cell}`, txid, { kind: 'board', gameId: game.id, board: game.board });
}

export function MatchPanel({
  identity,
  gameId,
  onActivity,
  onNeedsFunding,
}: {
  identity: Identity;
  gameId: string | null;
  onActivity: (label: string, txid: string, witness?: ActivityWitness) => void;
  onNeedsFunding: (needed: bigint) => void;
}) {
  const da = useDa();
  const game = useGame(gameId);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (!game) {
    return (
      <div className="stack">
        <h3>match</h3>
        <p className="hint">select a game in my games</p>
      </div>
    );
  }

  const finished = game.state >= 2;
  const completed = game.round_wins[0] + game.round_wins[1] + game.draws;
  const round = finished ? Math.min(completed, game.rounds_total) : completed + 1;
  const seat = seatOf(game.players, identity.userIdHex);
  const myTurn = isMyTurn(game, identity.userIdHex);
  const mover = toMoveMark(game.board) === 1 ? 'X' : 'O';
  const outcome = outcomeLine(game.state, game.players, identity.userIdHex, BigInt(game.stake));

  const play = async (cell: number) => {
    if (busy) return;
    const lane = da.state?.lane_subnet;
    if (!lane) {
      setErr('waiting for DA state');
      return;
    }
    setErr(null);
    setBusy(true);
    try {
      await submitTurn({ identity, client: await sharedClient(), lane, game, cell, onActivity, onNeedsFunding });
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="stack">
      <h3>match</h3>
      <div className="match-head">
        Round {round}/{game.rounds_total} · {game.state_name}
      </div>
      {game.state === 1 ? (
        <div>
          to move: {mover}
          {seat < 0 ? '' : myTurn ? ' (you)' : ' (opponent)'}
        </div>
      ) : (
        game.state === 0 && <div className="hint">waiting for a joiner</div>
      )}
      <Board board={game.board} enabled={myTurn && !busy} onCell={play} />
      <div>
        wins {game.round_wins[0]}–{game.round_wins[1]} · draws {game.draws}
      </div>
      <div className="hint">
        stake {kas(game.stake)} · pot {kas(game.pot)}
      </div>
      {finished && outcome && <div className="outcome">{outcome}</div>}
      {err && <div className="err">{err}</div>}
    </div>
  );
}
