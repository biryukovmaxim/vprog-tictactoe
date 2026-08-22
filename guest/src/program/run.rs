//! Core transaction handler, wrapped by `main` into the ABI `TransactionHandler`
//! shape passed to `process_transaction`.

use vprogs_zk_abi::{
    Result as AbiResult,
    transaction_processor::{Resource, Transaction},
    withdrawal::{DepositSink, ExitSink},
};
use vprogs_zk_backend_risc0_runtime_processor::{
    auth_context::{AuthContext, MultisigUnlocker},
    signer_trait::{Signer, SignerResolveContext},
};

use crate::{
    program::{
        action::{self, ApplyContext, apply_action},
        deposit_policy::DepositPolicy,
    },
    runtime::{
        ix::{DecodedIx, decode_ix},
        signer::{
            GenesisSchnorrSigPtrSigner, MultisigPrevTxV1WitnessSigner, MultisigSchnorrSigPtrSigner,
            PrevTxV1WitnessSigner, SchnorrSigPtrSigner, SignerEnum,
        },
    },
};

/// Verifies signers against resource locks and applies the decoded actions.
///
/// `main` adapts this into the ABI [`TransactionHandler`] shape. `exits` receives L2-to-L1 exits
/// emitted by `Withdraw` actions; `deposit` receives the deposit-address commitment written by a
/// `Deposit` action; `merge_idx` and `context_hash` are unused.
///
/// Generic over `P: DepositPolicy` so the program can supply its own deposit rules in
/// `main.rs` without touching this file.
///
/// [`TransactionHandler`]: vprogs_zk_abi::transaction_processor::TransactionHandler
pub fn run<'a, P: DepositPolicy>(
    tx: &Transaction<'a>,
    resources: &mut [Resource<'a>],
    exits: &mut ExitSink,
    deposit: &mut DepositSink,
    policy: &P,
) -> AbiResult<()> {
    // The witness format is V1-specific; the ABI only decodes V1 transactions.
    let current_rest_preimage = tx.rest_preimage;
    let payload = &tx.payload;
    let DecodedIx { signers, actions, end_of_actions_in_ix } =
        decode_ix(payload.ix_data, resources.len(), action::decode_action)?;

    let access_prefix_len = payload.bytes.len() - payload.ix_data.len();
    let end_of_actions_in_payload = access_prefix_len + end_of_actions_in_ix;
    let payload_presig = &payload.bytes[..end_of_actions_in_payload];

    // Resolve signers -> AuthContext
    let auth_ctx = resolve_signers(
        &signers,
        &SignerResolveContext::new(payload.bytes, current_rest_preimage, payload_presig, resources),
    )?;

    let mut cx = ApplyContext::new(tx, resources, &auth_ctx, exits, deposit);

    for action in &actions {
        apply_action(action, &mut cx, policy)?;
    }

    Ok(())
}

/// Walks parsed signers, calls `Signer::resolve` on each, and routes the
/// produced unlocker into the matching `AuthContext` bucket. Each signer
/// kind is statically tied to one bucket via its `Signer::Unlocker`.
fn resolve_signers<'a>(
    signers: &[(u8, SignerEnum)],
    ctx: &SignerResolveContext<'a>,
) -> AbiResult<AuthContext> {
    let mut auth = AuthContext::default();

    for (resource_idx, signer) in signers {
        let resource_idx = *resource_idx;
        match signer {
            SignerEnum::SchnorrSigPtr(s) => {
                let u = SchnorrSigPtrSigner::resolve(s, resource_idx, ctx)?;
                auth.schnorr.push((resource_idx, u));
            }
            SignerEnum::GenesisSchnorrSigPtr(s) => {
                let u = GenesisSchnorrSigPtrSigner::resolve(s, resource_idx, ctx)?;
                auth.schnorr.push((resource_idx, u));
            }
            SignerEnum::PrevTxV1Witness(s) => {
                let u = PrevTxV1WitnessSigner::resolve(s, resource_idx, ctx)?;
                auth.schnorr.push((resource_idx, u));
            }
            SignerEnum::MultisigSchnorrSigPtr(s) => {
                let u = MultisigSchnorrSigPtrSigner::resolve(s, resource_idx, ctx)?;
                append_multisig_contrib(&mut auth.multisig, resource_idx, u);
            }
            SignerEnum::MultisigPrevTxV1Witness(s) => {
                let u = MultisigPrevTxV1WitnessSigner::resolve(s, resource_idx, ctx)?;
                append_multisig_contrib(&mut auth.multisig, resource_idx, u);
            }
        }
    }

    Ok(auth)
}

/// Appends `contrib`'s pubkeys into the multisig bucket entry for
/// `resource_idx`, creating the entry if absent. Wire signers are sorted by
/// `resource_idx` (decode_ix invariant), so the target entry, if present,
/// is always the last one in the bucket; this avoids any map lookup.
pub(crate) fn append_multisig_contrib(
    bucket: &mut alloc::vec::Vec<(u8, MultisigUnlocker)>,
    resource_idx: u8,
    contrib: MultisigUnlocker,
) {
    if let Some((last_idx, last)) = bucket.last_mut() {
        if *last_idx == resource_idx {
            last.pubkeys.extend(contrib.pubkeys);
            return;
        }
    }
    bucket.push((resource_idx, contrib));
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;

    #[test]
    fn multisig_aggregator_appends_to_existing_entry_for_same_resource() {
        let mut bucket = Vec::new();
        append_multisig_contrib(
            &mut bucket,
            0,
            MultisigUnlocker { pubkeys: alloc::vec![[0x01u8; 32]] },
        );
        append_multisig_contrib(
            &mut bucket,
            0,
            MultisigUnlocker { pubkeys: alloc::vec![[0x02u8; 32]] },
        );
        assert_eq!(bucket.len(), 1);
        assert_eq!(bucket[0].0, 0);
        assert_eq!(bucket[0].1.pubkeys, alloc::vec![[0x01u8; 32], [0x02u8; 32]]);
    }

    #[test]
    fn multisig_aggregator_creates_new_entry_for_different_resource() {
        let mut bucket = Vec::new();
        append_multisig_contrib(
            &mut bucket,
            0,
            MultisigUnlocker { pubkeys: alloc::vec![[0x01u8; 32]] },
        );
        append_multisig_contrib(
            &mut bucket,
            1,
            MultisigUnlocker { pubkeys: alloc::vec![[0x02u8; 32]] },
        );
        assert_eq!(bucket.len(), 2);
        assert_eq!(bucket[0].0, 0);
        assert_eq!(bucket[1].0, 1);
    }

    #[test]
    fn multisig_aggregator_preserves_wire_order_within_resource() {
        // Order of pubkeys in the aggregated list mirrors the order of
        // append calls; no sort. Matchers reject if not strict-asc.
        let mut bucket = Vec::new();
        for pk in [[0x03u8; 32], [0x01u8; 32], [0x02u8; 32]] {
            append_multisig_contrib(&mut bucket, 7, MultisigUnlocker { pubkeys: alloc::vec![pk] });
        }
        assert_eq!(bucket.len(), 1);
        assert_eq!(bucket[0].1.pubkeys, alloc::vec![[0x03u8; 32], [0x01u8; 32], [0x02u8; 32]]);
    }
}
