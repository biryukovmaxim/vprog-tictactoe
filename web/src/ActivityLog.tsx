//! Activity log: one row per submitted action with its trust chip. Rows are
//! created `pending` at submit and advanced pending -> on L2 -> settled by the
//! pure poll walker (match.ts `advanceActivity`) driven from App. A row still
//! pending past the healthy window flips to `no L2 effect` — the carrier was
//! most likely rejected by the rollup, which nothing reports back.

import { shortHex, type ActivityRow } from './composition';
import { rowStuck } from './match';

export function ActivityLog({ rows, now = Date.now() }: { rows: ActivityRow[]; now?: number }) {
  return (
    <div className="stack">
      <h3>activity</h3>
      {rows.length === 0 && <p className="hint">nothing submitted yet</p>}
      <ul className="activity">
        {rows.map((r) => {
          const stuck = rowStuck(r, now);
          return (
            <li key={r.id}>
              {r.label} · {shortHex(r.txid)}{' '}
              <span
                className={`chip ${chipClass(r.status, stuck)}`}
                title={stuck ? 'submitted but never landed on L2 — the carrier was most likely rejected' : undefined}
              >
                {stuck ? 'no L2 effect' : r.status}
              </span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function chipClass(status: ActivityRow['status'], stuck: boolean): string {
  if (stuck) return 'stuck';
  return status === 'pending' ? 'pending' : status === 'on L2' ? 'l2' : 'settled';
}
