//! Core transaction handler, wrapped by `main` into the ABI `TransactionHandler`
//! shape passed to `process_transaction`.

use vprogs_zk_abi::{
    Error as AbiError, Result as AbiResult,
    transaction_processor::{MergesetContext, Resource, Transaction},
    withdrawal::{DepositSink, ExitSink},
};
use vprogs_zk_backend_risc0_runtime_processor::{
    deposit_policy::DepositPolicy,
    lifecycle::Lifecycle,
    signer_trait::{Signer, SignerResolveContext},
};

use crate::{
    program::action::{self, apply_action},
    runtime::{
        auth_context::{AuthContext, MultisigUnlocker},
        ix::{DecodedIx, decode_ix},
        lock::LockEnum,
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
/// `Deposit` action; `merge_idx` is unused. `context` is the chain-block clock, exposed to
/// time-dependent actions through [`ApplyContext`].
///
/// Generic over `P: DepositPolicy` so the program can supply its own deposit rules in
/// `main.rs` without touching this file.
///
/// [`TransactionHandler`]: vprogs_zk_abi::transaction_processor::TransactionHandler
pub fn run<'a, P: DepositPolicy<Lock<'a> = LockEnum<'a>>>(
    tx: &Transaction<'a>,
    context: &MergesetContext,
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

    // Signers resolve lazily: each is attempted up front and still-pending ones are retried
    // before every action, so a signer over a resource this tx creates (a `[Deposit +
    // CreateGame]` entry carrier signing as the user the deposit births) resolves once the
    // creating action has run.
    let mut pending: alloc::vec::Vec<&(u8, SignerEnum)> = signers.iter().collect();
    let mut auth_ctx = AuthContext::default();
    resolve_pending(
        &mut pending,
        &mut auth_ctx,
        &SignerResolveContext::new(payload.bytes, current_rest_preimage, payload_presig, resources),
        resources,
    )?;

    // Per-tx apply state carried across the per-action contexts below.
    let mut lifecycle: alloc::vec::Vec<Lifecycle> =
        resources.iter().map(Lifecycle::from_resource).collect();
    let mut consumed_outputs = alloc::vec::Vec::new();

    for action in &actions {
        if !pending.is_empty() {
            resolve_pending(
                &mut pending,
                &mut auth_ctx,
                &SignerResolveContext::new(
                    payload.bytes,
                    current_rest_preimage,
                    payload_presig,
                    resources,
                ),
                resources,
            )?;
        }
        let mut cx = crate::runtime::ApplyContext {
            tx,
            resources,
            context,
            auth_ctx: &auth_ctx,
            exits,
            deposit,
            lifecycle: core::mem::take(&mut lifecycle),
            consumed_outputs: core::mem::take(&mut consumed_outputs),
        };
        apply_action(action, &mut cx, policy)?;
        lifecycle = core::mem::take(&mut cx.lifecycle);
        consumed_outputs = core::mem::take(&mut cx.consumed_outputs);
    }

    if !pending.is_empty() {
        // A signer whose resource never went live: the same verdict eager resolution gives.
        return Err(AbiError::Decode("signer: unknown or absent resource kind".into()));
    }

    Ok(())
}

/// Resolves every pending signer whose target resource is already live, keeping the rest
/// pending. Sig-pointer variants read the resource's lock, so an absent (or not-yet-born)
/// target defers; the genesis and prev-tx witness variants carry their own authority and
/// resolve immediately.
fn resolve_pending(
    pending: &mut alloc::vec::Vec<&(u8, SignerEnum)>,
    auth: &mut AuthContext,
    ctx: &SignerResolveContext<'_>,
    resources: &[Resource<'_>],
) -> AbiResult<()> {
    let mut still = alloc::vec::Vec::new();
    for entry in pending.drain(..) {
        let (resource_idx, signer) = (entry.0, &entry.1);
        let gated =
            matches!(signer, SignerEnum::SchnorrSigPtr(_) | SignerEnum::MultisigSchnorrSigPtr(_));
        if gated && !lock_bearing(resources, resource_idx) {
            still.push(entry);
            continue;
        }
        resolve_one(signer, resource_idx, auth, ctx)?;
    }
    *pending = still;
    // Lazy rounds can append out of wire order; the unlock matchers expect buckets ordered
    // by resource index.
    auth.schnorr.sort_unstable_by_key(|(idx, _)| *idx);
    auth.multisig.sort_unstable_by_key(|(idx, _)| *idx);
    Ok(())
}

/// Whether `resource_idx` holds `User` or `Config` data, the lock-bearing kinds a sig-pointer
/// signer can resolve against.
fn lock_bearing(resources: &[Resource<'_>], resource_idx: u8) -> bool {
    use crate::program::resources::kind::Kind;
    let Some(data) = resources.get(resource_idx as usize).map(|r| r.data()) else {
        return false;
    };
    let kind = data.first().copied().and_then(|b| Kind::try_from(b).ok());
    matches!(kind, Some(Kind::User) | Some(Kind::Config))
}

/// Resolves one signer and routes the produced unlocker into the matching `AuthContext`
/// bucket. Each signer kind is statically tied to one bucket via its `Signer::Unlocker`.
fn resolve_one(
    signer: &SignerEnum,
    resource_idx: u8,
    auth: &mut AuthContext,
    ctx: &SignerResolveContext<'_>,
) -> AbiResult<()> {
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
    Ok(())
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
    match bucket.last_mut() {
        Some((last_idx, last)) if *last_idx == resource_idx => {
            last.pubkeys.extend(contrib.pubkeys);
            return;
        }
        _ => {}
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
