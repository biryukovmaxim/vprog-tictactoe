//! DA HTTP server for ttd.
//!
//! Exposes node state and configuration endpoints for the web frontend.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use kaspa_addresses::{Address, Prefix, Version};
use serde::{Deserialize, Serialize};
use tower_http::{cors::CorsLayer, services::ServeDir};
use vprog_tictactoe_guest::program::resources::{
    config::ConfigBody,
    game::{GameBody, State as GameState},
    id::config_resource_id,
    user::UserBody,
};
use vprogs_core_types::ResourceId;
use vprogs_state_ptr_latest::StatePtrLatest;
use vprogs_state_version::StateVersion;
use vprogs_storage_rocksdb_store::RocksDbStore;
use vprogs_storage_types::{ReadStore, Store};
use vprogs_zk_backend_risc0_api::delegate_entry_spk_hash;

use crate::{
    da_store::{ExitView, exit_views, latest_settlement},
    indexer::{GameStatus, scan_games_by_status},
};

/// Shared DA server state.
#[derive(Clone)]
pub struct DaState {
    /// Storage engine handle.
    pub store: Arc<RocksDbStore>,
    /// Covenant id followed by this node.
    pub covenant_id: [u8; 32],
    /// Lane subnetwork namespace bytes.
    pub lane_subnet: Vec<u8>,
    /// Address prefix identifying the network.
    pub network_prefix: String,
    /// Optional directory holding static web frontend assets.
    pub web_dir: Option<String>,
}

/// Settlement status information.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettlementInfo {
    /// Hex-encoded state root after settlement.
    pub state_root: String,
    /// Hex-encoded permission tree root.
    pub permission_root: String,
    /// Hex-encoded settlement transaction id.
    pub txid: String,
    /// DAA score at which settlement occurred.
    pub daa_score: u64,
}

/// Response payload for `GET /api/state`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateResponse {
    /// Latest canonical L2 block tip index.
    pub l2_tip: u64,
    /// Hex-encoded covenant id.
    pub covenant_id: String,
    /// Hex-encoded lane subnetwork namespace.
    pub lane_subnet: String,
    /// Deposit P2SH address for user funding.
    pub deposit_address: String,
    /// Settlement details, or `None` if unsettled.
    pub settled: Option<SettlementInfo>,
}

/// Response payload for `GET /api/config`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigResponse {
    /// True if the on-chain config resource has been initialized.
    pub initialized: bool,
    /// Minimum allowed withdrawal amount in sompi.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_withdrawal_amount: Option<u64>,
    /// Turn TTL in DAA-score units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_ttl: Option<u64>,
    /// Hex-encoded covenant id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covenant_id: Option<String>,
    /// Lock tag identifying the config authority lock.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock_tag: Option<u8>,
}

/// Loads the latest committed byte data for `id`, returning `None` if absent.
pub fn latest_body<S: ReadStore>(store: &S, id: &ResourceId) -> Option<Vec<u8>> {
    StatePtrLatest::get(store, id)?;
    let sv = StateVersion::from_latest_data(store, *id);
    if sv.data().is_empty() { None } else { Some(sv.data().clone()) }
}

/// Parses a network prefix string into an address [`Prefix`].
fn parse_prefix(raw: &str) -> Prefix {
    Prefix::try_from(raw).unwrap_or_else(|_| match raw.to_lowercase().as_str() {
        "mainnet" => Prefix::Mainnet,
        "testnet" | "tn10" => Prefix::Testnet,
        "simnet" => Prefix::Simnet,
        "devnet" => Prefix::Devnet,
        _ => Prefix::Testnet,
    })
}

