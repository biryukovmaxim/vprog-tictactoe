//! Typed client for the node crate's DA HTTP API (node/src/da.rs).
//! Types mirror the served JSON exactly; absent-on-uninitialized fields are
//! optional because the node `skip_serializing_if`s them away.

/// `GET /api/state` — `StateResponse`.
export interface DaState {
  l2_tip: number;
  covenant_id: string;
  lane_subnet: string;
  deposit_address: string;
  settled: SettlementInfo | null;
}

/// `StateResponse.settled` — from the latest paired settlement record.
export interface SettlementInfo {
  state_root: string;
  permission_root: string;
  txid: string;
  daa_score: number;
}

/// `GET /api/config` — `ConfigResponse`; only `initialized` when false.
export interface DaConfig {
  initialized: boolean;
  min_withdrawal_amount?: number;
  turn_ttl?: number;
  covenant_id?: string;
  lock_tag?: number;
}

/// `GameBody` wire view — `game_json` in da.rs.
export interface DaGame {
  id: string;
  /** 0 Open, 1 Playing, 2 First, 3 Second, 4 Draw (>= 2 finished). */
  state: number;
  state_name: string;
  stake: number;
  pot: number;
  rounds_total: number;
  round_wins: [number, number];
  draws: number;
  players: [string, string | null];
  /** 0 empty, 1 X, 2 O, row-major 3x3. */
  board: number[];
  last_move_at: number;
}

/// `GET /api/accounts/:id` — `AccountResponse`; only `exists` when absent.
export interface DaAccount {
  exists: boolean;
  balance?: number;
  games_started?: number;
  games_won?: number;
  games_finished?: number;
  lock_hash?: string;
}

/// One exit leaf of a settled exit root — `exit_leaf_json` in da.rs.
export interface ExitLeaf {
  index: number;
  spk_hex: string;
  amount: number;
  spent: { spend_txid: string; deduct: number } | null;
  siblings: string[];
  full_claim: { new_root: string; new_unclaimed: number };
}

/// One materialized exit root — `exit_root_json` in da.rs.
export interface ExitRoot {
  root: string;
  settlement_txid: string;
  outpoint_index: number;
  daa_score: number;
  unclaimed: number;
  leaves: ExitLeaf[];
}

async function get<T>(path: string): Promise<T> {
  const resp = await fetch(path);
  if (!resp.ok) throw new Error(`${path}: HTTP ${resp.status}`);
  return resp.json() as Promise<T>;
}

export type GameStatus = 'open' | 'playing' | 'finished';

export function fetchState(): Promise<DaState> {
  return get('/api/state');
}

export function fetchConfig(): Promise<DaConfig> {
  return get('/api/config');
}

export function fetchGames(status: GameStatus = 'open', after?: string, limit?: number): Promise<DaGame[]> {
  const q = new URLSearchParams({ status });
  if (after !== undefined) q.set('after', after);
  if (limit !== undefined) q.set('limit', String(limit));
  return get<{ games: DaGame[] }>(`/api/games?${q}`).then((r) => r.games);
}

export function fetchGame(id: string): Promise<DaGame> {
  return get(`/api/games/${id}`);
}

export function fetchAccount(userIdHex: string): Promise<DaAccount> {
  return get(`/api/accounts/${userIdHex}`);
}

export function fetchExits(): Promise<ExitRoot[]> {
  return get<{ roots: ExitRoot[] }>('/api/exits').then((r) => r.roots);
}
