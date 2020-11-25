use crate::bytes::Bytes;
use crate::error::Error;
use crate::state_ext::StateExt;
use crate::syscalls::{GetContractCode, L2Syscalls, RunResult};
use gw_common::state::{build_account_key, serialize_nonce, State, GW_ACCOUNT_NONCE};
use gw_types::{
    packed::{BlockInfo, CallContext, L2Block, RawL2Block},
    prelude::*,
};
use lazy_static::lazy_static;

use ckb_vm::{
    machine::asm::{AsmCoreMachine, AsmMachine},
    DefaultMachineBuilder,
};

lazy_static! {
    static ref VALIDATOR: Bytes = include_bytes!("../../../c/build/validator").to_vec().into();
    static ref GENERATOR: Bytes = include_bytes!("../../../c/build/generator").to_vec().into();
}

pub struct DepositionRequest {
    pub pubkey_hash: [u8; 20],
    pub account_id: u32,
    pub token_id: [u8; 32],
    pub value: u128,
}

pub struct StateTransitionArgs {
    pub l2block: L2Block,
    pub deposition_requests: Vec<DepositionRequest>,
}

pub struct Generator<CS> {
    generator: Bytes,
    validator: Bytes,
    code_store: CS,
}

impl<CS: GetContractCode> Generator<CS> {
    pub fn new(code_store: CS) -> Self {
        Generator {
            generator: GENERATOR.clone(),
            validator: VALIDATOR.clone(),
            code_store,
        }
    }

    /// Apply l2 state transition
    ///
    /// Notice:
    /// This function do not verify the block and transactions signature.
    /// The caller is supposed to do the verification.
    pub fn apply_state_transition<S: State>(
        &self,
        state: &mut S,
        args: StateTransitionArgs,
    ) -> Result<(), Error> {
        let raw_block = args.l2block.raw();

        // skip invalid blocks
        if raw_block.valid() == 0u8.into() {
            return Ok(());
        }

        // handle deposition
        state.apply_deposition_requests(&args.deposition_requests)?;

        // handle transactions
        if raw_block.submit_transactions().to_opt().is_some() {
            let block_info = get_block_info(&raw_block);
            for tx in args.l2block.transactions() {
                let raw_tx = tx.raw();
                // check nonce
                let expected_nonce = state.get_nonce(raw_tx.from_id().unpack())?;
                let actual_nonce: u32 = raw_tx.nonce().unpack();
                if actual_nonce != expected_nonce {
                    return Err(Error::Nonce {
                        expected: expected_nonce,
                        actual: actual_nonce,
                    });
                }
                // build call context
                // NOTICE users only allowed to send HandleMessage CallType txs
                let call_context = raw_tx.to_call_context();
                let run_result = self.execute(state, &block_info, &call_context)?;
                state.apply_run_result(&run_result)?;
            }
        }

        Ok(())
    }

    /// execute a layer2 tx
    pub fn execute<S: State>(
        &self,
        state: &S,
        block_info: &BlockInfo,
        call_context: &CallContext,
    ) -> Result<RunResult, Error> {
        let mut run_result = RunResult::default();
        {
            let core_machine = Box::<AsmCoreMachine>::default();
            let machine_builder =
                DefaultMachineBuilder::new(core_machine).syscall(Box::new(L2Syscalls {
                    state,
                    block_info: block_info,
                    call_context: call_context,
                    result: &mut run_result,
                    code_store: &self.code_store,
                }));
            let mut machine = AsmMachine::new(machine_builder.build(), None);
            let program_name = Bytes::from_static(b"generator");
            machine.load_program(&self.generator, &[program_name])?;
            let code = machine.run()?;
            if code != 0 {
                return Err(Error::InvalidExitCode(code).into());
            }
        }
        // set nonce
        let sender_id: u32 = call_context.from_id().unpack();
        let nonce = state.get_nonce(sender_id)?;
        let nonce_raw_key = build_account_key(sender_id, GW_ACCOUNT_NONCE);
        if run_result.read_values.get(&nonce_raw_key).is_none() {
            run_result
                .read_values
                .insert(nonce_raw_key, serialize_nonce(nonce));
        }
        // increase nonce
        run_result
            .write_values
            .insert(nonce_raw_key, serialize_nonce(nonce + 1));
        Ok(run_result)
    }
}

fn get_block_info(l2block: &RawL2Block) -> BlockInfo {
    BlockInfo::new_builder()
        .aggregator_id(l2block.aggregator_id())
        .number(l2block.number())
        .timestamp(l2block.timestamp())
        .build()
}