/// Handler for `GET /api/state`.
async fn get_state(State(state): State<DaState>) -> Json<StateResponse> {
    let l2_tip = state.store.canonical_chain().snapshot().tip();
    let covenant_id_hex = faster_hex::hex_string(&state.covenant_id);
    let lane_subnet_hex = faster_hex::hex_string(&state.lane_subnet);

    let prefix = parse_prefix(&state.network_prefix);
    let entry_spk_hash = delegate_entry_spk_hash(&state.covenant_id);
    let deposit_address = Address::new(prefix, Version::ScriptHash, &entry_spk_hash).to_string();

    let settled: Option<SettlementInfo> =
        latest_settlement(state.store.as_ref()).map(|s| SettlementInfo {
            state_root: faster_hex::hex_string(&s.state_root),
            permission_root: faster_hex::hex_string(&s.permission_root),
            txid: faster_hex::hex_string(&s.txid),
            daa_score: s.daa_score,
        });

    Json(StateResponse {
        l2_tip,
        covenant_id: covenant_id_hex,
        lane_subnet: lane_subnet_hex,
        deposit_address,
        settled,
    })
}

/// Handler for `GET /api/config`.
async fn get_config(State(state): State<DaState>) -> Json<ConfigResponse> {
    let config_id = config_resource_id();
    match latest_body(state.store.as_ref(), &config_id) {
        Some(bytes) => match ConfigBody::from_bytes(&bytes) {
            Ok(body) => Json(ConfigResponse {
                initialized: true,
                min_withdrawal_amount: Some(body.min_withdrawal_amount()),
                turn_ttl: Some(body.turn_ttl()),
                covenant_id: Some(faster_hex::hex_string(body.covenant_id())),
                lock_tag: Some(body.lock_tag()),
            }),
            Err(_) => Json(ConfigResponse {
                initialized: false,
                min_withdrawal_amount: None,
                turn_ttl: None,
                covenant_id: None,
                lock_tag: None,
            }),
        },
        None => Json(ConfigResponse {
            initialized: false,
            min_withdrawal_amount: None,
            turn_ttl: None,
            covenant_id: None,
            lock_tag: None,
        }),
    }
}

/// Builds the canonical exit-leaf JSON object from a view and its leaf position.
fn exit_leaf_json(view: &ExitView, index: usize) -> serde_json::Value {
    let leaf = &view.record.leaves[index];
    let spent = match &view.spent[index] {
        Some(entry) => serde_json::json!({
            "spend_txid": faster_hex::hex_string(&entry.spend_txid),
            "deduct": entry.deduct,
        }),
        None => serde_json::Value::Null,
    };
    serde_json::json!({
        "index": index,
        "spk_hex": faster_hex::hex_string(leaf.script_bytes()),
        "amount": leaf.amount,
        "spent": spent,
        "siblings": view.siblings[index]
            .iter()
            .map(|s| faster_hex::hex_string(s))
            .collect::<Vec<String>>(),
        "full_claim": {
            "new_root": faster_hex::hex_string(&view.full_claim_roots[index]),
            "new_unclaimed": view.record.unclaimed.saturating_sub(1),
        },
    })
}

/// Builds the canonical exit-root JSON object from a materialized view.
///
/// `root` is the family's current raw root (the redeem's `old_root` space) and `full_claim`
/// models a full-leaf deduct (the demo lock: full claims only).
fn exit_root_json(view: &ExitView) -> serde_json::Value {
    let rec = &view.record;
    let leaves: Vec<serde_json::Value> =
        (0..rec.leaves.len()).map(|i| exit_leaf_json(view, i)).collect();
    serde_json::json!({
        "root": faster_hex::hex_string(&view.root),
        "settlement_txid": faster_hex::hex_string(&rec.settlement_txid),
        "outpoint_index": rec.outpoint_index,
        "daa_score": rec.daa_score,
        "unclaimed": rec.unclaimed,
        "rent": rec.rent,
        "leaves": leaves,
    })
}

/// Handler for `GET /api/exits`.
async fn get_exits(State(state): State<DaState>) -> Json<serde_json::Value> {
    let views = exit_views(state.store.as_ref());
    let roots: Vec<serde_json::Value> = views.iter().map(exit_root_json).collect();
    Json(serde_json::json!({"roots": roots}))
}

/// Maps a [`GameState`] to its string name.
fn state_name(s: GameState) -> &'static str {
    match s {
        GameState::Open => "Open",
        GameState::Playing => "Playing",
        GameState::First => "First",
        GameState::Second => "Second",
        GameState::Draw => "Draw",
    }
}

