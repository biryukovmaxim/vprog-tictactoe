//! Environment configuration for the tic-tac-toe node daemon (`ttd`).

use std::{path::PathBuf, str::FromStr};

use kaspa_consensus_core::network::NetworkId;
use kaspa_hashes::Hash;
use secp256k1::SecretKey;
use vprogs_runner::{RunnerConfig, StartMode};

/// Parsed node configuration containing the runner configuration.
pub struct Config {
    /// The engine configuration handed to the runner.
    pub runner: RunnerConfig,
    /// The bind address for the DA HTTP server.
    pub da_bind: String,
    /// Optional directory holding static web frontend assets to serve.
    pub web_dir: Option<String>,
}

impl Config {
    /// Reads configuration from the process environment, panicking with a clear message on
    /// missing or malformed required values.
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok().filter(|s| !s.is_empty()))
    }

    /// Reads configuration from a variable lookup closure.
    pub fn from_lookup<F>(lookup: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let wrpc_url = req(&lookup, "TT_WRPC_URL");
        let private_key = opt(&lookup, "TT_PRIVATE_KEY").map(|s| {
            SecretKey::from_str(s.trim())
                .expect("TT_PRIVATE_KEY must be a 32-byte hex secp256k1 key")
        });
        let network_id = opt(&lookup, "TT_NETWORK")
            .map(|s| parse_network(&s))
            .unwrap_or_else(|| parse_network("tn10"));
        let program_elf = Some(PathBuf::from(
            opt(&lookup, "TT_PROGRAM_ELF").unwrap_or_else(|| "guest/compiled/program.elf".into()),
        ));
        let batch_elf = Some(PathBuf::from(req(&lookup, "TT_BATCH_ELF")));
        let aggregator_elf = Some(PathBuf::from(req(&lookup, "TT_AGGREGATOR_ELF")));
        let data_dir =
            PathBuf::from(opt(&lookup, "TT_DATA_DIR").unwrap_or_else(|| "./ttd-data".into()));
        let lane_id =
            opt(&lookup, "TT_LANE_ID").map(|s| s.parse().expect("TT_LANE_ID must be a u32"));
        let covenant_id = opt(&lookup, "TT_COVENANT_ID")
            .map(|s| Hash::from_str(s.trim()).expect("TT_COVENANT_ID must be 32-byte hex"));
        let bootstrap_txid = opt(&lookup, "TT_BOOTSTRAP_TXID")
            .map(|s| Hash::from_str(s.trim()).expect("TT_BOOTSTRAP_TXID must be 32-byte hex"));
        let start_from = opt(&lookup, "TT_START_FROM")
            .map(|s| Hash::from_str(s.trim()).expect("TT_START_FROM must be 32-byte hex"));
        let seed_depth = opt_u64(&lookup, "TT_SEED_DEPTH", 500);
        let min_confirmations = opt(&lookup, "TT_MIN_CONFIRMATIONS")
            .map(|s| s.parse().expect("TT_MIN_CONFIRMATIONS must be a u64"));
        let prove = opt(&lookup, "TT_PROVE").is_some_and(|s| s != "0");
        let start_mode = opt(&lookup, "TT_START_MODE")
            .map(|s| match s.to_lowercase().as_str() {
                "fresh" => StartMode::Fresh,
                "resume" => StartMode::Resume,
                "catchup" => StartMode::Catchup,
                _ => panic!("TT_START_MODE must be fresh, resume, or catchup"),
            })
            .or_else(|| covenant_id.map(|_| StartMode::Catchup));

        let runner = RunnerConfig {
            wrpc_url,
            private_key,
            network_id,
            program_elf,
            batch_elf,
            aggregator_elf,
            data_dir,
            lane_id,
            covenant_id,
            bootstrap_txid,
            start_from,
            seed_depth,
            min_confirmations,
            prove,
            start_mode,
        };

        let da_bind = opt(&lookup, "TT_DA_BIND").unwrap_or_else(|| "127.0.0.1:9880".into());
        let web_dir = opt(&lookup, "TT_WEB_DIR");

        Self { runner, da_bind, web_dir }
    }
}

