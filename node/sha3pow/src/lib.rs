use codec::{Decode, Encode};
use sc_consensus_pow::{Error as PowError, PowAlgorithm};
use sha3::{Digest, Sha3_256};
use sp_api::ProvideRuntimeApi;
use sc_client_api::{backend::AuxStore, blockchain::HeaderBackend};
use sp_consensus_pow::{DifficultyApi, Seal as RawSeal};
use sp_core::{blake2_256, H256, U256};
use sp_runtime::generic::BlockId;
use sp_runtime::traits::Block as BlockT;
use std::sync::Arc;

// Exported module of the whole app
pub mod app {
	use sp_application_crypto::{app_crypto, sr25519};
	use sp_core::crypto::KeyTypeId;

	pub const ID: KeyTypeId = KeyTypeId(*b"crn1");

	app_crypto!(sr25519, ID);
}


// Check if the given hash satisfies the given difficulty
// Multiply both together. If the product overflows the bounds of U256 then
// the hash was to high
pub fn hash_meets_difficulty(hash: &H256, difficulty: U256) -> bool {
	let num_hash = U256::from(&hash[..]);
	let (_, overflowed) = num_hash.overflowing_mul(difficulty);

	!overflowed
}


// A Seal struct to encode as Vec<u8> and use as the 'RawSeal' type
#[derive(Clone, PartialEq, Eq, Encode, Decode, Debug)]
pub struct Seal {
	pub difficulty: U256,
	pub work: H256,
	pub nonce: U256,
}


// An attempt to solve a PoW
#[derive(Clone, PartialEq, Eq, Encode, Decode, Debug)]
pub struct Compute {
	pub difficulty: U256,
	pub pre_hash: H256,
	pub nonce: U256,
}

impl Compute {
	pub fn compute(self) -> Seal {
		let work = H256::from_slice(Sha3_256::digest(&self.encode()[..]).as_slice());

		Seal {
			nonce: self.nonce,
			difficulty: self.difficulty,
			work,
		}
	}
}


// A simple Sha3 hashing algorithm
pub struct Sha3Algorithm<C> {
	client: Arc<C>,
}

impl<C> Sha3Algorithm<C> {
	pub fn new(client: Arc<C>) -> Self {
		Self { client }
	}
}

impl<C> Clone for Sha3Algorithm<C> {
	fn clone(&self) -> Self {
		Self::new(self.client.clone())
	}
}


// Implementing PowAlgorithm trait is a must
impl<B: BlockT<Hash=H256>, C> PowAlgorithm<B> for Sha3Algorithm<C>
	where
		C: HeaderBackend<B> + AuxStore + ProvideRuntimeApi<B>,
		C::Api: DifficultyApi<B, U256>,
{
	type Difficulty = U256;

	// Get the next block's difficulty
	fn difficulty(&self, parent: B::Hash) -> Result<Self::Difficulty, PowError<B>> {
		let parent_id = BlockId::<B>::hash(parent);
		let difficulty = self
			.client
			.runtime_api()
			.difficulty(&parent_id)
			.map_err(|e| {
				PowError::Environment(format!("Fetching difficulty from runtime failed: {:?}", e))
			});

		difficulty
	}

	// Break a tie situation when choosing a chain fork
	fn break_tie(&self, own_seal: &RawSeal, new_seal: &RawSeal) -> bool {
		blake2_256(&own_seal[..]) > blake2_256(&new_seal[..])
	}


	// Verify that the difficulty is valid against given seal
	fn verify(
		&self,
		_parent: &BlockId<B>,
		pre_hash: &H256,
		_pre_digest: Option<&[u8]>,
		seal: &RawSeal,
		difficulty: Self::Difficulty,
	) -> Result<bool, PowError<B>> {
		// Try to construct a seal object by decoding the raw seal given
		let seal = match Seal::decode(&mut &seal[..]) {
			Ok(seal) => seal,
			Err(_) => return Ok(false),
		};

		// Check if hash meets the difficulty
		if !hash_meets_difficulty(&seal.work, difficulty) {
			return Ok(false);
		}

		// Check if the provided work comes from the correct pre_hash
		let compute = Compute {
			difficulty,
			pre_hash: *pre_hash,
			nonce: seal.nonce,
		};

		if compute.compute() != seal {
			return Ok(false);
		}

		Ok(true)
	}

	fn preliminary_verify(
		&self,
		_pre_hash: &<B as BlockT>::Hash,
		_seal: &RawSeal,
	) -> Result<Option<bool>, PowError<B>> {
		Ok(None)
	}
}
