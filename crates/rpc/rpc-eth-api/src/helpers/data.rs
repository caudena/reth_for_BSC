//! Helper data structures for RPC
use jsonrpsee::core::Serialize;

use serde::Deserialize;

use alloy_rpc_types::Block;
use alloy_rpc_types_trace::parity::{LocalizedTransactionTrace, TraceResults};

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
    pub inner: Block<EnrichedTransaction>,

    ///static block rewards
    pub rewards: Vec<LocalizedTransactionTrace>,
}