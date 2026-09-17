//! Environment configuration for the tic-tac-toe scenario driver (`ttflow`).

use std::{str::FromStr, time::Duration};

use kaspa_consensus_core::network::{NetworkId, NetworkType};
use kaspa_hashes::Hash;
use secp256k1::SecretKey;
use vprog_tictactoe_guest::runtime::genesis::GENESIS_PUBKEY;
use vprogs_zk_backend_risc0_app_kit::dev_genesis_keypair;

/// Parsed driver configuration for executing the scripted game scenario.
pub struct Config {
    /// WebSocket RPC URL of the Kaspa node.
    pub wrpc_url: String,
    /// Network identifier (e.g. `tn10`, `simnet`, `mainnet`).
    pub network_id: NetworkId,
    /// Target execution lane identifier.
    pub lane_id: u32,
    /// 32-byte covenant identifier for deposit outputs.
    pub covenant_id: Hash,
    /// Secret key of the funding operator wallet.
    pub private_key: SecretKey,
    /// Secret key for genesis `Init` authentication.
    pub genesis_key: SecretKey,
    /// Stake per player for created games in sompis.
    pub stake: u64,
    /// Total rounds per match.
    pub rounds: u8,
    /// Funding amount per player deposit in sompis.
    pub deposit_amount: u64,
    /// Delay between scenario steps.
    pub step_delay: Duration,
    /// Turn time-to-live written into config `Init`, in DAA-score units.
    pub turn_ttl: u64,
    /// In-rollup sompis transferred from player A to player B mid-scenario (0 skips the step).
    pub transfer_amount: u64,
    /// Fixed player-A secret key, letting the run's exit-leaf owner be known in advance;
    /// random when unset.
    pub player_a_key: Option<SecretKey>,
}

impl Config {
    /// Reads configuration from the process environment, panicking on missing or malformed required
    /// values.
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok().filter(|s| !s.is_empty()))
    }

    /// Reads configuration from a variable lookup closure.
    pub fn from_lookup<F>(lookup: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let wrpc_url = req(&lookup, "TT_WRPC_URL");
        let network_id = opt(&lookup, "TT_NETWORK")
            .map(|s| parse_network(&s))
            .unwrap_or_else(|| parse_network("tn10"));
        let lane_id = req(&lookup, "TT_LANE_ID").parse().expect("TT_LANE_ID must be a valid u32");
        let covenant_id = Hash::from_str(req(&lookup, "TT_COVENANT_ID").trim())
            .expect("TT_COVENANT_ID must be 32-byte hex");
        let private_key = {
            let hex = req(&lookup, "TTFLOW_PRIVATE_KEY");
            SecretKey::from_str(hex.trim())
                .expect("TTFLOW_PRIVATE_KEY must be a 32-byte hex secp256k1 key")
        };
        let genesis_key = opt(&lookup, "TTFLOW_GENESIS_KEY")
            .map(|hex| {
                SecretKey::from_str(hex.trim())
                    .expect("TTFLOW_GENESIS_KEY must be a 32-byte hex secp256k1 key")
            })
            .unwrap_or_else(|| dev_genesis_keypair(&GENESIS_PUBKEY).secret_key());

        let stake = opt_u64(&lookup, "TTFLOW_STAKE", 50_000_000);
        let rounds = opt_u64(&lookup, "TTFLOW_ROUNDS", 1) as u8;
        let deposit_amount = opt_u64(&lookup, "TTFLOW_DEPOSIT_AMOUNT", 100_000_000);
        let step_delay = Duration::from_millis(opt_u64(&lookup, "TTFLOW_STEP_DELAY_MS", 2000));
        let turn_ttl = opt_u64(&lookup, "TTFLOW_TURN_TTL", 10_000);
        let transfer_amount = opt_u64(&lookup, "TTFLOW_TRANSFER_AMOUNT", 0);
        let player_a_key = opt(&lookup, "TTFLOW_PLAYER_A_KEY").map(|hex| {
            SecretKey::from_str(hex.trim())
                .expect("TTFLOW_PLAYER_A_KEY must be a 32-byte hex secp256k1 key")
        });

        Self {
            wrpc_url,
            network_id,
            lane_id,
            covenant_id,
            private_key,
            genesis_key,
            stake,
            rounds,
            deposit_amount,
            step_delay,
            turn_ttl,
            transfer_amount,
            player_a_key,
        }
    }
}

/// Parses network string into a Kaspa [`NetworkId`].
pub fn parse_network(raw: &str) -> NetworkId {
    let v = raw.trim().to_lowercase();
    match v.as_str() {
        "testnet-10" | "testnet10" | "tn10" => NetworkId::with_suffix(NetworkType::Testnet, 10),
        "mainnet" => NetworkId::new(NetworkType::Mainnet),
        "devnet" => NetworkId::new(NetworkType::Devnet),
        "simnet" => NetworkId::new(NetworkType::Simnet),
        _ => match v.strip_prefix("testnet-").and_then(|n| n.parse::<u32>().ok()) {
            Some(n) => NetworkId::with_suffix(NetworkType::Testnet, n),
            None => panic!("TT_NETWORK={raw:?} unrecognized network identifier"),
        },
    }
}

