//! Action-body encoders and sorted resource-set helpers for host issuers.
//!
//! Encodes action variants to match the on-wire format expected by [`super::decode_action`].
//! Every encoder writes `ActionTag as u8` followed by the variant's payload fields in exact
//! decode order.

use alloc::vec::Vec;

use vprogs_core_types::{AccessMetadata, ResourceId};
use vprogs_zk_abi::withdrawal::StandardSpk;
use vprogs_zk_backend_risc0_runtime_processor::signer_trait::Signer;

use crate::{
    program::{action::ActionTag, resources::game::Cell},
    runtime::{lock::LockEnum, signer_variants::GenesisSchnorrSigPtrSigner},
};

/// Tag discriminant for [`GenesisSchnorrSigPtrSigner`].
pub const GENESIS_SIG_PTR_TAG: u8 = GenesisSchnorrSigPtrSigner::TAG;

/// Encodes an `Update` config action body.
pub fn encode_update_action(
    config_idx: u8,
    new_min_withdrawal_amount: u64,
    new_turn_ttl: u64,
    new_covenant_id: &[u8; 32],
    new_lock: &LockEnum<'_>,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(ActionTag::Update as u8);
    out.push(config_idx);
    out.extend_from_slice(&new_min_withdrawal_amount.to_le_bytes());
    out.extend_from_slice(&new_turn_ttl.to_le_bytes());
    out.extend_from_slice(new_covenant_id);
    new_lock.encode(&mut out);
    out
}

/// Encodes an `Init` config action body.
pub fn encode_init_action(
    config_idx: u8,
    new_min_withdrawal_amount: u64,
    new_turn_ttl: u64,
    new_covenant_id: &[u8; 32],
    new_lock: &LockEnum<'_>,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(ActionTag::Init as u8);
    out.push(config_idx);
    out.extend_from_slice(&new_min_withdrawal_amount.to_le_bytes());
    out.extend_from_slice(&new_turn_ttl.to_le_bytes());
    out.extend_from_slice(new_covenant_id);
    new_lock.encode(&mut out);
    out
}

/// Encodes a `Transfer` action crediting an existing destination.
pub fn encode_transfer_action(source_idx: u8, dest_idx: u8, amount: u64) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(ActionTag::Transfer as u8);
    out.push(source_idx);
    out.push(dest_idx);
    out.extend_from_slice(&amount.to_le_bytes());
    out.push(0);
    out
}

/// Encodes a `Transfer` action creating its destination if the slot is new.
pub fn encode_transfer_create_action(
    source_idx: u8,
    dest_idx: u8,
    amount: u64,
    dest_init: &LockEnum<'_>,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(ActionTag::Transfer as u8);
    out.push(source_idx);
    out.push(dest_idx);
    out.extend_from_slice(&amount.to_le_bytes());
    out.push(1);
    dest_init.encode(&mut out);
    out
}

/// Encodes an `UpdateUserLock` action body.
pub fn encode_update_user_lock_action(user_idx: u8, new_lock: &LockEnum<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(ActionTag::UpdateUserLock as u8);
    out.push(user_idx);
    new_lock.encode(&mut out);
    out
}

/// Encodes a `Deposit` action body.
pub fn encode_deposit_action(
    user_idx: u8,
    config_idx: u8,
    output_idx: u32,
    initial_lock: &LockEnum<'_>,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(ActionTag::Deposit as u8);
    out.push(user_idx);
    out.push(config_idx);
    out.extend_from_slice(&output_idx.to_le_bytes());
    initial_lock.encode(&mut out);
    out
}

/// Encodes a `Withdraw` action body.
pub fn encode_withdraw_action(
    user_idx: u8,
    config_idx: u8,
    amount: u64,
    dest: &StandardSpk<'_>,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(ActionTag::Withdraw as u8);
    out.push(user_idx);
    out.push(config_idx);
    out.extend_from_slice(&amount.to_le_bytes());
    dest.encode(&mut out);
    out
}

