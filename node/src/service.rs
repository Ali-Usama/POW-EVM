//! Service and ServiceFactory implementation. Specialized wrapper over substrate service.

use node_template_runtime::{self, opaque::Block, RuntimeApi};
use sc_client_api::{BlockBackend, BlockchainEvents, ExecutorProvider};
// use sc_consensus_aura::{ImportQueueParams, SlotProportion, StartAuraParams};
pub use sc_executor::NativeElseWasmExecutor;
use sc_finality_grandpa::SharedVoterState;
use sc_keystore::LocalKeystore;
use sc_service::{error::Error as ServiceError, Configuration, TaskManager, BasePath};
use sc_telemetry::{Telemetry, TelemetryWorker};
// use sp_consensus_aura::sr25519::AuthorityPair as AuraPair;
use std::{future, sync::{Arc, Mutex}, time::Duration, collections::BTreeMap};
use fc_mapping_sync::{MappingSyncWorker, SyncStrategy};
use fc_rpc_core::types::{FeeHistoryCache, FeeHistoryCacheLimit};
use fc_consensus::FrontierBlockImport;
// use sp_consensus::CanAuthorWithNativeNativeVersion;
use futures::StreamExt;
use sp_runtime::traits::IdentifyAccount;
use sp_core::Encode;
use sha3_pow::*;

// Our native executor instance.
pub struct ExecutorDispatch;

impl sc_executor::NativeExecutionDispatch for ExecutorDispatch {
	/// Only enable the benchmarking host functions when we actually want to benchmark.
	#[cfg(feature = "runtime-benchmarks")]
	type ExtendHostFunctions = frame_benchmarking::benchmarking::HostFunctions;
	/// Otherwise we only use the default Substrate host functions.
	#[cfg(not(feature = "runtime-benchmarks"))]
	type ExtendHostFunctions = ();

	fn dispatch(method: &str, data: &[u8]) -> Option<Vec<u8>> {
		node_template_runtime::api::dispatch(method, data)
	}

	fn native_version() -> sc_executor::NativeVersion {
		node_template_runtime::native_version()
	}
}

pub(crate) type FullClient =
sc_service::TFullClient<Block, RuntimeApi, NativeElseWasmExecutor<ExecutorDispatch>>;
type FullBackend = sc_service::TFullBackend<Block>;
type FullSelectChain = sc_consensus::LongestChain<FullBackend, Block>;
pub type Executor = NativeElseWasmExecutor<ExecutorDispatch>;

pub fn frontier_database_dir(config: &Configuration) -> std::path::PathBuf {
	let config_dir = config
		.base_path
		.as_ref()
		.map(|base_path| base_path.config_dir(config.chain_spec.id()))
		.unwrap_or_else(|| {
			BasePath::from_project("", "", config.chain_spec.id())
				.config_dir(config.chain_spec.id())
		});
	config_dir.join("frontier").join("db")
}

pub fn open_frontier_backend<C>(
	client: Arc<C>,
	config: &Configuration,
) -> Result<Arc<fc_db::Backend<Block>>, String>
	where C: sp_blockchain::HeaderBackend<Block>,
{
	Ok(Arc::new(fc_db::Backend::<Block>::new(
		client,
		&fc_db::DatabaseSettings {
			source: fc_db::DatabaseSource::RocksDb {
				path: frontier_database_dir(&config),
				cache_size: 0,
			},
		},
	)?))
}

type POWBlockImport = sc_consensus_pow::PowBlockImport<
	Block,
	Arc<FullClient>,
	FullClient,
	FullSelectChain,
	MinimalSha3Algorithm,
	Box<
		dyn sp_inherents::CreateInherentDataProviders<
			Block,
			(),
			InherentDataProviders=sp_timestamp::InherentDataProvider,
		>,
	>,
>;


pub fn new_partial(
	config: &Configuration,
) -> Result<
	sc_service::PartialComponents<
		FullClient,
		FullBackend,
		FullSelectChain,
		sc_consensus::DefaultImportQueue<Block, FullClient>,
		sc_transaction_pool::FullPool<Block, FullClient>,
		(
			FrontierBlockImport<
				Block,
				POWBlockImport,
				FullClient,
			>,
			Arc<fc_db::Backend<Block>>,
			Option<Telemetry>,
			(FeeHistoryCache, FeeHistoryCacheLimit),
			POWBlockImport,
		),
	>,
	ServiceError,