/// Parses network string, delegating to the runner's parser.
fn parse_network(raw: &str) -> NetworkId {
    vprogs_runner::parse_network(raw)
        .unwrap_or_else(|e| panic!("TT_NETWORK={raw:?} unrecognized: {e}"))
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
        m.insert(
            "TT_PRIVATE_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001".into(),
        );
        m.insert("TT_BATCH_ELF", "/tmp/batch.elf".into());
        m.insert("TT_AGGREGATOR_ELF", "/tmp/aggregator.elf".into());
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
    fn from_lookup_allows_missing_private_key() {
        let mut env = sample_env();
        env.remove("TT_PRIVATE_KEY");
        let cfg = Config::from_lookup(|k| env.get(k).cloned());
        assert!(cfg.runner.private_key.is_none());
    }

    #[test]
    #[should_panic(expected = "TT_PRIVATE_KEY must be a 32-byte hex secp256k1 key")]
    fn from_lookup_panics_on_malformed_private_key() {
        let mut env = sample_env();
        env.insert("TT_PRIVATE_KEY", "not-a-valid-hex-key".into());
        Config::from_lookup(|k| env.get(k).cloned());
    }

    #[test]
    #[should_panic(expected = "missing required env var TT_BATCH_ELF")]
    fn from_lookup_panics_on_missing_batch_elf() {
        let mut env = sample_env();
        env.remove("TT_BATCH_ELF");
        Config::from_lookup(|k| env.get(k).cloned());
    }

    #[test]
    #[should_panic(expected = "missing required env var TT_AGGREGATOR_ELF")]
    fn from_lookup_panics_on_missing_aggregator_elf() {
        let mut env = sample_env();
        env.remove("TT_AGGREGATOR_ELF");
        Config::from_lookup(|k| env.get(k).cloned());
    }

    #[test]
    fn from_lookup_populates_defaults() {
        let env = sample_env();
        let cfg = Config::from_lookup(|k| env.get(k).cloned());
        assert_eq!(cfg.runner.wrpc_url, "ws://127.0.0.1:17210");
        assert_eq!(cfg.runner.program_elf, Some(PathBuf::from("guest/compiled/program.elf")));
        assert_eq!(cfg.runner.batch_elf, Some(PathBuf::from("/tmp/batch.elf")));
        assert_eq!(cfg.runner.aggregator_elf, Some(PathBuf::from("/tmp/aggregator.elf")));
        assert_eq!(cfg.runner.data_dir, PathBuf::from("./ttd-data"));
        assert_eq!(cfg.runner.seed_depth, 500);
        assert!(!cfg.runner.prove);
        assert_eq!(cfg.runner.network_id, parse_network("tn10"));
        assert!(cfg.runner.start_mode.is_none());
        assert_eq!(cfg.da_bind, "127.0.0.1:9880");
        assert_eq!(cfg.web_dir, None);
    }

    #[test]
    fn from_lookup_reads_da_bind_and_web_dir() {
        let mut env = sample_env();
        env.insert("TT_DA_BIND", "0.0.0.0:8080".into());
        env.insert("TT_WEB_DIR", "/var/www".into());
        let cfg = Config::from_lookup(|k| env.get(k).cloned());
        assert_eq!(cfg.da_bind, "0.0.0.0:8080");
        assert_eq!(cfg.web_dir, Some("/var/www".into()));
    }

    #[test]
    fn from_lookup_auto_selects_catchup_mode_on_covenant_id() {
        let mut env = sample_env();
        env.insert(
            "TT_COVENANT_ID",
            "1111111111111111111111111111111111111111111111111111111111111111".into(),
        );
        let cfg = Config::from_lookup(|k| env.get(k).cloned());
        assert_eq!(cfg.runner.start_mode, Some(StartMode::Catchup));
    }
}