/// Encodes a `CreateGame` action body.
pub fn encode_create_game_action(
    creator_idx: u8,
    game_idx: u8,
    stake: u64,
    rounds: u8,
    mark: Cell,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(ActionTag::CreateGame as u8);
    out.push(creator_idx);
    out.push(game_idx);
    out.extend_from_slice(&stake.to_le_bytes());
    out.push(rounds);
    out.push(mark as u8);
    out
}

/// Encodes a `JoinGame` action body.
pub fn encode_join_game_action(game_idx: u8, joiner_idx: u8) -> Vec<u8> {
    alloc::vec![ActionTag::JoinGame as u8, game_idx, joiner_idx]
}

/// Encodes a `Turn` action body.
pub fn encode_turn_action(game_idx: u8, user_idx: u8, cell: u8) -> Vec<u8> {
    alloc::vec![ActionTag::Turn as u8, game_idx, user_idx, cell]
}

/// Encodes a `Timeout` action body.
pub fn encode_timeout_action(game_idx: u8, config_idx: u8) -> Vec<u8> {
    alloc::vec![ActionTag::Timeout as u8, game_idx, config_idx]
}

/// Sorted access positions for a user resource and the singleton config resource.
pub struct UserConfigAccess {
    /// Position of the user resource in the sorted access list.
    pub user_idx: u8,
    /// Position of the config resource in the sorted access list.
    pub config_idx: u8,
    /// Access metadata entries sorted strictly ascending by [`ResourceId`].
    pub access: Vec<AccessMetadata>,
}

/// Sorted access positions for two user resources.
pub struct TwoUserAccess {
    /// Position of the source user resource in the sorted access list.
    pub source_idx: u8,
    /// Position of the destination user resource in the sorted access list.
    pub dest_idx: u8,
    /// Access metadata entries sorted strictly ascending by [`ResourceId`].
    pub access: Vec<AccessMetadata>,
}

/// Sorted access positions for a game resource and a user resource.
pub struct GameUserAccess {
    /// Position of the game resource in the sorted access list.
    pub game_idx: u8,
    /// Position of the user resource in the sorted access list.
    pub user_idx: u8,
    /// Access metadata entries sorted strictly ascending by [`ResourceId`].
    pub access: Vec<AccessMetadata>,
}

/// Sorted access positions for a game resource and the singleton config resource.
pub struct GameConfigAccess {
    /// Position of the game resource in the sorted access list.
    pub game_idx: u8,
    /// Position of the config resource in the sorted access list.
    pub config_idx: u8,
    /// Access metadata entries sorted strictly ascending by [`ResourceId`].
    pub access: Vec<AccessMetadata>,
}

/// Sorted access positions for a game resource and two user resources.
pub struct GameTwoUserAccess {
    /// Position of the game resource in the sorted access list.
    pub game_idx: u8,
    /// Position of the first user resource in the sorted access list.
    pub first_user_idx: u8,
    /// Position of the second user resource in the sorted access list.
    pub second_user_idx: u8,
    /// Access metadata entries sorted strictly ascending by [`ResourceId`].
    pub access: Vec<AccessMetadata>,
}

/// Returns sorted access metadata for a user (Write) and config (Read) pair.
pub fn user_config_access(user_id: ResourceId, config_id: ResourceId) -> UserConfigAccess {
    let (user_idx, config_idx, access) = if user_id < config_id {
        (0, 1, alloc::vec![AccessMetadata::write(user_id), AccessMetadata::read(config_id)])
    } else {
        (1, 0, alloc::vec![AccessMetadata::read(config_id), AccessMetadata::write(user_id)])
    };
    UserConfigAccess { user_idx, config_idx, access }
}

/// Returns sorted access metadata for two user (Write) resources.
pub fn two_user_access(source_id: ResourceId, dest_id: ResourceId) -> TwoUserAccess {
    let (source_idx, dest_idx, access) = if source_id < dest_id {
        (0, 1, alloc::vec![AccessMetadata::write(source_id), AccessMetadata::write(dest_id)])
    } else {
        (1, 0, alloc::vec![AccessMetadata::write(dest_id), AccessMetadata::write(source_id)])
    };
    TwoUserAccess { source_idx, dest_idx, access }
}

