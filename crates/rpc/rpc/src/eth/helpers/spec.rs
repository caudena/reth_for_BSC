use alloy_primitives::{BlockHash, U256};
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{helpers::EthApiSpec, RpcNodeCore};
use reth_rpc_eth_types::EthApiError;

use crate::EthApi;

//Custom imports
use crate::trace::reward_trace;
use alloy_evm::block::calc::{base_block_reward_pre_merge, block_reward, ommer_reward};
use alloy_rpc_types_trace::parity::{LocalizedTransactionTrace, RewardAction, RewardType};
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardfork, EthereumHardforks, SEPOLIA};
use reth_primitives_traits::BlockHeader;

impl<N, Rpc> EthApiSpec for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError>,
{
    fn starting_block(&self) -> U256 {
        self.inner.starting_block()
    }

    fn calculate_base_block_reward<H: BlockHeader>(
        &self,
        header: &H,
    ) -> Result<Option<u128>, EthApiError> {
         let chain_spec = self.provider().chain_spec();

        if chain_spec.is_paris_active_at_block(header.number()) {
            return Ok(None)
        }

        Ok(Some(base_block_reward_pre_merge(&chain_spec, header.number())))
    }

    fn extract_reward_traces<H: BlockHeader>(
        &self,
        header: &H,
        block_hash: BlockHash,
        ommers: Option<&[H]>,
        base_block_reward: u128,
    ) -> Vec<LocalizedTransactionTrace> {
        let ommers_cnt = ommers.as_ref().map(|o| o.len()).unwrap_or_default();
        let mut traces = Vec::with_capacity(ommers_cnt + 1);

        let block_reward = block_reward(base_block_reward, ommers_cnt);
        traces.push(reward_trace(
            block_hash,
            header,
            RewardAction {
                author: header.beneficiary(),
                reward_type: RewardType::Block,
                value: U256::from(block_reward),
            },
        ));

        let Some(ommers) = ommers else { return traces };

        for uncle in ommers {
            let uncle_reward = ommer_reward(base_block_reward, header.number(), uncle.number());
            traces.push(reward_trace(
                block_hash,
                header,
                RewardAction {
                    author: uncle.beneficiary(),
                    reward_type: RewardType::Uncle,
                    value: U256::from(uncle_reward),
                },
            ));
        }
        traces
    }
}
