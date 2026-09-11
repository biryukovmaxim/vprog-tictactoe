//! 3x3 board: row-major cells 0..=8 (0 empty, 1 X, 2 O); a cell is clickable
//! only while the board is enabled and the cell is empty.

export function Board({ board, enabled, onCell }: { board: number[]; enabled: boolean; onCell: (cell: number) => void }) {
  return (
    <div className="board">
      {board.map((cell, i) => (
        <button
          key={i}
          className="cell"
          disabled={!enabled || cell !== 0}
          onClick={() => onCell(i)}
          aria-label={`cell ${i}${cell === 1 ? ', X' : cell === 2 ? ', O' : ''}`}
        >
          {cell === 1 ? 'X' : cell === 2 ? 'O' : ''}
        </button>
      ))}
    </div>
  );
}
