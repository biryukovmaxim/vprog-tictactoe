//! Lane carrier queue strip: the rollup's in-flight carriers decoded straight
//! from the L1 mempool (carriers.ts), one line under the settlement banner.
//! Once a carrier mines, the follower picks it up and it leaves this list;
//! carriers the guest rejects surface in the activity log as `no L2 effect`.

import { actionLabel, useLaneCarriers } from './carriers';

export function LaneQueue() {
  const carriers = useLaneCarriers();
  if (carriers.length === 0) return <div className="settle hint">L1 queue: empty</div>;
  return (
    <div className="settle hint">
      L1 queue: {carriers.map((c) => c.actions.map(actionLabel).join(' + ')).join('  ·  ')}
    </div>
  );
}
