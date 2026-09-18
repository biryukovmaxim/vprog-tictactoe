//! 3x3 board: row-major cells 0..=8 (0 empty, 1 X, 2 O). A cell is clickable
//! only while the board is enabled and the cell is empty; ghost marks render
//! pending state — in-flight turns at plain opacity, queued premoves numbered.

export interface Ghost {
  cell: number;
  mark: 1 | 2;
  /// Queue position for premoves; absent for in-flight turns.
  order?: number;
}

export function Board({
  board,
  enabled,
  ghosts = [],
  onCell,
}: {
  board: number[];
  enabled: boolean;
  ghosts?: Ghost[];
  onCell: (cell: number) => void;
}) {
  const ghostAt = (i: number) => ghosts.find((g) => g.cell === i && board[i] === 0);
  return (
    <div className="board">
      {board.map((cell, i) => {
        const ghost = ghostAt(i);
        return (
          <button
            key={i}
            className="cell"
            disabled={!enabled || cell !== 0 || ghost !== undefined}
            onClick={() => onCell(i)}
            aria-label={`cell ${i}${cell === 1 ? ', X' : cell === 2 ? ', O' : ghost ? `, pending ${ghost.mark === 1 ? 'X' : 'O'}` : ''}`}
          >
            {cell === 1 ? 'X' : cell === 2 ? 'O' : ghost ? (
              <span className="ghost">
                {ghost.mark === 1 ? 'X' : 'O'}
                {ghost.order !== undefined && <sup>{ghost.order}</sup>}
              </span>
            ) : (
              ''
            )}
          </button>
        );
      })}
    </div>
  );
}
