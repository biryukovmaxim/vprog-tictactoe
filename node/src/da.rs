//! DA HTTP server for ttd.
//!
//! Exposes node state and configuration endpoints for the web frontend.

use std::sync::Arc;

use axum::{Json, Router, extract::State, routing::get};
use kaspa_addresses::{Address, Prefix, Version};
use serde::{Deserialize, Serialize};
use tower_http::{cors::CorsLayer, services::ServeDir};
use vprog_tictactoe_guest::program::resources::{config::ConfigBody, id::config_resource_id};
use vprogs_core_types::ResourceId;
use vprogs_state_ptr_latest::StatePtrLatest;
use vprogs_state_version::StateVersion;
use vprogs_storage_rocksdb_store::RocksDbStore;
use vprogs_storage_types::{ReadStore, Store};
use vprogs_zk_backend_risc0_api::delegate_entry_spk_hash;

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
    /// Confirmation count on L1.
    pub confirmations: u64,
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

    // TODO(task-4): wire settlement source once exposed on RunnerHandles
    let settled: Option<SettlementInfo> = None;

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

/// Constructs the DA server router.
pub fn router(state: DaState) -> Router {
    let web_dir = state.web_dir.clone();
    let api = Router::new()
        .route("/api/state", get(get_state))
        .route("/api/config", get(get_config))
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
        program::resources::config::{config_total_len, write_config},
        runtime::lock::{LockEnum, UnlockedLockView},
    };

    use super::*;

    #[tokio::test]
    async fn test_api_config_uninitialized_then_initialized() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(RocksDbStore::open(dir.path()));
        let (_settlement_tx, _settlement_rx) = tokio::sync::watch::channel(None::<()>);
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
        let (_settlement_tx, _settlement_rx) = tokio::sync::watch::channel(None::<()>);
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
}
