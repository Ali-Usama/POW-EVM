//! A collection of node-specific RPC methods.
//! Substrate provides the `sc-rpc` crate, which defines the core RPC layer
//! used by Substrate nodes. This file extends those RPC definitions with
//! capabilities that are specific to this project's runtime configuration.

#![warn(missing_docs)]

use std::sync::Arc;
use std::collections::BTreeMap;

use jsonrpsee::RpcModule;
use node_template_runtime::{opaque::Block, AccountId, Balance, Index, Hash};
use sc_client_api::{backend::{Backend, StorageProvider, StateBackend},
					client::BlockchainEvents,
					AuxStore};
use sc_network::NetworkService;
use sc_transaction_pool::{ChainApi, Pool};
use sc_transaction_pool_api::TransactionPool;
use sp_api::ProvideRuntimeApi;
use sp_block_builder::BlockBuilder;
use sp_blockchain::{Error as BlockChainError, HeaderBackend, HeaderMetadata};
use fc_rpc::{
	EthBlockDataCacheTask, OverrideHandle, RuntimeApiStorageOverride, SchemaV1Override,
	SchemaV2Override, SchemaV3Override, StorageOverride,
};
use fc_rpc_core::types::{FeeHistoryCache, FeeHistoryCacheLimit};
use fp_storage::EthereumStorageSchema;

pub use sc_rpc_api::DenyUnsafe;
use sp_runtime::traits::BlakeTwo256;

/// EVM overrides
pub fn overrides_handle<C, BE>(client: Arc<C>) -> Arc<OverrideHandle<Block>>
	where
		C: ProvideRuntimeApi<Block> + StorageProvider<Block, BE> + AuxStore,
		C: HeaderBackend<Block> + HeaderMetadata<Block, Error=BlockChainError>,
		C: Send + Sync + 'static,
		C::Api: sp_api::ApiExt<Block>
		+ fp_rpc::EthereumRuntimeRPCApi<Block>
		+ fp_rpc::ConvertTransactionRuntimeApi<Block>,
		BE: Backend<Block> + 'static,
		BE::State: StateBackend<BlakeTwo256>,
{
	let mut overrides_map = BTreeMap::new();
	overrides_map.insert(
		EthereumStorageSchema::V1,
		Box::new(SchemaV1Override::new(client.clone()))
			as Box<dyn StorageOverride<_> + Send + Sync>,
	);
	overrides_map.insert(
		EthereumStorageSchema::V2,
		Box::new(SchemaV2Override::new(client.clone()))
			as Box<dyn StorageOverride<_> + Send + Sync>,
	);
	overrides_map.insert(
		EthereumStorageSchema::V3,
		Box::new(SchemaV3Override::new(client.clone()))
			as Box<dyn StorageOverride<_> + Send + Sync>,
	);

	Arc::new(OverrideHandle {
		schemas: overrides_map,
		fallback: Box::new(RuntimeApiStorageOverride::new(client)),
	})
}


/// Full client dependencies.
pub struct FullDeps<C, P, A: ChainApi> {
	/// The client instance to use.
	pub client: Arc<C>,
	/// Transaction pool instance.
	pub pool: Arc<P>,
	/// Graph pool instance
	pub graph: Arc<Pool<A>>,
	/// Whether to deny unsafe calls
	pub deny_unsafe: DenyUnsafe,
	/// Node Authority Flag
	pub is_authority: bool,
	/// Network Service
	pub network: Arc<NetworkService<Block, Hash>>,
	/// Backend
	pub backend: Arc<fc_db::Backend<Block>>,
	/// Fee history cache.
	pub fee_history_cache: FeeHistoryCache,
	/// Maximum fee history cache size.
	pub fee_history_cache_limit: FeeHistoryCacheLimit,
	/// Cache for Ethereum Block Data
	pub block_data_cache: Arc<EthBlockDataCacheTask<Block>>,
}

/// Instantiate all full RPC extensions.
pub fn create_full<C, P, BE, A>(
	deps: FullDeps<C, P, A>,
) -> Result<RpcModule<()>, Box<dyn std::error::Error + Send + Sync>>
	where
		BE: Backend<Block> + 'static,
		BE::State: StateBackend<BlakeTwo256>,
		C: BlockchainEvents<Block>,
		C: ProvideRuntimeApi<Block>,
		C: StorageProvider<Block, BE>,
		C: HeaderBackend<Block> + HeaderMetadata<Block, Error=BlockChainError> + 'static,
		C: Send + Sync + 'static,
		C: StorageProvider<Block, BE>,
		C::Api: substrate_frame_rpc_system::AccountNonceApi<Block, AccountId, Index>,
		C::Api: pallet_transaction_payment_rpc::TransactionPaymentRuntimeApi<Block, Balance>,
		C::Api: BlockBuilder<Block>,
		C::Api: fp_rpc::ConvertTransactionRuntimeApi<Block>,
		C::Api: fp_rpc::EthereumRuntimeRPCApi<Block>,
		P: TransactionPool<Block=Block> + 'static,
		A: ChainApi<Block=Block> + 'static,
{
	use fc_rpc::{Eth, EthApiServer, Net, NetApiServer};
	use pallet_transaction_payment_rpc::{TransactionPayment, TransactionPaymentApiServer};
	use substrate_frame_rpc_system::{System, SystemApiServer};

	let mut module = RpcModule::new(());
	let FullDeps {
		client, pool, graph,
		deny_unsafe, is_authority, network, backend,
		fee_history_cache, fee_history_cache_limit,
		block_data_cache
	} = deps;
	// We won't use the override feature
	let overrides = Arc::new(OverrideHandle {
		schemas: BTreeMap::new(),
		fallback: Box::new(RuntimeApiStorageOverride::new(client.clone())),
	});

	// Nor any signers
	let signers = Vec::new();

	// Limit the number of queryable logs.
	// let max_past_logs: u32 = 1024;

	module.merge(System::new(client.clone(), pool.clone(), deny_unsafe).into_rpc())?;
	module.merge(TransactionPayment::new(client.clone()).into_rpc())?;
	module.merge(Net::new(client.clone(), network.clone(), true).into_rpc())?;
	module.merge(
		Eth::new(
			client.clone(),
			pool.clone(),
			graph,
			Some(node_template_runtime::TransactionConverter),
			network.clone(),
			signers,
			overrides.clone(),
			backend.clone(),
			// Is authority.
			is_authority,
			block_data_cache.clone(),
			fee_history_cache,
			fee_history_cache_limit,
			10,
		)
			.into_rpc())?;

	// Extend this RPC with a custom API by using the following syntax.
	// `YourRpcStruct` should have a reference to a client, which is needed
	// to call into the runtime.
	// `module.merge(YourRpcTrait::into_rpc(YourRpcStruct::new(ReferenceToClient, ...)))?;`


	// Reasonable default caching inspired by the frontier template
	// let block_data_cache = Arc::new(EthBlockDataCacheTask::new(50, 50));


	Ok(module)
}