/// Builds the canonical game JSON object from a raw body plus its id hex.
fn game_json(id_hex: String, body: &GameBody) -> serde_json::Value {
    let state = body.state();
    let rw = body.round_wins();
    let board: Vec<u8> = body.board().iter().map(|c| *c as u8).collect();
    let creator_hex = faster_hex::hex_string(body.creator().as_slice());
    let joiner: serde_json::Value = body
        .joiner()
        .map(|j| serde_json::Value::String(faster_hex::hex_string(j.as_slice())))
        .unwrap_or(serde_json::Value::Null);
    serde_json::json!({
        "id": id_hex,
        "state": state as u8,
        "state_name": state_name(state),
        "stake": body.stake(),
        "pot": body.stake() * 2,
        "rounds_total": body.rounds_total(),
        "creator_mark": body.creator_mark() as u8,
        "round_wins": [rw[0], rw[1]],
        "draws": body.draws(),
        "players": [creator_hex, joiner],
        "board": board,
        "last_move_at": body.last_move_at(),
    })
}

/// Query parameters for `GET /api/games`.
#[derive(Deserialize)]
struct GamesQuery {
    status: Option<String>,
    after: Option<String>,
    limit: Option<usize>,
}

/// Parses a 32-byte hex string into a `[u8; 32]`, returning `None` on failure.
fn parse_id_hex(hex: &str) -> Option<[u8; 32]> {
    let mut bytes = [0u8; 32];
    faster_hex::hex_decode(hex.as_bytes(), &mut bytes).ok().map(|_| bytes)
}

