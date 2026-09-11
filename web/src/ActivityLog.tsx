//! Activity log: one row per submitted action with its trust chip. Rows are
//! created `pending` at submit and advanced pending -> on L2 -> settled by the
/// pure poll walker (match.ts `advanceActivity`) driven from App.

import { shortHex, type ActivityRow } from './composition';

export function ActivityLog({ rows }: { rows: ActivityRow[] }) {
  return (
    <div className="stack">
      <h3>activity</h3>
      {rows.length === 0 && <p className="hint">nothing submitted yet</p>}
      <ul className="activity">
        {rows.map((r) => (
          <li key={r.id}>
            {r.label} · {shortHex(r.txid)} <span className={`chip ${chipClass(r.status)}`}>{r.status}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function chipClass(status: ActivityRow['status']): string {
  return status === 'pending' ? 'pending' : status === 'on L2' ? 'l2' : 'settled';
}
