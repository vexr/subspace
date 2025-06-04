//! Benchmarking for `BalanceTransferCheck` extensions.

use crate::extension::{
    BalanceTransferCheckExtension, BalanceTransferChecks, MaybeBalancesCall, MaybeNestedCall,
};
use core::marker::PhantomData;
use frame_benchmarking::v2::*;
use frame_support::dispatch::{DispatchInfo, PostDispatchInfo};
use frame_system::Config;
use frame_system::pallet_prelude::RuntimeCallFor;
use pallet_balances::{AdjustmentDirection, Call as BalancesCall, Config as BalancesConfig};
use pallet_multisig::{Call as MultisigCall, Config as MultisigConfig};
use pallet_utility::{Call as UtilityCall, Config as UtilityConfig};
use scale_info::prelude::boxed::Box;
use scale_info::prelude::vec::Vec;
use scale_info::prelude::{fmt, vec};
use sp_runtime::Weight;
use sp_runtime::traits::{Dispatchable, StaticLookup};

pub struct Pallet<T: BalancesConfig + UtilityConfig + MultisigConfig>(PhantomData<T>);

const SEED: u32 = 0;

#[allow(clippy::multiple_bound_locations)]
#[benchmarks(where
	T: Send + Sync + scale_info::TypeInfo + fmt::Debug +
        UtilityConfig + BalanceTransferChecks + BalancesConfig + MultisigConfig,
    <T as BalancesConfig>::Balance: From<u128>,
    RuntimeCallFor<T>:
        Dispatchable<Info = DispatchInfo, PostInfo = PostDispatchInfo> +
        From<UtilityCall<T>> + From<BalancesCall<T>> + From<MultisigCall<T>> +
        Into<<T as MultisigConfig>::RuntimeCall> +
        MaybeBalancesCall<T> + MaybeNestedCall<T>)
]
mod benchmarks {
    use super::*;
    use frame_system::pallet_prelude::RuntimeCallFor;

    #[benchmark]
    fn balance_transfer_check_multiple(c: Linear<0, 1000>) {
        let mut calls = Vec::with_capacity(c as usize + 1);
        for i in 0..=c {
            // Non-balance calls are more expensive to check, because we have to read them all.
            // (We can only exit the check loop early if we encounter a balance transfer call.)
            calls.push(construct_non_balance_call::<T>(i));
        }

        let call = construct_utility_call_list::<T>(calls, c);

        #[block]
        {
            BalanceTransferCheckExtension::<T>::do_validate_signed(&call).unwrap();
        }
    }

    #[benchmark]
    fn balance_transfer_check_utility(c: Linear<0, 1000>) {
        let mut call = construct_balance_call::<T>(c);
        for i in 0..=c {
            call = construct_utility_call::<T>(call, i);
        }
        #[block]
        {
            BalanceTransferCheckExtension::<T>::do_validate_signed(&call).unwrap();
        }
    }

    #[benchmark]
    fn balance_transfer_check_multisig(c: Linear<0, 1000>) {
        let mut call = construct_balance_call::<T>(c);
        for i in 0..=c {
            call = construct_multisig_call::<T>(call, i);
        }
        #[block]
        {
            BalanceTransferCheckExtension::<T>::do_validate_signed(&call).unwrap();
        }
    }
}

/// Construct a banned balance transfer call.
/// The variant argument is used to make the calls different, so benchmarks are more realistic.
fn construct_balance_call<T: BalancesConfig>(variant: u32) -> RuntimeCallFor<T>
where
    RuntimeCallFor<T>: From<BalancesCall<T>>,
    <T as BalancesConfig>::Balance: From<u128>,
{
    let recipient: T::AccountId = account("recipient", variant, SEED);
    let recipient_lookup = T::Lookup::unlookup(recipient.clone());

    match variant % 3 {
        0 => BalancesCall::transfer_allow_death {
            dest: recipient_lookup,
            value: (variant as u128 * 1000).into(),
        },
        1 => BalancesCall::transfer_keep_alive {
            dest: recipient_lookup,
            value: (variant as u128 * 2000).into(),
        },
        _ => BalancesCall::transfer_all {
            dest: recipient_lookup,
            keep_alive: variant % 2 == 0,
        },
    }
    .into()
}

