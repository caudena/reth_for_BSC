use alloy_consensus::TxEnvelope;
use alloy_eips::BlockNumberOrTag;
use alloy_network::Ethereum;
use alloy_network::TransactionResponse;
use alloy_primitives::{hex, map::HashSet, Address, FixedBytes, Signature as Alloy_Signature};
use alloy_rpc_types::Block;
use alloy_rpc_types_eth::{BlockId, BlockTransactions};
use alloy_rpc_types_trace::parity::{
    LocalizedTransactionTrace, TraceResults, TraceResultsWithTransactionHash, TraceType,
};
use futures::join;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use jsonrpsee_types::ErrorObjectOwned;
use reth_node_api::BlockBody;
use reth_rpc_convert::CustomRpcHeader;
use reth_rpc_eth_api::helpers::{EthBlocks, FullEthApi, Trace};
use revm_inspectors::tracing::TracingInspectorConfig;
use serde::{Deserialize, Serialize};
use tracing::trace;

/// `EnrichedTransaction` object used in RPC
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnrichedTransaction {
    ///Alloy ETH transaction
    #[serde(flatten)]
    pub inner: alloy_rpc_types_eth::Transaction,

    ///compressed public key
    pub public_key: String,

    ///Alloy ETH receipts
    pub receipts: alloy_rpc_types_eth::TransactionReceipt,

    ///Alloy traces
    pub trace: TraceResults,
}

/// `EnrichedBlock` object used in RPC
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnrichedBlock {
    ///Alloy block
    #[serde(flatten)]
    pub inner: Block<EnrichedTransaction, CustomRpcHeader>,

    ///static block rewards
    pub rewards: Vec<LocalizedTransactionTrace>,
}

#[rpc(server, namespace = "eth")]
pub trait EthBlockReceiptsTraceApi {
    /// Returns enriched block with receipts, traces, and rewards.
    #[method(name = "getBlockReceiptsTrace")]
    async fn block_receipts_trace(
        &self,
        number: BlockNumberOrTag,
    ) -> RpcResult<Option<EnrichedBlock>>;
}

/// Wrapper around an Eth API for block receipts trace RPC.
#[derive(Debug)]
pub struct EthBlockReceiptsTraceExt<Eth> {
    eth_api: Eth,
}

impl<Eth> EthBlockReceiptsTraceExt<Eth> {
    /// Creates a new instance.
    pub const fn new(eth_api: Eth) -> Self {
        Self { eth_api }
    }
}

