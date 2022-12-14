use node_template_runtime::{
	AccountId, BalancesConfig, GrandpaConfig, GenesisConfig, Signature, SudoConfig, EVMConfig,
	SystemConfig, WASM_BINARY, GenesisAccount, EthereumConfig, TreasuryConfig, DifficultyConfig,
	RewardsConfig, UtxoConfig, utxo,
};
use sc_service::ChainType;
use hex_literal::hex;
// use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_core::{sr25519, Pair, Public, H160, U256, H256};
use sp_runtime::traits::{IdentifyAccount, Verify};
use std::{collections::BTreeMap, str::FromStr};
use sp_finality_grandpa::AuthorityId as GrandpaId;

// The URL for the telemetry server.
// const STAGING_TELEMETRY_URL: &str = "wss://telemetry.polkadot.io/submit/";

/// Specialized `ChainSpec`. This is a specialization of the general Substrate ChainSpec type.
pub type ChainSpec = sc_service::GenericChainSpec<GenesisConfig>;

/// Generate a crypto pair from seed.
pub fn get_from_seed<TPublic: Public>(seed: &str) -> <TPublic::Pair as Pair>::Public {
	TPublic::Pair::from_string(&format!("//{}", seed), None)
		.expect("static values are valid; qed")
		.public()
}

pub fn authority_keys_from_seed(s: &str) -> GrandpaId {
	get_from_seed::<GrandpaId>(s)
}

type AccountPublic = <Signature as Verify>::Signer;

/// Generate an account ID from seed.
pub fn get_account_id_from_seed<TPublic: Public>(seed: &str) -> AccountId
	where
		AccountPublic: From<<TPublic::Pair as Pair>::Public>,
{
	AccountPublic::from(get_from_seed::<TPublic>(seed)).into_account()
}


pub fn development_config() -> Result<ChainSpec, String> {
	let wasm_binary = WASM_BINARY.ok_or_else(|| "Development wasm not available".to_string())?;

	Ok(ChainSpec::from_genesis(
		// Name
		"Development",
		// ID
		"dev",
		ChainType::Development,
		move || {
			testnet_genesis(
				wasm_binary,
				// Sudo account
				get_account_id_from_seed::<sr25519::Public>("Alice"),
				vec![authority_keys_from_seed("Alice")],
				// Pre-funded accounts
				vec![
					get_account_id_from_seed::<sr25519::Public>("Alice"),
					get_account_id_from_seed::<sr25519::Public>("Bob"),
					get_account_id_from_seed::<sr25519::Public>("Alice//stash"),
					get_account_id_from_seed::<sr25519::Public>("Bob//stash"),
				],
				vec![
					get_from_seed::<sr25519::Public>("Alice"),
					get_from_seed::<sr25519::Public>("Bob"),
				],
				true,
			)
		},
		// Bootnodes
		vec![],
		// Telemetry
		None,
		// Protocol ID
		None,
		None,
		// Properties
		None,
		// Extensions
		None,
	))
}

