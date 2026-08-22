#![no_std]
#![no_main]

use vprog_tictactoe_guest::program::run;
use vprogs_zk_abi::transaction_processor::process_transaction;
use vprogs_zk_backend_risc0_api::{Host, Journal, Sha256};
use vprogs_zk_backend_risc0_runtime_processor::deposit_policy::ExampleDepositPolicy;

risc0_zkvm::guest::entry!(main);

/// The deposit policy wired into this runtime. It holds no baked
/// deposit address: the address is read from the config resource (committed
/// state) at apply time, so the image id is invariant to it.
const POLICY: ExampleDepositPolicy = ExampleDepositPolicy;

fn main() {
    process_transaction::<Sha256>(
        &mut Host,
        &mut Journal,
        |tx, _merge_idx, _context_hash, resources, exits, deposit| {
            run::run(tx, resources, exits, deposit, &POLICY)
        },
    );
}