#[async_trait::async_trait]
impl<Eth> EthBlockReceiptsTraceApiServer for EthBlockReceiptsTraceExt<Eth>
where
    Eth: FullEthApi<NetworkTypes = Ethereum> + Trace + Clone + 'static,
{
    async fn block_receipts_trace(
        &self,
        number: BlockNumberOrTag,
    ) -> RpcResult<Option<EnrichedBlock>> {
        trace!(target: "rpc::eth", ?number, ?true, "Serving eth_getBlockReceiptTrace");

        let trace_task = tokio::spawn({
            let eth_api = self.eth_api.clone();

            async move {
                let mut trace_types: HashSet<TraceType> = HashSet::default();
                trace_types.insert(TraceType::Trace);

                let block_id = BlockId::Number(number);

                eth_api
                    .trace_block_with(
                        block_id,
                        None,
                        TracingInspectorConfig::from_parity_config(&trace_types),
                        move |tx_info, mut ctx| {
                            let full_trace = ctx
                                .take_inspector()
                                .into_parity_builder()
                                .into_trace_results(&ctx.result, &trace_types);

                            // if let Some(ref mut state_diff) = full_trace.state_diff {
                            //     populate_state_diff(state_diff, &ctx.db, ctx.state.iter())
                            //         .map_err(|trace_res_err| {
                            //             ErrorObjectOwned::owned(
                            //                 1,
                            //                 format!(
                            //                     "Error getting block traces result {} for block{}",
                            //                     trace_res_err, number
                            //                 ),
                            //                 None::<()>,
                            //             )
                            //         })?;
                            // }

                            let trace = TraceResultsWithTransactionHash {
                                transaction_hash: tx_info.hash.expect("tx hash is set"),
                                full_trace,
                            };
                            Ok(trace)
                        },
                    )
                    .await
            }
        });

        let receipts_task = tokio::spawn({
            let eth_api = self.eth_api.clone();

            async move { EthBlocks::block_receipts(&eth_api, number.into()).await }
        });

        let (trx_traces_handle, trx_receipts_handle) = join!(trace_task, receipts_task);

        let trx_traces_handle_res = trx_traces_handle
            .map_err(|handle_err| {
                ErrorObjectOwned::owned(
                    1,
                    format!(
                        "Error in traces join handle for block number {}: {}",
                        number, handle_err
                    ),
                    None::<()>,
                )
            })?
            .map_err(|trace_res_err| {
                ErrorObjectOwned::owned(
                    1,
                    format!(
                        "Error getting block traces result {} for block{}",
                        trace_res_err, number
                    ),
                    None::<()>,
                )
            })
            .and_then(|traces_option| {
                traces_option.ok_or_else(|| {
                    ErrorObjectOwned::owned(
                        1,
                        format!("Error getting block traces option for block {}", number),
                        None::<()>,
                    )
                })
            });

        let trx_receipts_handle_res = trx_receipts_handle
            .map_err(|handle_err| {
                ErrorObjectOwned::owned(
                    1,
                    format!(
                        "Error in transaction receipts for block number {}: {}",
                        number, handle_err
                    ),
                    None::<()>,
                )
            })?
            .map_err(|receipt_res_err| {
                ErrorObjectOwned::owned(
                    2,
                    format!(
                        "Error getting transaction receipts result {} for block {}",
                        receipt_res_err, number
                    ),
                    None::<()>,
                )
            })
            .and_then(|receipts_option| {
                receipts_option.ok_or_else(|| {
                    ErrorObjectOwned::owned(
                        2,
                        format!("Error getting transaction receipts option for block{}", number),
                        None::<()>,
                    )
                })
            });

        let trx_traces = trx_traces_handle_res?;
        let trx_receipts = trx_receipts_handle_res?;

        let block = EthBlocks::rpc_block(&self.eth_api, number.into(), true)
            .await
            .map_err(|e| ErrorObjectOwned::owned(1, e.to_string(), None::<()>))?
            .unwrap();

        let block_rewards_task = tokio::spawn({
            let eth_api = self.eth_api.clone();

            let number = number.as_number().unwrap();

            async move {
                let maybe_block =
                    eth_api.recovered_block(BlockId::number(number)).await.unwrap();
                let mut trace_rewards: Vec<LocalizedTransactionTrace> = Vec::new();

                if let Some(block) = maybe_block {
                    if let Ok(Some(base_block_reward)) =
                        eth_api.calculate_base_block_reward(block.header())
                    {
                        trace_rewards.extend(eth_api.extract_reward_traces(
                            block.header(),
                            block.hash(),
                            block.body().ommers(),
                            base_block_reward,
                        ));
                    }
                }

                trace_rewards
            }
        });

        if trx_receipts.len() != block.transactions.len() {
            let trx_trace_len_error = ErrorObjectOwned::owned(
                1,
                "trx_receipts.size() != block.transactions.size()",
                None::<()>,
            );
            return Err(trx_trace_len_error);
        }

        if trx_traces.len() != block.transactions.len() {
            let trx_trace_len_error = ErrorObjectOwned::owned(
                1,
                "trx_traces.size() != block.transactions.size()",
                None::<()>,
            );
            return Err(trx_trace_len_error);
        }

        let mut enriched_trxs: Vec<EnrichedTransaction> = Vec::new();

        if let BlockTransactions::Full(transactions) = block.transactions {
            for ((trx, receipt), trace) in
                transactions.into_iter().zip(trx_receipts).zip(trx_traces) {
                if trx.tx_hash() != receipt.transaction_hash
                    || trx.tx_hash() != trace.transaction_hash
                {
                    let trx_trace_hash_error = ErrorObjectOwned::owned(
                        2,
                        format!(
                            "Mismatch between transaction hash and corresponding receipt hash {}",
                            trx.tx_hash()
                        ),
                        None::<()>,
                    );
                    return Err(trx_trace_hash_error);
                }

                let alloy_trx :alloy_rpc_types_eth::Transaction = trx;
                let  alloy_receipt:alloy_rpc_types_eth::TransactionReceipt = receipt;

                let alloy_public_key;

                let mut tx_message_hash: Option<FixedBytes<32>> = None;
                let mut tx_sig: Option<Alloy_Signature> = None;

                match TxEnvelope::try_from(alloy_trx.inner.inner().clone()) {
                    Ok(tx_envelope) => match tx_envelope {
                        TxEnvelope::Legacy(typed_tx) => {
                            tx_message_hash = Some(typed_tx.signature_hash());
                            tx_sig = Some(typed_tx.signature().clone());
                        }
                        TxEnvelope::Eip1559(typed_tx) => {
                            tx_message_hash = Some(typed_tx.signature_hash());
                            tx_sig = Some(typed_tx.signature().clone());
                        }
                        TxEnvelope::Eip2930(typed_tx) => {
                            tx_message_hash = Some(typed_tx.signature_hash());
                            tx_sig = Some(typed_tx.signature().clone());
                        }
                        TxEnvelope::Eip4844(typed_tx) => {
                            tx_message_hash = Some(typed_tx.signature_hash());
                            tx_sig = Some(typed_tx.signature().clone());
                        }
                        TxEnvelope::Eip7702(typed_tx) => {
                            tx_message_hash = Some(typed_tx.signature_hash());
                            tx_sig = Some(typed_tx.signature().clone());
                        }
                    },
                    Err(e) => {
                        println!("Conversion error: {:?}", e);
                    }
                } //match TxEnvelope::try_from

                if tx_sig.is_none() || tx_message_hash.is_none() {
                    let trx_sig_error = ErrorObjectOwned::owned(
                        1,
                        format!("Signature not extracted from transaction {}", alloy_trx.tx_hash()),
                        None::<()>,
                    );
                    return Err(trx_sig_error);
                }

                let check_address: Address;
                let alloy_sig = tx_sig.unwrap().recover_from_prehash(&tx_message_hash.unwrap());

                match alloy_sig {
                    Ok(signature) => {
                        let ec = signature.to_encoded_point(true);

                        check_address = Address::from_public_key(&signature);
                        alloy_public_key = format!("0x{}", hex::encode(ec.as_bytes()));
                    }
                    Err(p_key_err) => {
                        let alloy_pub_key_err = ErrorObjectOwned::owned(
                            1,
                            format!("Public key not extracted from message and signature, error: {}, transaction hash: {}"
                                    , p_key_err, alloy_trx.tx_hash()),
                            None::<()>,
                        );
                        return Err(alloy_pub_key_err);
                    }
                }

                if check_address != alloy_trx.from() {
                    let trx_pub_key_addr_error = ErrorObjectOwned::owned(
                        1,
                        format!(
                            "Address doesn't match public key to address for {}",
                            alloy_trx.tx_hash()
                        ),
                        None::<()>,
                    );
                    return Err(trx_pub_key_addr_error);
                }

                enriched_trxs.push(EnrichedTransaction {
                    inner: alloy_trx,
                    public_key: if alloy_public_key.is_empty() {
                        "0x0".to_string()
                    } else {
                        alloy_public_key
                    }
                        .to_string(),
                    receipts: alloy_receipt,
                    trace: trace.full_trace,
                });
            }
        }

        let e_block: Block<EnrichedTransaction, CustomRpcHeader> = Block {
            header: block.header,
            uncles: block.uncles,
            transactions: BlockTransactions::Full(enriched_trxs),
            withdrawals: block.withdrawals,
        };

        #[allow(unused_assignments, unused_mut)]
        let mut block_rewards = vec![];

        block_rewards = block_rewards_task.await.unwrap();

        let rich_block: EnrichedBlock = EnrichedBlock { inner: e_block, rewards: block_rewards };

        Ok(Some(rich_block))
    }
}
