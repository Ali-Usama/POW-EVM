//! Benchmarking setup for pallet-template

use super::*;

#[allow(unused)]
use crate::Pallet as Template;
use frame_benchmarking::{benchmarks, impl_benchmark_test_suite, whitelisted_caller};
use frame_system::RawOrigin;

benchmarks! {
	set_difficulty {
		let s in 0 .. 100;
	}: _(RawOrigin::Root, s)
	verify {
		assert_eq!(Difficulty::<T>::get(), Some(s));
	}
}

impl_benchmark_test_suite!(Template, crate::mock::new_test_ext(), crate::mock::Test);
