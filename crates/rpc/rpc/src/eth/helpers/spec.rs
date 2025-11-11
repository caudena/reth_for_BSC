use alloy_primitives::U256;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{spec::SignersForApi, EthApiSpec},
    RpcNodeCore,
};
use reth_storage_api::ProviderTx;

//Custom imports
use crate::trace::reward_trace;
use alloy_evm::block::calc::{base_block_reward_pre_merge, block_reward, ommer_reward};
use alloy_rpc_types_trace::parity::{LocalizedTransactionTrace, RewardAction, RewardType};
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardfork, SEPOLIA};
use reth_primitives_traits::BlockHeader;
use reth_rpc_eth_types::EthApiError;
//Custom imports

use crate::EthApi;

impl<N, Rpc> EthApiSpec for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    type Transaction = ProviderTx<N::Provider>;
    type Rpc = Rpc::Network;

    fn starting_block(&self) -> U256 {
        self.inner.starting_block()
    }

    fn signers(&self) -> &SignersForApi<Self> {
        self.inner.signers()
    }


    fn calculate_base_block_reward<H: BlockHeader>(
        &self,
        header: &H,
    ) -> Result<Option<u128>, EthApiError> {
        let chain_spec = self.provider().chain_spec();
        let is_paris_activated = if chain_spec.chain() == reth_chainspec::MAINNET.chain() {
            Some(header.number()) >= EthereumHardfork::Paris.mainnet_activation_block()
        } else if chain_spec.chain() == SEPOLIA.chain() {
            Some(header.number()) >= EthereumHardfork::Paris.sepolia_activation_block()
        } else {
            true
        };

        if is_paris_activated {
            return Ok(None);
        }

        Ok(Some(base_block_reward_pre_merge(&chain_spec, header.number())))
    }

    fn extract_reward_traces<H: BlockHeader>(
        &self,
        header: &H,
        ommers: Option<&[H]>,
        base_block_reward: u128,
    ) -> Vec<LocalizedTransactionTrace> {
        let ommers_cnt = ommers.as_ref().map(|o| o.len()).unwrap_or_default();
        let mut traces = Vec::with_capacity(ommers_cnt + 1);

        let block_reward = block_reward(base_block_reward, ommers_cnt);
        traces.push(reward_trace(
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