> {
	if config.keystore_remote.is_some() {
		return Err(ServiceError::Other("Remote Keystores are not supported.".into()));
	}

	let telemetry = config
		.telemetry_endpoints
		.clone()
		.filter(|x| !x.is_empty())
		.map(|endpoints| -> Result<_, sc_telemetry::Error> {
			let worker = TelemetryWorker::new(16)?;
			let telemetry = worker.handle().new_telemetry(endpoints);
			Ok((worker, telemetry))
		})
		.transpose()?;

	let executor = NativeElseWasmExecutor::<ExecutorDispatch>::new(
		config.wasm_method,
		config.default_heap_pages,
		config.max_runtime_instances,
		config.runtime_cache_size,
	);

	let (client, backend, keystore_container, task_manager) =
		sc_service::new_full_parts::<Block, RuntimeApi, Executor>(
			&config,
			telemetry.as_ref().map(|(_, telemetry)| telemetry.handle()),
			executor,
		)?;
	let client = Arc::new(client);

	let telemetry = telemetry.map(|(worker, telemetry)| {
		task_manager.spawn_handle().spawn("telemetry", None, worker.run());
		telemetry
	});

	let select_chain = sc_consensus::LongestChain::new(backend.clone());

	let transaction_pool = sc_transaction_pool::BasicPool::new_full(
		config.transaction_pool.clone(),
		config.role.is_authority().into(),
		config.prometheus_registry(),
		task_manager.spawn_essential_handle(),
		client.clone(),
	);


	let pow_block_import = sc_consensus_pow::PowBlockImport::new(
		client.clone(),
		client.clone(),
		MinimalSha3Algorithm,
		0,
		select_chain.clone(),
		Box::new(move |_, ()| async move {
			let provider = sp_timestamp::InherentDataProvider::from_system_time();
			Ok(provider)
		})
			as Box<
			dyn sp_inherents::CreateInherentDataProviders<
				Block,
				(),
				InherentDataProviders=sp_timestamp::InherentDataProvider,
			>,
		>,
	);

	let frontier_backend = open_frontier_backend(
		client.clone(),
		config,
	)?;

	let frontier_block_import = FrontierBlockImport::new(
		pow_block_import.clone(),
		client.clone(),
		frontier_backend.clone(),
	);

	// let can_author_with = sp_consensus::CanAuthorWithNativeVersion::new(client.executor().clone());


	let fee_history_limit: u64 = 2048;
	let fee_history_cache: FeeHistoryCache = Arc::new(Mutex::new(BTreeMap::new()));
	let fee_history_cache_limit: FeeHistoryCacheLimit = fee_history_limit;
	let fee_history = (fee_history_cache, fee_history_cache_limit);

	let import_queue = sc_consensus_pow::import_queue(
		Box::new(pow_block_import.clone()),
		None,
		MinimalSha3Algorithm,
		&task_manager.spawn_essential_handle(),
		None,
	)?;

	Ok(sc_service::PartialComponents {
		client,
		backend,
		task_manager,
		import_queue,
		keystore_container,
		select_chain,
		transaction_pool,
		other: (frontier_block_import, frontier_backend, telemetry, fee_history, pow_block_import),
	})
}

fn remote_keystore(_url: &String) -> Result<Arc<LocalKeystore>, &'static str> {
	// FIXME: here would the concrete keystore be built,
	//        must return a concrete type (NOT `LocalKeystore`) that
	//        implements `CryptoStore` and `SyncCryptoStore`
	Err("Remote Keystore not supported.")
}

