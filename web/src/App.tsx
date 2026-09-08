//! One page: KeyBar on top, three columns below (columns land in tasks 7-10).

import { useCallback, useState } from 'react';
import { KeyBar, type Identity } from './KeyBar';
import { useDa } from './state';

export default function App() {
  const [identity, setIdentity] = useState<Identity | null>(null);
  const da = useDa();
  const onIdentity = useCallback((id: Identity | null) => setIdentity(id), []);

  return (
    <>
      <KeyBar onIdentity={onIdentity} />
      {!da.reachable && <div className="banner">DA server unreachable — retrying…</div>}
      {identity && (
        <div className="cols">
          <section aria-label="open games" />
          <section aria-label="match" />
          <section aria-label="actions" />
        </div>
      )}
    </>
  );
}