/// Handler for `GET /api/games?status=open|playing|finished&after=<hex32>&limit=<n>`. An
/// unrecognized status value is rejected with 400.
async fn get_games(State(state): State<DaState>, Query(q): Query<GamesQuery>) -> impl IntoResponse {
    let status = match q.status.as_deref() {
        None | Some("open") => GameStatus::Open,
        Some("playing") => GameStatus::Playing,
        Some("finished") => GameStatus::Finished,
        Some(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let after: Option<ResourceId> = q.after.as_deref().and_then(|h| {
        let mut bytes = [0u8; 32];
        faster_hex::hex_decode(h.as_bytes(), &mut bytes).ok().map(|_| ResourceId::from(bytes))
    });
    let limit = q.limit.unwrap_or(50).min(200);
    let store = state.store.as_ref();
    let snapshot = store.canonical_chain().snapshot();
    let ids = scan_games_by_status(store, &snapshot, status, after, limit);

    let games: Vec<serde_json::Value> = ids
        .iter()
        .filter_map(|id| {
            let rid = ResourceId::from(*id);
            let bytes = latest_body(store, &rid)?;
            let body = GameBody::from_bytes(&bytes).ok()?;
            Some(game_json(faster_hex::hex_string(id), body))
        })
        .collect();

    Json(serde_json::json!({"games": games})).into_response()
}

/// Handler for `GET /api/games/:id`.
async fn get_game(State(state): State<DaState>, Path(id_hex): Path<String>) -> impl IntoResponse {
    let Some(bytes_32) = parse_id_hex(&id_hex) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let rid = ResourceId::from(bytes_32);
    let store = state.store.as_ref();
    match latest_body(store, &rid).and_then(|b| {
        let body = GameBody::from_bytes(&b).ok()?;
        Some(game_json(id_hex, body))
    }) {
        Some(json) => Json(json).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Response for `GET /api/accounts/:id` when the account exists.
#[derive(Serialize)]
struct AccountResponse {
    exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    balance: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    games_started: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    games_won: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    games_finished: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lock_hash: Option<String>,
}

/// Handler for `GET /api/accounts/:id`.
async fn get_account(
    State(state): State<DaState>,
    Path(id_hex): Path<String>,
) -> impl IntoResponse {
    let Some(bytes_32) = parse_id_hex(&id_hex) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let rid = ResourceId::from(bytes_32);
    let store = state.store.as_ref();
    match latest_body(store, &rid) {
        Some(bytes) => match UserBody::from_bytes(&bytes) {
            Ok(body) => Json(AccountResponse {
                exists: true,
                balance: Some(body.balance()),
                games_started: Some(body.games_started()),
                games_won: Some(body.games_won()),
                games_finished: Some(body.games_finished()),
                lock_hash: Some(faster_hex::hex_string(body.initial_lock_hash())),
            })
            .into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        None => Json(AccountResponse {
            exists: false,
            balance: None,
            games_started: None,
            games_won: None,
            games_finished: None,
            lock_hash: None,
        })
        .into_response(),
    }
}

/// Constructs the DA server router.
pub fn router(state: DaState) -> Router {
    let web_dir = state.web_dir.clone();
    let api = Router::new()
        .route("/api/state", get(get_state))
        .route("/api/config", get(get_config))
        .route("/api/exits", get(get_exits))
        .route("/api/games", get(get_games))
        .route("/api/games/{id}", get(get_game))
        .route("/api/accounts/{id}", get(get_account))
        .layer(CorsLayer::permissive())
        .with_state(state);

    if let Some(dir) = web_dir { api.fallback_service(ServeDir::new(dir)) } else { api }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tempfile::TempDir;
    use tower::ServiceExt;
    use vprog_tictactoe_guest::{
        program::resources::{
            config::{config_total_len, write_config},
            game::{Cell, GAME_WIRE_LEN, GameBody, State, write_game},
            user::{GameStats, user_total_len, write_user},
        },
        runtime::lock::{LockEnum, UnlockedLockView},
    };
    use vprogs_l1_types::{PermissionSpend, SettlementInfo as L1SettlementInfo, TransactionId};
    use vprogs_runner::{ExitIndexer, ExitLeaf, ExitsForBundle};
    use vprogs_storage_types::{StateSpace, WriteBatch};
    use vprogs_zk_abi::withdrawal::StandardSpk;
    use vprogs_zk_backend_risc0_api::{PermissionTreeAccumulator, PermissionTreeView};
    use zerocopy::{IntoBytes, little_endian::U64};

    use super::*;
    use crate::{
        exit_index::TicTacToeExitIndexer,
        indexer::{GameStatus, GameStatusKey},
    };

    #[tokio::test]
    async fn test_api_config_uninitialized_then_initialized() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(RocksDbStore::open(dir.path()));
        let state = DaState {
            store: store.clone(),
            covenant_id: [0x11; 32],
            lane_subnet: vec![1, 2, 3, 4],
            network_prefix: "kaspasim".to_string(),
            web_dir: None,
        };

        let app = router(state.clone());
        let req = Request::builder().uri("/api/config").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, serde_json::json!({"initialized": false}));

        // Seed config in store.
        let lock = LockEnum::Unlocked(UnlockedLockView);
        let mut buf = vec![0u8; config_total_len(&lock)];
        write_config(&mut buf, 1000, 500, &[0x22; 32], &lock).unwrap();

        let config_id = config_resource_id();
        let mut wb = store.write_batch();
        StateVersion::put(&mut wb, 1, &config_id, &buf);
        StatePtrLatest::put(&mut wb, &config_id, 1);
        store.commit(wb);

        let app2 = router(state);
        let req2 = Request::builder().uri("/api/config").body(Body::empty()).unwrap();
        let resp2 = app2.oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let body2 = axum::body::to_bytes(resp2.into_body(), usize::MAX).await.unwrap();
        let json2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
        assert_eq!(json2["initialized"], true);
        assert_eq!(json2["min_withdrawal_amount"], 1000);
        assert_eq!(json2["turn_ttl"], 500);
        assert_eq!(json2["covenant_id"], faster_hex::hex_string(&[0x22; 32]));
    }

    #[tokio::test]
    async fn test_api_state() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(RocksDbStore::open(dir.path()));
        let covenant_id = [0x33; 32];
        let lane_subnet = vec![0xaa, 0xbb, 0xcc, 0xdd];
        let state = DaState {
            store: store.clone(),
            covenant_id,
            lane_subnet: lane_subnet.clone(),
            network_prefix: "kaspasim".to_string(),
            web_dir: None,
        };

        let app = router(state);
        let req = Request::builder().uri("/api/state").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["l2_tip"], 0);
        assert!(json["settled"].is_null());
        assert_eq!(json["covenant_id"], faster_hex::hex_string(&covenant_id));
        assert_eq!(json["lane_subnet"], faster_hex::hex_string(&lane_subnet));
        let deposit_addr = json["deposit_address"].as_str().unwrap();
        assert!(!deposit_addr.is_empty());
        assert!(deposit_addr.starts_with("kaspasim:"));
    }

    #[tokio::test]
    async fn test_web_dir_fallback() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(RocksDbStore::open(dir.path().join("db")));
        let web_dir = dir.path().join("web");
        std::fs::create_dir_all(&web_dir).unwrap();
        std::fs::write(web_dir.join("index.html"), "<html>hello</html>").unwrap();

        let state = DaState {
            store,
            covenant_id: [0x33; 32],
            lane_subnet: vec![1, 2, 3, 4],
            network_prefix: "kaspasim".to_string(),
            web_dir: Some(web_dir.to_str().unwrap().to_string()),
        };

        let app = router(state);
        let req = Request::builder().uri("/index.html").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body, "<html>hello</html>");
    }

    #[tokio::test]
    async fn test_api_accounts_absent_and_present() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(RocksDbStore::open(dir.path()));
        let state = DaState {
            store: store.clone(),
            covenant_id: [0x11; 32],
            lane_subnet: vec![1, 2, 3, 4],
            network_prefix: "kaspasim".to_string(),
            web_dir: None,
        };

        // 1. Absent account: exists == false.
        let absent_id = ResourceId::from([0x99; 32]);
        let absent_id_hex = faster_hex::hex_string(absent_id.as_slice());
        let app = router(state.clone());
        let req = Request::builder()
            .uri(format!("/api/accounts/{absent_id_hex}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, serde_json::json!({"exists": false}));

        // 2. Present account: seed via write_user.
        let lock = LockEnum::Unlocked(UnlockedLockView);
        let mut user_buf = vec![0u8; user_total_len(&lock)];
        let stats = GameStats { started: 3, won: 2, finished: 2 };
        let lock_hash = [0x55; 32];
        write_user(&mut user_buf, 75_000, stats, &lock_hash, &lock).unwrap();

        let user_id = ResourceId::from([0x42; 32]);
        let user_id_hex = faster_hex::hex_string(user_id.as_slice());
        let mut wb = store.write_batch();
        StateVersion::put(&mut wb, 1, &user_id, &user_buf);
        StatePtrLatest::put(&mut wb, &user_id, 1);
        store.commit(wb);

        let app2 = router(state);
        let req2 = Request::builder()
            .uri(format!("/api/accounts/{user_id_hex}"))
            .body(Body::empty())
            .unwrap();
        let resp2 = app2.oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let body2 = axum::body::to_bytes(resp2.into_body(), usize::MAX).await.unwrap();
        let json2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
        assert_eq!(json2["exists"], true);
        assert_eq!(json2["balance"], 75000);
        assert_eq!(json2["games_started"], 3);
        assert_eq!(json2["games_won"], 2);
        assert_eq!(json2["games_finished"], 2);
        assert_eq!(json2["lock_hash"], faster_hex::hex_string(&lock_hash));
    }

    #[tokio::test]
    async fn test_api_games_list_detail_and_pagination() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(RocksDbStore::open(dir.path()));
        let state = DaState {
            store: store.clone(),
            covenant_id: [0x11; 32],
            lane_subnet: vec![1, 2, 3, 4],
            network_prefix: "kaspasim".to_string(),
            web_dir: None,
        };

        // Seed canonical chain with version 1.
        let mut manager = store.canonical_chain_manager::<u64>(0);
        manager.append(1u64);

        let creator_id = ResourceId::from([0x10; 32]);
        let creator_id_hex = faster_hex::hex_string(creator_id.as_slice());
        let joiner_id = ResourceId::from([0x20; 32]);
        let joiner_id_hex = faster_hex::hex_string(joiner_id.as_slice());

        // Game 1: Open, stake 10_000, 3 rounds.
        let game1_id = ResourceId::from([0x01; 32]);
        let game1_id_hex = faster_hex::hex_string(game1_id.as_slice());
        let mut game1_buf = vec![0u8; GAME_WIRE_LEN];
        write_game(&mut game1_buf, &creator_id, Cell::X, 10_000, 3).unwrap();

        // Game 2: Open, stake 20_000, 5 rounds.
        let game2_id = ResourceId::from([0x02; 32]);
        let game2_id_hex = faster_hex::hex_string(game2_id.as_slice());
        let mut game2_buf = vec![0u8; GAME_WIRE_LEN];
        write_game(&mut game2_buf, &creator_id, Cell::O, 20_000, 5).unwrap();

        // Game 3: Playing with joiner.
        let game3_id = ResourceId::from([0x03; 32]);
        let game3_id_hex = faster_hex::hex_string(game3_id.as_slice());
        let mut game3_buf = vec![0u8; GAME_WIRE_LEN];
        write_game(&mut game3_buf, &creator_id, Cell::X, 30_000, 3).unwrap();
        {
            let g3 = GameBody::from_bytes_mut(&mut game3_buf).unwrap();
            g3.set_state(State::Playing);
            g3.set_joiner(&joiner_id);
            g3.set_last_move_at(12345);
        }

        let mut wb = store.write_batch();
        // State versions & latest ptrs.
        StateVersion::put(&mut wb, 1, &game1_id, &game1_buf);
        StatePtrLatest::put(&mut wb, &game1_id, 1);
        StateVersion::put(&mut wb, 1, &game2_id, &game2_buf);
        StatePtrLatest::put(&mut wb, &game2_id, 1);
        StateVersion::put(&mut wb, 1, &game3_id, &game3_buf);
        StatePtrLatest::put(&mut wb, &game3_id, 1);

        // Status index entries.
        let k1 = GameStatusKey::new(GameStatus::Open, &game1_id);
        wb.put(StateSpace::Index, k1.as_bytes(), &1u64.to_be_bytes());
        let k2 = GameStatusKey::new(GameStatus::Open, &game2_id);
        wb.put(StateSpace::Index, k2.as_bytes(), &1u64.to_be_bytes());
        let k3 = GameStatusKey::new(GameStatus::Playing, &game3_id);
        wb.put(StateSpace::Index, k3.as_bytes(), &1u64.to_be_bytes());
        store.commit(wb);

        // Detail: Game 1 (Open, no joiner).
        let app = router(state.clone());
        let req = Request::builder()
            .uri(format!("/api/games/{game1_id_hex}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], game1_id_hex);
        assert_eq!(json["state"], 0);
        assert_eq!(json["state_name"], "Open");
        assert_eq!(json["stake"], 10000);
        assert_eq!(json["pot"], 20000);
        assert_eq!(json["rounds_total"], 3);
        assert_eq!(json["creator_mark"], 1);
        assert_eq!(json["round_wins"], serde_json::json!([0, 0]));
        assert_eq!(json["draws"], 0);
        assert_eq!(json["players"], serde_json::json!([creator_id_hex, null]));
        assert_eq!(json["board"], serde_json::json!([0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert_eq!(json["last_move_at"], 0);

        // Detail: Game 3 (Playing with joiner).
        let app = router(state.clone());
        let req = Request::builder()
            .uri(format!("/api/games/{game3_id_hex}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], game3_id_hex);
        assert_eq!(json["state"], 1);
        assert_eq!(json["state_name"], "Playing");
        assert_eq!(json["stake"], 30000);
        assert_eq!(json["pot"], 60000);
        assert_eq!(json["players"], serde_json::json!([creator_id_hex, joiner_id_hex]));
        assert_eq!(json["last_move_at"], 12345);

        // Detail: Absent game -> 404 NOT FOUND.
        let absent_id_hex = faster_hex::hex_string(&[0x99; 32]);
        let app = router(state.clone());
        let req = Request::builder()
            .uri(format!("/api/games/{absent_id_hex}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // List: Open games.
        let app = router(state.clone());
        let req = Request::builder().uri("/api/games?status=open").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let games = json["games"].as_array().unwrap();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0]["id"], game1_id_hex);
        assert_eq!(games[1]["id"], game2_id_hex);
        assert_eq!(games[1]["creator_mark"], 2);

        // Pagination: limit=1.
        let app = router(state.clone());
        let req =
            Request::builder().uri("/api/games?status=open&limit=1").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let games = json["games"].as_array().unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0]["id"], game1_id_hex);

        // Pagination: after=game1 & limit=1.
        let app = router(state.clone());
        let req = Request::builder()
            .uri(format!("/api/games?status=open&after={game1_id_hex}&limit=1"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let games = json["games"].as_array().unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0]["id"], game2_id_hex);

        // Pagination: after=game2 & limit=1 -> empty.
        let app = router(state.clone());
        let req = Request::builder()
            .uri(format!("/api/games?status=open&after={game2_id_hex}&limit=1"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let games = json["games"].as_array().unwrap();
        assert!(games.is_empty());

        // Unknown status -> 400 BAD REQUEST; omitted status still defaults to open.
        let app = router(state.clone());
        let req = Request::builder().uri("/api/games?status=settled").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let app = router(state);
        let req = Request::builder().uri("/api/games").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Runs `GET /api/exits` and returns the served roots array.
    async fn fetch_exit_roots(state: &DaState) -> Vec<serde_json::Value> {
        let app = router(state.clone());
        let req = Request::builder().uri("/api/exits").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        json["roots"].as_array().expect("roots array").clone()
    }

    #[tokio::test]
    async fn test_api_exits_serves_advancing_family() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(RocksDbStore::open(dir.path()));
        let state = DaState {
            store: store.clone(),
            covenant_id: [0x11; 32],
            lane_subnet: vec![1, 2, 3, 4],
            network_prefix: "kaspasim".to_string(),
            web_dir: None,
        };

        // Empty store: no roots.
        assert!(fetch_exit_roots(&state).await.is_empty());

        // A two-leaf family settles; the two claims below drain it leaf by leaf.
        let leaves = vec![
            ExitLeaf::from_pair(StandardSpk::PubKey(&[0x21; 32]), 50_000_000),
            ExitLeaf::from_pair(StandardSpk::PubKey(&[0x22; 32]), 25_000_000),
        ];
        let bundle = ExitsForBundle {
            new_state: [0xaa; 32],
            permission_spk_hash: [0xbb; 32],
            leaves: Arc::new(leaves.clone()),
        };
        let settlement = L1SettlementInfo {
            tx_id: TransactionId::from_bytes([0xcc; 32]),
            daa_score: U64::new(100),
            ..Default::default()
        };
        let indexer = TicTacToeExitIndexer::default();
        let mut wb = store.write_batch();
        indexer.on_exits_committed(&bundle, &settlement, &mut wb);
        store.commit(wb);

        let tree = PermissionTreeView::from_leaves(&leaves);
        let empty = PermissionTreeAccumulator::hash_empty();
        let root0 = tree.root();
        let root1 = tree.root_with_leaf(0, empty);
        let root2 = PermissionTreeAccumulator::hash_branch(&empty, &empty);
        let hex = |b: &[u8; 32]| faster_hex::hex_string(b);

        // Fresh family: the served root is the raw padded root, every leaf unspent.
        let roots = fetch_exit_roots(&state).await;
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0]["root"], hex(&root0));
        assert_eq!(roots[0]["settlement_txid"], hex(&[0xcc; 32]));
        assert_eq!(roots[0]["outpoint_index"], 1);
        assert_eq!(roots[0]["unclaimed"], 2);
        let fresh = &roots[0]["leaves"];
        assert!(fresh[0]["spent"].is_null());
        assert_eq!(fresh[0]["siblings"], serde_json::json!([hex(&tree.siblings(0)[0])]));
        assert_eq!(
            fresh[0]["full_claim"],
            serde_json::json!({"new_root": hex(&root1), "new_unclaimed": 1})
        );
        assert!(fresh[1]["spent"].is_null());
        assert_eq!(
            fresh[1]["full_claim"],
            serde_json::json!({"new_root": hex(&tree.root_with_leaf(1, empty)), "new_unclaimed": 1})
        );

        // First claim: the family advances onto the claim's continuation output.
        let spend1 = PermissionSpend {
            covenant_id: [0x11; 32],
            old_root: root0,
            old_unclaimed: 2,
            depth: tree.depth(),
            leaf_index: 0,
            leaf_spk_bytes: leaves[0].script_bytes().to_vec(),
            leaf_amount: 50_000_000,
            deduct: 50_000_000,
            new_root: root1,
            spend_txid: [0xee; 32],
            new_outpoint_index: 1,
            chain_idx: 10,
        };
        let mut wb = store.write_batch();
        indexer.on_permission_spent(&spend1, &mut wb);
        store.commit(wb);

        let roots = fetch_exit_roots(&state).await;
        assert_eq!(roots.len(), 1, "the re-keyed family replaces the spent root");
        assert_eq!(roots[0]["root"], hex(&root1));
        assert_eq!(roots[0]["settlement_txid"], hex(&[0xee; 32]), "serves the claim txid");
        assert_eq!(roots[0]["outpoint_index"], 1);
        assert_eq!(roots[0]["unclaimed"], 1);
        let advanced = &roots[0]["leaves"];
        assert_eq!(
            advanced[0]["spent"],
            serde_json::json!({"spend_txid": hex(&[0xee; 32]), "deduct": 50_000_000})
        );
        assert!(advanced[1]["spent"].is_null());
        // The emptied slot folds to the empty hash: leaf 1's sibling and both post-claim roots
        // come from the folded tree, not the padded leaf hashes.
        assert_eq!(advanced[1]["siblings"], serde_json::json!([hex(&empty)]));
        assert_eq!(
            advanced[1]["full_claim"],
            serde_json::json!({"new_root": hex(&root2), "new_unclaimed": 0})
        );

        // Second claim drains the family: the record keeps its key with unclaimed 0.
        let spend2 = PermissionSpend {
            covenant_id: [0x11; 32],
            old_root: root1,
            old_unclaimed: 1,
            depth: tree.depth(),
            leaf_index: 1,
            leaf_spk_bytes: leaves[1].script_bytes().to_vec(),
            leaf_amount: 25_000_000,
            deduct: 25_000_000,
            new_root: root2,
            spend_txid: [0xef; 32],
            new_outpoint_index: 1,
            chain_idx: 11,
        };
        let mut wb = store.write_batch();
        indexer.on_permission_spent(&spend2, &mut wb);
        store.commit(wb);

        let roots = fetch_exit_roots(&state).await;
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0]["root"], hex(&root1), "a drained family keeps its key");
        assert_eq!(roots[0]["unclaimed"], 0);
        assert_eq!(
            roots[0]["leaves"][1]["spent"],
            serde_json::json!({"spend_txid": hex(&[0xef; 32]), "deduct": 25_000_000})
        );
    }

    #[tokio::test]
    async fn test_api_state_settled_from_latest_settlement_record() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(RocksDbStore::open(dir.path()));
        let state = DaState {
            store: store.clone(),
            covenant_id: [0x33; 32],
            lane_subnet: vec![1, 2, 3, 4],
            network_prefix: "kaspasim".to_string(),
            web_dir: None,
        };

        // Drive the exit indexer handler exactly as the runner does.
        let bundle = ExitsForBundle {
            new_state: [0xaa; 32],
            permission_spk_hash: [0xbb; 32],
            leaves: Arc::new(vec![ExitLeaf::from_pair(
                StandardSpk::PubKey(&[0x12; 32]),
                50_000_000,
            )]),
        };
        let settlement = L1SettlementInfo {
            tx_id: TransactionId::from_bytes([0xcc; 32]),
            daa_score: U64::new(789_101),
            ..Default::default()
        };
        let mut wb = store.write_batch();
        TicTacToeExitIndexer::default().on_exits_committed(&bundle, &settlement, &mut wb);
        store.commit(wb);

        let app = router(state);
        let req = Request::builder().uri("/api/state").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json["settled"],
            serde_json::json!({
                "state_root": faster_hex::hex_string(&[0xaa; 32]),
                "permission_root": faster_hex::hex_string(&[0xbb; 32]),
                "txid": faster_hex::hex_string(&[0xcc; 32]),
                "daa_score": 789_101,
            })
        );
    }
}