/// Builds a new service for a full client.
pub fn new_full(mut config: Configuration) -> Result<TaskManager, ServiceError> {
	let sc_service::PartialComponents {
		client,
		backend,
		mut task_manager,
		import_queue,
		mut keystore_container,
		select_chain,
		transaction_pool,
		other: (block_import, frontier_backend, mut telemetry, fee_history,
			pow_block_import),
	} = new_partial(&config)?;

	if let Some(url) = &config.keystore_remote {
		match remote_keystore(url) {
			Ok(k) => keystore_container.set_remote_keystore(k),
			Err(e) =>
				return Err(ServiceError::Other(format!(
					"Error hooking up remote keystore for {}: {}",
					url, e
				))),
		};
	}
	let grandpa_protocol_name = sc_finality_grandpa::protocol_standard_name(
		&client.block_hash(0).ok().flatten().expect("Genesis block exists; qed"),
		&config.chain_spec,
	);

	config
		.network
		.extra_sets
		.push(sc_finality_grandpa::grandpa_peers_set_config(grandpa_protocol_name.clone()));

	let (network, system_rpc_tx, tx_handler_controller, network_starter) =
		sc_service::build_network(sc_service::BuildNetworkParams {
			config: &config,
			client: client.clone(),
			transaction_pool: transaction_pool.clone(),
			spawn_handle: task_manager.spawn_handle(),
			import_queue,
			block_announce_validator_builder: None,
			warp_sync: None,
		})?;

	if config.offchain_worker.enabled {
		sc_service::build_offchain_workers(
			&config,
			task_manager.spawn_handle(),
			client.clone(),
			network.clone(),
		);
	}

	let role = config.role.clone();
	let force_authoring = config.force_authoring;
	let backoff_authoring_blocks: Option<()> = None;
	let name = config.network.node_name.clone();
	let enable_grandpa = !config.disable_grandpa;
	let prometheus_registry = config.prometheus_registry().cloned();
	let is_authority = config.role.is_authority();
	let (fee_history_cache, fee_history_cache_limit) = fee_history;

	let rpc_extensions_builder = {
		let client = client.clone();
		let pool = transaction_pool.clone();
		let network = network.clone();
		let frontier_backend = frontier_backend.clone();
		let overrides = crate::rpc::overrides_handle(client.clone());
		let fee_history_cache = fee_history_cache.clone();
		let block_data_cache = Arc::new(fc_rpc::EthBlockDataCacheTask::new(
			task_manager.spawn_handle(),
			overrides.clone(),
			50,
			50,
			prometheus_registry.clone(),
		));

		Box::new(move |deny_unsafe, _| {
			let deps =
				crate::rpc::FullDeps {
					client: client.clone(),
					pool: pool.clone(),
					graph: pool.pool().clone(),
					deny_unsafe,
					is_authority,
					network: network.clone(),
					backend: frontier_backend.clone(),
					block_data_cache: block_data_cache.clone(),
					fee_history_cache: fee_history_cache.clone(),
					fee_history_cache_limit,
				};
			crate::rpc::create_full(deps).map_err(Into::into)
		})
	};

	let _rpc_handlers = sc_service::spawn_tasks(sc_service::SpawnTasksParams {
		network: network.clone(),
		client: client.clone(),
		keystore: keystore_container.sync_keystore(),
		task_manager: &mut task_manager,
		transaction_pool: transaction_pool.clone(),
		rpc_builder: rpc_extensions_builder,
		backend: backend.clone(),
		system_rpc_tx,
		tx_handler_controller,
		config,
		telemetry: telemetry.as_mut(),
	})?;

	task_manager.spawn_essential_handle().spawn(
		"frontier-mapping-sync-worker", None,
		MappingSyncWorker::new(
			client.import_notification_stream(),
			Duration::new(6, 0),    // kick off the sync worker every 6 seconds
			client.clone(),
			backend.clone(),
			frontier_backend.clone(),
			3,
			0,
			SyncStrategy::Normal,
		)
			.for_each(|()| future::ready(())),
	);


	let proposer_factory = sc_basic_authorship::ProposerFactory::new(
		task_manager.spawn_handle(),
		client.clone(),
		transaction_pool,
		prometheus_registry.as_ref(),
		telemetry.as_ref().map(|x| x.handle()),
	);

	// let can_author_with =
	// 	sp_consensus::CanAuthorWithNativeVersion::new(client.executor().clone());
	let select_chain = sc_consensus::LongestChain::new(backend.clone());

	let address = sp_runtime::MultiSigner::from(sp_keyring::Sr25519Keyring::Alice.public())
		.into_account()
		.encode();

	let (_worker, worker_task) = sc_consensus_pow::start_mining_worker(
		Box::new(pow_block_import),
		client,
		select_chain,
		MinimalSha3Algorithm,
		proposer_factory, network.clone(), network.clone(),
		Some(address),
		move |_, ()| async move {
			let provider = sp_timestamp::InherentDataProvider::from_system_time();
			Ok(provider)
		},
		Duration::new(2, 0),
		Duration::new(10, 0),
	);

	// the AURA authoring task is considered essential, i.e. if it
	// fails we take down the service with it.
	task_manager
		.spawn_essential_handle()
		.spawn("pow", Some("block-authoring"), worker_task);


	network_starter.start_network();
	Ok(task_manager)
}
