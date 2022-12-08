#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::{
		pallet_prelude::*,
		sp_std,
		traits::{Currency, FindAuthor},
		ConsensusEngineId,
	};
	use frame_system::pallet_prelude::*;

	#[pallet::config]
	pub trait Config: frame_system::Config + pallet_balances::Config {
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// concrete currency implementataion
		type Currency: Currency<Self::AccountId>;
	}

	#[pallet::storage]
	#[pallet::getter(fn block_reward)]
	// Learn more about declaring storage items:
	// https://substrate.dev/docs/en/knowledgebase/runtime/storage#declaring-storage-items
	pub type Reward<T: Config> = StorageValue<_, <T::Currency as Currency<T::AccountId>>::Balance>;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A block author  has just been rewarded.
		/// [AccountId]
		AuthorRewarded(T::AccountId),
		/// Block reward has just been updated
		RewardUpdated(<T::Currency as Currency<T::AccountId>>::Balance),
	}

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		pub reward: <T::Currency as Currency<T::AccountId>>::Balance,
	}

	#[cfg(feature = "std")]
	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			GenesisConfig { reward: <T::Currency as Currency<T::AccountId>>::Balance::from(100u8) }
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			Reward::<T>::put(self.reward)
		}
	}

	/// A type for representing the validator id in a session.
	pub type ValidatorId<T> = <T as frame_system::Config>::AccountId;

	/// Keep track of number of authored blocks per authority, uncles are counted as
	/// well since they're a valid proof of being online.
	impl<T: Config + pallet_authorship::Config>
		pallet_authorship::EventHandler<ValidatorId<T>, T::BlockNumber> for Pallet<T>
	{
		fn note_author(author: ValidatorId<T>) {
			if let Some(reward) = Reward::<T>::get() {
				T::Currency::deposit_creating(&author, reward);
				Self::deposit_event(Event::AuthorRewarded(author));
			}
		}

		fn note_uncle(_author: ValidatorId<T>, _age: T::BlockNumber) {}
	}

	pub struct FindAuthorFromDigests<T>(sp_std::marker::PhantomData<T>);

	impl<T: Config> FindAuthor<T::AccountId> for FindAuthorFromDigests<T> {
		fn find_author<'a, I>(digests: I) -> Option<T::AccountId>
		where
			I: 'a + IntoIterator<Item = (ConsensusEngineId, &'a [u8])>,
		{
			digests.into_iter().find_map(|item| match item {
				(_, author) => T::AccountId::decode(&mut &author[..]).ok(),
			})
		}
	}

	// Dispatchable functions allows users to interact with the pallet and invoke state changes.
	// These functions materialize as "extrinsics", which are often compared to transactions.
	// Dispatchable functions must be annotated with a weight and must return a DispatchResult.
	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// An example dispatchable that takes a singles value as a parameter, writes the value to
		/// storage and emits an event. This function must be dispatched by a signed extrinsic.
		#[pallet::weight(10_000 + T::DbWeight::get().writes(1).ref_time())]
		pub fn set_reward(
			origin: OriginFor<T>,
			reward: <T::Currency as Currency<T::AccountId>>::Balance,
		) -> DispatchResult {
			// only root origins allowed
			ensure_root(origin)?;

			// Update storage.
			<Reward<T>>::put(reward);

			// Emit an event.
			Self::deposit_event(Event::RewardUpdated(reward));
			Ok(())
		}
	}
}
