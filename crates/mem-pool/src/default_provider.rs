use std::sync::Arc;

use anyhow::{anyhow, Result};
use gw_poa::PoA;
use gw_rpc_client::RPCClient;
use gw_types::{
    offchain::{DepositInfo, InputCellInfo, RollupContext},
    packed::{CellInput, WithdrawalRequest},
    prelude::*,
};
use smol::{lock::Mutex, Task};

use crate::{
    constants::MAX_MEM_BLOCK_DEPOSITS, custodian::AvailableCustodians, traits::MemPoolProvider,
};

pub struct DefaultMemPoolProvider {
    /// RPC client
    rpc_client: RPCClient,
    /// POA Context
    poa: Arc<Mutex<PoA>>,
}

impl DefaultMemPoolProvider {
    pub fn new(rpc_client: RPCClient, poa: Arc<Mutex<PoA>>) -> Self {
        DefaultMemPoolProvider { rpc_client, poa }
    }
}

impl MemPoolProvider for DefaultMemPoolProvider {
    fn estimate_next_blocktime(&self) -> Task<Result<u64>> {
        // estimate next l2block timestamp
        let poa = Arc::clone(&self.poa);
        let rpc_client = self.rpc_client.clone();
        smol::spawn(async move {
            let poa = poa.lock().await;
            let rollup_cell = rpc_client
                .query_rollup_cell()
                .await?
                .ok_or_else(|| anyhow!("can't find rollup cell"))?;
            let input_cell = InputCellInfo {
                input: CellInput::new_builder()
                    .previous_output(rollup_cell.out_point.clone())
                    .build(),
                cell: rollup_cell,
            };
            let ctx = poa.query_poa_context(&input_cell).await?;
            // TODO how to estimate a more accurate timestamp?
            let timestamp = poa.estimate_next_round_start_time(ctx);
            Ok(timestamp)
        })
    }

    fn collect_deposit_cells(&self) -> Task<Result<Vec<DepositInfo>>> {
        let rpc_client = self.rpc_client.clone();
        smol::spawn(async move { rpc_client.query_deposit_cells(MAX_MEM_BLOCK_DEPOSITS).await })
    }

    fn query_available_custodians(
        &self,
        withdrawals: Vec<WithdrawalRequest>,
        last_finalized_block_number: u64,
        rollup_context: RollupContext,
    ) -> Task<Result<AvailableCustodians>> {
        let rpc_client = self.rpc_client.clone();
        smol::spawn(async move {
            let r = AvailableCustodians::build_from_withdrawals(
                &rpc_client,
                withdrawals.clone().into_iter(),
                &rollup_context,
                last_finalized_block_number,
            )
            .await?;
            Ok(r.expect_any())
        })
    }
}