/// Returns sorted access metadata for a game (Write) and user (Write) pair.
pub fn game_user_access(game_id: ResourceId, user_id: ResourceId) -> GameUserAccess {
    let (game_idx, user_idx, access) = if game_id < user_id {
        (0, 1, alloc::vec![AccessMetadata::write(game_id), AccessMetadata::write(user_id)])
    } else {
        (1, 0, alloc::vec![AccessMetadata::write(user_id), AccessMetadata::write(game_id)])
    };
    GameUserAccess { game_idx, user_idx, access }
}

/// Returns sorted access metadata for a game (Write) and config (Read) pair.
pub fn game_config_access(game_id: ResourceId, config_id: ResourceId) -> GameConfigAccess {
    let (game_idx, config_idx, access) = if game_id < config_id {
        (0, 1, alloc::vec![AccessMetadata::write(game_id), AccessMetadata::read(config_id)])
    } else {
        (1, 0, alloc::vec![AccessMetadata::read(config_id), AccessMetadata::write(game_id)])
    };
    GameConfigAccess { game_idx, config_idx, access }
}

/// Returns sorted access metadata for a game (Write) and two user (Write) resources.
pub fn game_two_user_access(
    game_id: ResourceId,
    first_user_id: ResourceId,
    second_user_id: ResourceId,
) -> GameTwoUserAccess {
    let mut entries = [
        (0u8, AccessMetadata::write(game_id)),
        (1u8, AccessMetadata::write(first_user_id)),
        (2u8, AccessMetadata::write(second_user_id)),
    ];
    entries.sort_by_key(|(_, meta)| meta.resource_id);
    let mut game_idx = 0;
    let mut first_user_idx = 0;
    let mut second_user_idx = 0;
    for (sorted_idx, (orig_tag, _)) in entries.iter().enumerate() {
        match orig_tag {
            0 => game_idx = sorted_idx as u8,
            1 => first_user_idx = sorted_idx as u8,
            _ => second_user_idx = sorted_idx as u8,
        }
    }
    let access = alloc::vec![entries[0].1, entries[1].1, entries[2].1];
    GameTwoUserAccess { game_idx, first_user_idx, second_user_idx, access }
}

#[cfg(test)]
mod tests {
    use vprogs_zk_backend_risc0_runtime_processor::lock_variants::{
        SchnorrLockView, UnlockedLockView,
    };

    use super::*;
    use crate::program::action::{ActionBody, decode_action};

    #[test]
    fn genesis_sig_ptr_tag_matches_signer_tag() {
        assert_eq!(GENESIS_SIG_PTR_TAG, GenesisSchnorrSigPtrSigner::TAG);
        assert_eq!(GENESIS_SIG_PTR_TAG, 0x05);
    }

