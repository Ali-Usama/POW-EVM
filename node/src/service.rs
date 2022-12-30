//! Service and ServiceFactory implementation. Specialized wrapper over substrate service.

use node_template_runtime::{self, opaque::Block, RuntimeApi};
use sc_client_api::{BlockBackend, BlockchainEvents};
// use sc_consensus_aura::{ImportQueueParams, SlotProportion, StartAuraParams};
pub use sc_executor::NativeElseWasmExecutor;
use sc_keystore::LocalKeystore;
use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};
use sc_service::{error::Error as ServiceError, Configuration, TaskManager, BasePath};
use sc_telemetry::{Telemetry, TelemetryWorker};
// use sp_consensus_aura::sr25519::AuthorityPair as AuraPair;
use std::{
	future, thread, sync::{Arc, Mutex}, time::Duration, collections::BTreeMap, path::PathBuf,
	str::FromStr,
};
use fc_mapping_sync::{MappingSyncWorker, SyncStrategy};
use fc_rpc_core::types::{FeeHistoryCache, FeeHistoryCacheLimit};
use fc_consensus::FrontierBlockImport;
// use sp_consensus::CanAuthorWithNativeNativeVersion;
use futures::StreamExt;
use sp_core::{
	Encode, U256, H256, crypto::{Ss58AddressFormat, Ss58AddressFormatRegistry, UncheckedFrom, Ss58Codec},
	Pair,
};
use sha3_pow::*;
use frame_benchmarking::log::*;

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
	Sha3Algorithm<FullClient>,
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

	let algorithm = Sha3Algorithm::new(client.clone());
	let pow_block_import = sc_consensus_pow::PowBlockImport::new(
		client.clone(),
		client.clone(),
		algorithm.clone(),
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
		algorithm,
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

pub fn decode_author(
	author: Option<&str>,
	keystore: SyncCryptoStorePtr,
	keystore_path: Option<PathBuf>,
) -> Result<sha3_pow::app::Public, String> {
	if let Some(author) = author {
		if author.starts_with("0x") {
			Ok(sha3_pow::app::Public::unchecked_from(
				H256::from_str(&author[2..]).map_err(|_| "Invalid author account".to_string())?,
			)
				.into())
		} else {
			// This line compiles if sp_core::crypto std feature is enabled
			let (address, version) = sha3_pow::app::Public::from_ss58check_with_version(author)
				.map_err(|_| "Invalid author address".to_string())?;
			if version != Ss58AddressFormat::from(Ss58AddressFormatRegistry::BareSr25519Account) {
				return Err("Invalid author version".to_string());
			}
			Ok(address)
		}
	} else {
		info!("The node is configured for mining, but no author key is provided.");

		// This line compiles if sp_application_crypto std feature is enabled
		let (pair, phrase, _) = sha3_pow::app::Pair::generate_with_phrase(None);

		SyncCryptoStore::insert_unknown(
			&*keystore.as_ref(),
			sha3_pow::app::ID,
			&phrase,
			pair.public().as_ref(),
		)
			.map_err(|e| format!("Registering mining key failed: {:?}", e))?;

		info!(
			"Generated a mining key with address: {}",
			pair.public()
				.to_ss58check_with_version(Ss58AddressFormat::from(Ss58AddressFormatRegistry::BareSr25519Account))
		);

		match keystore_path {
			Some(path) => info!("You can go to {:?} to find the seed phrase of the mining key.", path),
			None => warn!("Keystore is not local. This means that your mining key will be lost when exiting the program. This should only happen if you are in dev mode."),
		}

		Ok(pair.public())
	}
}

/// Builds a new service for a full client.
pub fn new_full(
	mut config: Configuration, author: Option<&str>
) -> Result<TaskManager, ServiceError> {
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
	
	let prometheus_registry = config.prometheus_registry().cloned();
	let is_authority = config.role.is_authority();
	let (fee_history_cache, fee_history_cache_limit) = fee_history;
	let keystore_path = config.keystore.path().map(|p| p.to_owned());

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

	let algorithm = Sha3Algorithm::new(client.clone());
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

	let author = decode_author(author, keystore_container.sync_keystore(), keystore_path)?;

	let (_worker, worker_task) = sc_consensus_pow::start_mining_worker(
		Box::new(pow_block_import),
		client,
		select_chain,
		algorithm.clone(),
		proposer_factory, network.clone(), network.clone(),
		Some(author.encode()),
		move |_, ()| async move {
			let provider = sp_timestamp::InherentDataProvider::from_system_time();
			Ok(provider)
		},
		// Time to wait for a new block before starting to mine a new one
		Duration::new(10, 0),
		// How long to take to actually build the block (i.e. executing extrinsic)
		Duration::new(10, 0),
	);

	// the AURA authoring task is considered essential, i.e. if it
	// fails we take down the service with it.
	task_manager
		.spawn_essential_handle()
		.spawn("pow", Some("block-authoring"), worker_task);

	// Start Mining
	// (from recipes a bit modified)
	let mut nonce: U256 = U256::from(0);
	thread::spawn(move || loop {
		let worker = _worker.clone();
		let metadata = worker.metadata();
		if let Some(metadata) = metadata {
			let compute = Compute {
				difficulty: metadata.difficulty,
				pre_hash: metadata.pre_hash,
				nonce,
			};
			let seal = compute.compute();
			if hash_meets_difficulty(&seal.work, seal.difficulty) {
				nonce = U256::from(0);
				let _ = futures::executor::block_on(worker.submit(seal.encode()));
			} else {
				nonce = nonce.saturating_add(U256::from(1));
				if nonce == U256::MAX {
					nonce = U256::from(0);
				}
			}
		} else {
			thread::sleep(Duration::new(1, 0));
		}
	});


	network_starter.start_network();
	Ok(task_manager)
}
