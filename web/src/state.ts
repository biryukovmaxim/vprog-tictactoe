//! Polled aggregate store over the DA API — plain React context, no state lib.
//! Polls every endpoint every 2 s; components read through `useDa()`.

import { createContext, createElement, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  fetchConfig,
  fetchExits,
  fetchGames,
  fetchState,
  type DaConfig,
  type DaGame,
  type DaState,
  type ExitRoot,
  type GameStatus,
} from './da';

const POLL_MS = 2_000;

export interface DaSnapshot {
  state: DaState | null;
  config: DaConfig | null;
  exits: ExitRoot[];
  games: Record<GameStatus, DaGame[]>;
  /// False while the DA server is unreachable (red banner per UI Design).
  reachable: boolean;
  error: string | null;
}

const EMPTY_GAMES: Record<GameStatus, DaGame[]> = { open: [], playing: [], finished: [] };

async function pollAll(): Promise<Omit<DaSnapshot, 'reachable' | 'error'>> {
  const [state, config, exits, open, playing, finished] = await Promise.all([
    fetchState(),
    fetchConfig(),
    fetchExits(),
    fetchGames('open'),
    fetchGames('playing'),
    fetchGames('finished'),
  ]);
  return { state, config, exits, games: { open, playing, finished } };
}

const DaContext = createContext<DaSnapshot>({
  state: null,
  config: null,
  exits: [],
  games: EMPTY_GAMES,
  reachable: true,
  error: null,
});

export function DaProvider({ children }: { children: ReactNode }) {
  const [snap, setSnap] = useState<DaSnapshot>({
    state: null,
    config: null,
    exits: [],
    games: EMPTY_GAMES,
    reachable: true,
    error: null,
  });
  const inflight = useRef(false);

  useEffect(() => {
    let stop = false;
    const tick = async () => {
      if (inflight.current) return;
      inflight.current = true;
      try {
        const fresh = await pollAll();
        if (!stop) setSnap({ ...fresh, reachable: true, error: null });
      } catch (e) {
        if (!stop) setSnap((s) => ({ ...s, reachable: false, error: String(e) }));
      } finally {
        inflight.current = false;
      }
    };
    tick();
    const id = setInterval(tick, POLL_MS);
    return () => {
      stop = true;
      clearInterval(id);
    };
  }, []);

  // createElement (not JSX) keeps this a plain .ts module.
  return createElement(DaContext.Provider, { value: snap }, children);
}

/// Latest DA snapshot, refreshed every 2 s.
export function useDa(): DaSnapshot {
  return useContext(DaContext);
}

/// Memoized game lookup by id across all statuses.
export function useGame(id: string | null): DaGame | null {
  const { games } = useDa();
  return useMemo(
    () => (id === null ? null : games.open.find((g) => g.id === id) ?? games.playing.find((g) => g.id === id) ?? games.finished.find((g) => g.id === id) ?? null),
    [games, id],
  );
}