    #[test]
    fn round_trip_update_action() {
        let pubkey = [0x42u8; 32];
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &pubkey });
        let cov = [0x55u8; 32];
        let encoded = encode_update_action(0, 100, 200, &cov, &lock);
        let mut buf = encoded.as_slice();
        let view = decode_action(&mut buf, 1).expect("decode update");
        assert!(buf.is_empty());
        match view.body {
            ActionBody::Update {
                config_idx,
                new_min_withdrawal_amount,
                new_turn_ttl,
                new_covenant_id,
                new_lock,
            } => {
                assert_eq!(config_idx, 0);
                assert_eq!(new_min_withdrawal_amount, 100);
                assert_eq!(new_turn_ttl, 200);
                assert_eq!(new_covenant_id, cov);
                assert_eq!(new_lock.id_hash(), lock.id_hash());
            }
            _ => panic!("expected update action"),
        }
    }

    #[test]
    fn round_trip_init_action() {
        let pubkey = [0x43u8; 32];
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &pubkey });
        let cov = [0x56u8; 32];
        let encoded = encode_init_action(0, 50, 300, &cov, &lock);
        let mut buf = encoded.as_slice();
        let view = decode_action(&mut buf, 1).expect("decode init");
        assert!(buf.is_empty());
        match view.body {
            ActionBody::Init {
                config_idx,
                new_min_withdrawal_amount,
                new_turn_ttl,
                new_covenant_id,
                new_lock,
            } => {
                assert_eq!(config_idx, 0);
                assert_eq!(new_min_withdrawal_amount, 50);
                assert_eq!(new_turn_ttl, 300);
                assert_eq!(new_covenant_id, cov);
                assert_eq!(new_lock.id_hash(), lock.id_hash());
            }
            _ => panic!("expected init action"),
        }
    }

    #[test]
    fn round_trip_transfer_action() {
        let encoded = encode_transfer_action(0, 1, 1000);
        let mut buf = encoded.as_slice();
        let view = decode_action(&mut buf, 2).expect("decode transfer");
        assert!(buf.is_empty());
        match view.body {
            ActionBody::Transfer { source_idx, dest_idx, amount, dest_init } => {
                assert_eq!(source_idx, 0);
                assert_eq!(dest_idx, 1);
                assert_eq!(amount, 1000);
                assert!(dest_init.is_none());
            }
            _ => panic!("expected transfer action"),
        }
    }

    #[test]
    fn round_trip_transfer_create_action() {
        let pubkey = [0x44u8; 32];
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &pubkey });
        let encoded = encode_transfer_create_action(0, 1, 2000, &lock);
        let mut buf = encoded.as_slice();
        let view = decode_action(&mut buf, 2).expect("decode transfer create");
        assert!(buf.is_empty());
        match view.body {
            ActionBody::Transfer { source_idx, dest_idx, amount, dest_init } => {
                assert_eq!(source_idx, 0);
                assert_eq!(dest_idx, 1);
                assert_eq!(amount, 2000);
                let dest_lock = dest_init.expect("dest lock present");
                assert_eq!(dest_lock.id_hash(), lock.id_hash());
            }
            _ => panic!("expected transfer action"),
        }
    }

    #[test]
    fn round_trip_update_user_lock_action() {
        let lock = LockEnum::Unlocked(UnlockedLockView);
        let encoded = encode_update_user_lock_action(2, &lock);
        let mut buf = encoded.as_slice();
        let view = decode_action(&mut buf, 3).expect("decode update user lock");
        assert!(buf.is_empty());
        match view.body {
            ActionBody::UpdateUserLock { user_idx, new_lock } => {
                assert_eq!(user_idx, 2);
                assert_eq!(new_lock.id_hash(), lock.id_hash());
            }
            _ => panic!("expected update user lock action"),
        }
    }

    #[test]
    fn round_trip_deposit_action() {
        let pubkey = [0x45u8; 32];
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &pubkey });
        let encoded = encode_deposit_action(0, 1, 7, &lock);
        let mut buf = encoded.as_slice();
        let view = decode_action(&mut buf, 2).expect("decode deposit");
        assert!(buf.is_empty());
        match view.body {
            ActionBody::Deposit { user_idx, config_idx, output_idx, initial_lock } => {
                assert_eq!(user_idx, 0);
                assert_eq!(config_idx, 1);
                assert_eq!(output_idx, 7);
                assert_eq!(initial_lock.id_hash(), lock.id_hash());
            }
            _ => panic!("expected deposit action"),
        }
    }

    #[test]
    fn round_trip_withdraw_action() {
        let pk = [0x77u8; 32];
        let dest = StandardSpk::PubKey(&pk);
        let encoded = encode_withdraw_action(1, 0, 500, &dest);
        let mut buf = encoded.as_slice();
        let view = decode_action(&mut buf, 2).expect("decode withdraw");
        assert!(buf.is_empty());
        match view.body {
            ActionBody::Withdraw { user_idx, config_idx, amount, dest: decoded_dest } => {
                assert_eq!(user_idx, 1);
                assert_eq!(config_idx, 0);
                assert_eq!(amount, 500);
                assert_eq!(decoded_dest, dest);
            }
            _ => panic!("expected withdraw action"),
        }
    }

    #[test]
    fn round_trip_create_game_action() {
        for (mark, expected) in [(Cell::X, Cell::X), (Cell::O, Cell::O)] {
            let encoded = encode_create_game_action(0, 1, 5000, 3, mark);
            let mut buf = encoded.as_slice();
            let view = decode_action(&mut buf, 2).expect("decode create game");
            assert!(buf.is_empty());
            match view.body {
                ActionBody::CreateGame { creator_idx, game_idx, stake, rounds, mark: m } => {
                    assert_eq!(creator_idx, 0);
                    assert_eq!(game_idx, 1);
                    assert_eq!(stake, 5000);
                    assert_eq!(rounds, 3);
                    assert_eq!(m, expected);
                }
                _ => panic!("expected create game action"),
            }
        }
    }

    #[test]
    fn round_trip_join_game_action() {
        let encoded = encode_join_game_action(1, 0);
        let mut buf = encoded.as_slice();
        let view = decode_action(&mut buf, 2).expect("decode join game");
        assert!(buf.is_empty());
        match view.body {
            ActionBody::JoinGame { game_idx, joiner_idx } => {
                assert_eq!(game_idx, 1);
                assert_eq!(joiner_idx, 0);
            }
            _ => panic!("expected join game action"),
        }
    }

    #[test]
    fn round_trip_turn_action() {
        for cell in [0u8, 4u8, 8u8] {
            let encoded = encode_turn_action(0, 1, cell);
            let mut buf = encoded.as_slice();
            let view = decode_action(&mut buf, 2).expect("decode turn");
            assert!(buf.is_empty());
            match view.body {
                ActionBody::Turn { game_idx, user_idx, cell: c } => {
                    assert_eq!(game_idx, 0);
                    assert_eq!(user_idx, 1);
                    assert_eq!(c, cell);
                }
                _ => panic!("expected turn action"),
            }
        }
    }

    #[test]
    fn round_trip_timeout_action() {
        let encoded = encode_timeout_action(0, 1);
        let mut buf = encoded.as_slice();
        let view = decode_action(&mut buf, 2).expect("decode timeout");
        assert!(buf.is_empty());
        match view.body {
            ActionBody::Timeout { game_idx, config_idx } => {
                assert_eq!(game_idx, 0);
                assert_eq!(config_idx, 1);
            }
            _ => panic!("expected timeout action"),
        }
    }

    #[test]
    fn user_config_access_sorts_and_flips_indices() {
        let id_lo = ResourceId::from([0x01u8; 32]);
        let id_hi = ResourceId::from([0x02u8; 32]);

        let a = user_config_access(id_lo, id_hi);
        assert_eq!(a.user_idx, 0);
        assert_eq!(a.config_idx, 1);
        assert_eq!(a.access[0].resource_id, id_lo);
        assert_eq!(a.access[1].resource_id, id_hi);

        let b = user_config_access(id_hi, id_lo);
        assert_eq!(b.user_idx, 1);
        assert_eq!(b.config_idx, 0);
        assert_eq!(b.access[0].resource_id, id_lo);
        assert_eq!(b.access[1].resource_id, id_hi);
    }

    #[test]
    fn two_user_access_sorts_and_flips_indices() {
        let id_lo = ResourceId::from([0x10u8; 32]);
        let id_hi = ResourceId::from([0x20u8; 32]);

        let a = two_user_access(id_lo, id_hi);
        assert_eq!(a.source_idx, 0);
        assert_eq!(a.dest_idx, 1);

        let b = two_user_access(id_hi, id_lo);
        assert_eq!(b.source_idx, 1);
        assert_eq!(b.dest_idx, 0);
    }

    #[test]
    fn game_user_access_sorts_and_flips_indices() {
        let id_lo = ResourceId::from([0x05u8; 32]);
        let id_hi = ResourceId::from([0x08u8; 32]);

        let a = game_user_access(id_lo, id_hi);
        assert_eq!(a.game_idx, 0);
        assert_eq!(a.user_idx, 1);

        let b = game_user_access(id_hi, id_lo);
        assert_eq!(b.game_idx, 1);
        assert_eq!(b.user_idx, 0);
    }

    #[test]
    fn game_config_access_sorts_and_flips_indices() {
        let id_lo = ResourceId::from([0x30u8; 32]);
        let id_hi = ResourceId::from([0x40u8; 32]);

        let a = game_config_access(id_lo, id_hi);
        assert_eq!(a.game_idx, 0);
        assert_eq!(a.config_idx, 1);

        let b = game_config_access(id_hi, id_lo);
        assert_eq!(b.game_idx, 1);
        assert_eq!(b.config_idx, 0);
    }

    #[test]
    fn game_two_user_access_sorts_all_permutations() {
        let id_0 = ResourceId::from([0x01u8; 32]);
        let id_1 = ResourceId::from([0x02u8; 32]);
        let id_2 = ResourceId::from([0x03u8; 32]);

        // game = id_0, u1 = id_1, u2 = id_2
        let a = game_two_user_access(id_0, id_1, id_2);
        assert_eq!(a.game_idx, 0);
        assert_eq!(a.first_user_idx, 1);
        assert_eq!(a.second_user_idx, 2);
        assert_eq!(a.access[0].resource_id, id_0);
        assert_eq!(a.access[1].resource_id, id_1);
        assert_eq!(a.access[2].resource_id, id_2);

        // game = id_2, u1 = id_0, u2 = id_1
        let b = game_two_user_access(id_2, id_0, id_1);
        assert_eq!(b.game_idx, 2);
        assert_eq!(b.first_user_idx, 0);
        assert_eq!(b.second_user_idx, 1);
        assert_eq!(b.access[0].resource_id, id_0);
        assert_eq!(b.access[1].resource_id, id_1);
        assert_eq!(b.access[2].resource_id, id_2);

        // game = id_1, u1 = id_2, u2 = id_0
        let c = game_two_user_access(id_1, id_2, id_0);
        assert_eq!(c.game_idx, 1);
        assert_eq!(c.first_user_idx, 2);
        assert_eq!(c.second_user_idx, 0);
        assert_eq!(c.access[0].resource_id, id_0);
        assert_eq!(c.access[1].resource_id, id_1);
        assert_eq!(c.access[2].resource_id, id_2);
    }

    #[test]
    fn ix_round_trip_multiple_actions() {
        use crate::runtime::ix::decode_ix;

        let pubkey = [0x42u8; 32];
        let lock = LockEnum::Schnorr(SchnorrLockView { pubkey: &pubkey });
        let cov = [0x55u8; 32];

        // 3 resources: 0 = game, 1 = config, 2 = user
        let act1 = encode_init_action(1, 100, 200, &cov, &lock);
        let act2 = encode_create_game_action(2, 0, 500, 3, Cell::X);
        let act3 = encode_turn_action(0, 2, 4);

        let mut ix = Vec::new();
        // 0 signers
        ix.extend_from_slice(&0u32.to_le_bytes());
        // 3 actions
        ix.extend_from_slice(&3u32.to_le_bytes());
        ix.extend_from_slice(&act1);
        ix.extend_from_slice(&act2);
        ix.extend_from_slice(&act3);

        let decoded = decode_ix(&ix, 3, decode_action).expect("decode ix");
        assert_eq!(decoded.actions.len(), 3);
        assert_eq!(decoded.actions[0].action_tag, ActionTag::Init);
        assert_eq!(decoded.actions[1].action_tag, ActionTag::CreateGame);
        assert_eq!(decoded.actions[2].action_tag, ActionTag::Turn);
    }
}