pub fn local_testnet_config() -> Result<ChainSpec, String> {
	let wasm_binary = WASM_BINARY.ok_or_else(|| "Development wasm not available".to_string())?;

	Ok(ChainSpec::from_genesis(
		// Name
		"Local Testnet",
		// ID
		"local_testnet",
		ChainType::Local,
		move || {
			testnet_genesis(
				wasm_binary,
				// Sudo account
				get_account_id_from_seed::<sr25519::Public>("Alice"),
				vec![authority_keys_from_seed("Alice"), authority_keys_from_seed("Bob")],
				// Pre-funded accounts
				vec![
					get_account_id_from_seed::<sr25519::Public>("Alice"),
					get_account_id_from_seed::<sr25519::Public>("Bob"),
					get_account_id_from_seed::<sr25519::Public>("Charlie"),
					get_account_id_from_seed::<sr25519::Public>("Dave"),
					get_account_id_from_seed::<sr25519::Public>("Eve"),
					get_account_id_from_seed::<sr25519::Public>("Ferdie"),
					get_account_id_from_seed::<sr25519::Public>("Alice//stash"),
					get_account_id_from_seed::<sr25519::Public>("Bob//stash"),
					get_account_id_from_seed::<sr25519::Public>("Charlie//stash"),
					get_account_id_from_seed::<sr25519::Public>("Dave//stash"),
					get_account_id_from_seed::<sr25519::Public>("Eve//stash"),
					get_account_id_from_seed::<sr25519::Public>("Ferdie//stash"),
				],
				vec![
					get_from_seed::<sr25519::Public>("Alice"),
					get_from_seed::<sr25519::Public>("Bob"),
				],
				true,
			)
		},
		// Bootnodes
		vec![],
		// Telemetry
		None,
		// Protocol ID
		None,
		// Properties
		None,
		None,
		// Extensions
		None,
	))
}

/// Configure initial storage state for FRAME modules.
fn testnet_genesis(
	wasm_binary: &[u8],
	root_key: AccountId,
	initial_authorities: Vec<GrandpaId>,
	endowed_accounts: Vec<AccountId>,
	endowed_utxos: Vec<sr25519::Public>,
	_enable_println: bool,
) -> GenesisConfig {
	GenesisConfig {
		system: SystemConfig {
			// Add Wasm runtime to storage.
			code: wasm_binary.to_vec(),
		},
		balances: BalancesConfig {
			// Configure endowed accounts with initial balance of 1 << 60.
			balances: endowed_accounts.iter().cloned().map(|k| (k, 1 << 60)).collect(),
		},
		// aura: AuraConfig {
		// 	authorities: initial_authorities.iter().map(|x| (x.0.clone())).collect(),
		// },
		grandpa: GrandpaConfig {
			authorities: initial_authorities.iter().map(|x| (x.clone(), 1)).collect(),
			// authorities: vec![],
		},
		sudo: SudoConfig {
			// Assign network admin rights.
			key: Some(root_key),
		},
		transaction_payment: Default::default(),
		difficulty: DifficultyConfig {
			difficulty: 100_000.into()
		},
		rewards: RewardsConfig {
			reward: 100u128
		},
		treasury: TreasuryConfig {},
		evm: EVMConfig {
			accounts: {
				// Prefund the "ALICE" account
				let mut accounts = BTreeMap::new();
				accounts.insert(
					// H160 address of Alice dev account
					// Derived from SS58 (42 prefix) address
					// SS58: 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
					// hex: 0xd43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d
					// Using the full hex key, truncating to the first 20 bytes (the first 40 hex chars)
					H160::from_str("7ed8c8a0C4d1FeA01275fE13F0Ef23bce5CBF8C3")
						.expect("internal H160 is valid; qed"),
					GenesisAccount {
						balance: U256::from_str("0xffffffffffffffffffffffffffffffff")
							.expect("internal U256 is valid; qed"),
						code: vec![],
						nonce: U256::zero(),
						storage: BTreeMap::new(),
					},
				);

				accounts.insert(
					H160::from_slice(&hex!("6Be02d1d3665660d22FF9624b7BE0551ee1Ac91b")),
					GenesisAccount {
						nonce: U256::zero(),
						// Using a larger number, so I can tell the accounts apart by balance.
						balance: U256::from_str("0xffffffffffffffffffffffff")
							.expect("internal U256 is valid; qed"),
						code: vec![],
						storage: BTreeMap::new(),
					},
				);
				accounts
			}
		},
		ethereum: EthereumConfig {},
		base_fee: Default::default(),
		utxo: UtxoConfig {
			genesis_utxos: endowed_utxos
				.iter()
				.map(|x|
					utxo::TransactionOutput {
						value: 100 as utxo::Value,
						pubkey: H256::from_slice(x),
					}
				)
				.collect()
		},
	}
}
