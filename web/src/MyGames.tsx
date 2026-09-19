//! My games list: playing + finished games I sit in; click selects into the
//! match column (MatchPanel itself lands in a later task).

import { kas, shortHex } from './composition';
import type { Identity } from './KeyBar';
import { useDa } from './state';

export function MyGames({
  identity,
  selectedId,
  onSelect,
}: {
  identity: Identity;
  selectedId: string | null;
  onSelect: (gameId: string) => void;
}) {
  const da = useDa();
  // Own open games list here too: OpenGames hides them (no self-join).
  const mine = [...da.games.open, ...da.games.playing, ...da.games.finished].filter(
    (g) => g.players[0] === identity.userIdHex || g.players[1] === identity.userIdHex,
  );

  return (
    <div className="stack">
      <h3>my games</h3>
      {mine.length === 0 && <p className="hint">no games yet — create or join one</p>}
      {mine.map((g) => {
        const other = g.players[0] === identity.userIdHex ? g.players[1] : g.players[0];
        return (
          <button key={g.id} className={`game-row${g.id === selectedId ? ' selected' : ''}`} onClick={() => onSelect(g.id)}>
            <span>{g.state_name}</span>
            <span>stake {kas(g.stake)}</span>
            <span>vs {other ? shortHex(other) : 'waiting for opponent'}</span>
          </button>
        );
      })}
    </div>
  );
}