/// Reads a required environment variable, panicking if missing or empty.
fn req<F>(lookup: &F, key: &str) -> String
where
    F: Fn(&str) -> Option<String>,
{
    lookup(key).unwrap_or_else(|| panic!("missing required env var {key}"))
}

/// Reads an optional environment variable.
fn opt<F>(lookup: &F, key: &str) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    lookup(key)
}

/// Reads an optional `u64` environment variable with a default fallback.
fn opt_u64<F>(lookup: &F, key: &str, default: u64) -> u64
where
    F: Fn(&str) -> Option<String>,
{
    opt(lookup, key)
        .map(|s| s.parse().unwrap_or_else(|_| panic!("{key} must be a u64")))
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn sample_env() -> HashMap<&'static str, String> {
        let mut m = HashMap::new();
        m.insert("TT_WRPC_URL", "ws://127.0.0.1:17210".into());
        m.insert("TT_LANE_ID", "1".into());
        m.insert(
            "TT_COVENANT_ID",
            "1111111111111111111111111111111111111111111111111111111111111111".into(),
        );
        m.insert(
            "TTFLOW_PRIVATE_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001".into(),
        );
        m
    }

    #[test]
    #[should_panic(expected = "missing required env var TT_WRPC_URL")]
    fn from_lookup_panics_on_missing_wrpc_url() {
        let mut env = sample_env();
        env.remove("TT_WRPC_URL");
        Config::from_lookup(|k| env.get(k).cloned());
    }

    #[test]
    #[should_panic(expected = "missing required env var TT_LANE_ID")]
    fn from_lookup_panics_on_missing_lane_id() {
        let mut env = sample_env();
        env.remove("TT_LANE_ID");
        Config::from_lookup(|k| env.get(k).cloned());
    }

    #[test]
    #[should_panic(expected = "missing required env var TT_COVENANT_ID")]
    fn from_lookup_panics_on_missing_covenant_id() {
        let mut env = sample_env();
        env.remove("TT_COVENANT_ID");
        Config::from_lookup(|k| env.get(k).cloned());
    }

    #[test]
    #[should_panic(expected = "missing required env var TTFLOW_PRIVATE_KEY")]
    fn from_lookup_panics_on_missing_private_key() {
        let mut env = sample_env();
        env.remove("TTFLOW_PRIVATE_KEY");
        Config::from_lookup(|k| env.get(k).cloned());
    }

    #[test]
    fn from_lookup_populates_defaults() {
        let env = sample_env();
        let cfg = Config::from_lookup(|k| env.get(k).cloned());
        assert_eq!(cfg.wrpc_url, "ws://127.0.0.1:17210");
        assert_eq!(cfg.lane_id, 1);
        assert_eq!(
            cfg.covenant_id,
            Hash::from_str("1111111111111111111111111111111111111111111111111111111111111111")
                .unwrap()
        );
        assert_eq!(cfg.network_id, parse_network("tn10"));
        assert_eq!(cfg.stake, 50_000_000);
        assert_eq!(cfg.rounds, 1);
        assert_eq!(cfg.deposit_amount, 100_000_000);
        assert_eq!(cfg.step_delay, Duration::from_millis(2000));
        assert_eq!(cfg.turn_ttl, 10_000);
        assert_eq!(cfg.transfer_amount, 0);
        assert!(cfg.player_a_key.is_none());
        assert_eq!(cfg.genesis_key, dev_genesis_keypair(&GENESIS_PUBKEY).secret_key());
    }

    #[test]
    fn from_lookup_accepts_overrides() {
        let mut env = sample_env();
        env.insert("TT_NETWORK", "simnet".into());
        env.insert("TTFLOW_STAKE", "25000000".into());
        env.insert("TTFLOW_ROUNDS", "3".into());
        env.insert("TTFLOW_DEPOSIT_AMOUNT", "80000000".into());
        env.insert("TTFLOW_STEP_DELAY_MS", "500".into());
        env.insert("TTFLOW_TURN_TTL", "50000".into());
        env.insert("TTFLOW_TRANSFER_AMOUNT", "25000000".into());
        env.insert(
            "TTFLOW_PLAYER_A_KEY",
            "0000000000000000000000000000000000000000000000000000000000000005".into(),
        );
        env.insert(
            "TTFLOW_GENESIS_KEY",
            "0000000000000000000000000000000000000000000000000000000000000004".into(),
        );

        let cfg = Config::from_lookup(|k| env.get(k).cloned());
        assert_eq!(cfg.network_id, parse_network("simnet"));
        assert_eq!(cfg.stake, 25_000_000);
        assert_eq!(cfg.rounds, 3);
        assert_eq!(cfg.deposit_amount, 80_000_000);
        assert_eq!(cfg.step_delay, Duration::from_millis(500));
        assert_eq!(cfg.turn_ttl, 50_000);
        assert_eq!(cfg.transfer_amount, 25_000_000);
        assert_eq!(
            cfg.player_a_key,
            Some(
                SecretKey::from_str(
                    "0000000000000000000000000000000000000000000000000000000000000005"
                )
                .unwrap()
            )
        );
        assert_eq!(
            cfg.genesis_key,
            SecretKey::from_str("0000000000000000000000000000000000000000000000000000000000000004")
                .unwrap()
        );
    }
}