/// Construct an accepted pallet-balances call.
/// The variant argument is used to make the calls different, so benchmarks are more realistic.
fn construct_non_balance_call<T: BalancesConfig>(variant: u32) -> RuntimeCallFor<T>
where
    RuntimeCallFor<T>: From<BalancesCall<T>>,
    <T as BalancesConfig>::Balance: From<u128>,
{
    let recipient: T::AccountId = account("recipient", variant, SEED);
    let recipient_lookup = T::Lookup::unlookup(recipient.clone());

    // We skip some force_* calls, to avoid confusion with actual balance transfers.
    match variant % 4 {
        0 => BalancesCall::upgrade_accounts {
            who: vec![recipient],
        },
        1 => BalancesCall::force_unreserve {
            who: recipient_lookup,
            amount: (variant as u128 * 3000).into(),
        },
        2 => BalancesCall::force_adjust_total_issuance {
            direction: AdjustmentDirection::Increase,
            delta: (variant as u128 * 4000).into(),
        },
        _ => BalancesCall::burn {
            value: (variant as u128 * 5000).into(),
            keep_alive: variant % 2 == 0,
        },
    }
    .into()
}

fn construct_utility_call<T: UtilityConfig>(
    call: RuntimeCallFor<T>,
    variant: u32,
) -> RuntimeCallFor<T>
where
    RuntimeCallFor<T>: From<UtilityCall<T>>,
{
    match variant % 5 {
        0 => UtilityCall::batch {
            calls: vec![call.into()],
        },
        1 => UtilityCall::as_derivative {
            index: variant as u16,
            call: Box::new(call.into()),
        },
        2 => UtilityCall::batch_all {
            calls: vec![call.into()],
        },
        3 => UtilityCall::force_batch {
            calls: vec![call.into()],
        },
        /* TODO: implement this, perhaps with a variant for `as_origin`?
        4 => UtilityCall::dispatch_as {
            as_origin: Box::new(Default::default()),
            call: Box::new(call.into()),
        },
        */
        _ => UtilityCall::with_weight {
            call: Box::new(call.into()),
            weight: Weight::from_parts(variant as u64, 0),
        },
    }
    .into()
}

/// Wrap `calls` in a pallet-utility call.
/// The variant argument is used to make the calls different, so benchmarks are more realistic.
fn construct_utility_call_list<T: UtilityConfig>(
    calls: Vec<RuntimeCallFor<T>>,
    variant: u32,
) -> RuntimeCallFor<T>
where
    RuntimeCallFor<T>: From<UtilityCall<T>>,
{
    let calls = calls.into_iter().map(Into::into).collect();

    match variant % 3 {
        0 => UtilityCall::batch { calls },
        1 => UtilityCall::batch_all { calls },
        _ => UtilityCall::force_batch { calls },
    }
    .into()
}

/// Wrap `calls` in a pallet-multisig call.
/// The variant argument is used to make the calls different, so benchmarks are more realistic.
fn construct_multisig_call<T: MultisigConfig>(
    call: RuntimeCallFor<T>,
    variant: u32,
) -> RuntimeCallFor<T>
where
    RuntimeCallFor<T>: From<MultisigCall<T>> + Into<<T as MultisigConfig>::RuntimeCall>,
{
    match variant % 2 {
        0 => MultisigCall::as_multi_threshold_1 {
            other_signatories: vec![],
            call: Box::new(call.into()),
        },
        _ => MultisigCall::as_multi {
            threshold: variant as u16,
            other_signatories: vec![],
            maybe_timepoint: None,
            call: Box::new(call.into()),
            max_weight: Weight::from_parts(variant as u64, 0),
        },
    }
    .into()
}
