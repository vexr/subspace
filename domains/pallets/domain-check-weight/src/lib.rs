// Copyright (C) 2024 Subspace Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![cfg_attr(not(feature = "std"), no_std)]

use frame_support::dispatch::{DispatchInfo, PostDispatchInfo};
use frame_support::traits::Get;
use frame_system::limits::BlockWeights;
use frame_system::{Config, ConsumedWeight};
use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;
use sp_runtime::DispatchResult;
use sp_runtime::traits::{
    DispatchInfoOf, DispatchOriginOf, Dispatchable, PostDispatchInfoOf, TransactionExtension,
    ValidateResult,
};
use sp_runtime::transaction_validity::{
    TransactionSource, TransactionValidity, TransactionValidityError,
};
use sp_weights::Weight;

/// Wrapper of [`frame_system::CheckWeight`]
///
/// It performs the same check as [`frame_system::CheckWeight`] except the `max_total/max_block` weight limit
/// check is removed from the `pre_dispatch/pre_dispatch_unsigned` because the total weight of a domain block
/// is based on probability instead of a hard limit.
#[derive(Encode, Decode, Clone, Eq, PartialEq, Default, TypeInfo)]
#[scale_info(skip_type_params(T))]
pub struct CheckWeight<T: Config + Send + Sync>(core::marker::PhantomData<T>);

impl<T: Config + Send + Sync> CheckWeight<T>
where
    T::RuntimeCall: Dispatchable<Info = DispatchInfo, PostInfo = PostDispatchInfo>,
{
    /// Creates new `SignedExtension` to check weight of the extrinsic.
    pub fn new() -> Self {
        Self(Default::default())
    }

    /// Check the block length and the max extrinsic weight and notes the new weight and length value.
    ///
    /// It is same as the [`frame_system::CheckWeight::do_prepare`] except the `max_total/max_block`
    /// weight limit check is removed.
    pub fn do_prepare(
        info: &DispatchInfoOf<T::RuntimeCall>,
        len: usize,
        next_len: u32,
    ) -> Result<(), TransactionValidityError> {
        let all_weight = frame_system::Pallet::<T>::block_weight();
        let maximum_weight = T::BlockWeights::get();
        let next_weight =
            calculate_consumed_weight::<T::RuntimeCall>(&maximum_weight, all_weight, info, len);

        frame_system::AllExtrinsicsLen::<T>::put(next_len);
        frame_system::BlockWeight::<T>::put(next_weight);

        Ok(())
    }
}

/// Calculate the new block weight value with the given extrinsic
fn calculate_consumed_weight<Call>(
    maximum_weight: &BlockWeights,
    mut all_weight: ConsumedWeight,
    info: &DispatchInfoOf<Call>,
    len: usize,
) -> ConsumedWeight
where
    Call: Dispatchable<Info = DispatchInfo, PostInfo = PostDispatchInfo>,
{
    // Also Consider extrinsic length as proof weight.
    let extrinsic_weight = (info.call_weight + info.extension_weight)
        .saturating_add(maximum_weight.get(info.class).base_extrinsic)
        .saturating_add(Weight::from_parts(0, len as u64));

    // Saturating add the weight
    all_weight.accrue(extrinsic_weight, info.class);

    all_weight
}

impl<T: Config + Send + Sync> TransactionExtension<T::RuntimeCall> for CheckWeight<T>
where
    T::RuntimeCall: Dispatchable<Info = DispatchInfo, PostInfo = PostDispatchInfo>,
{
    type Implicit = ();
    type Pre = ();
    type Val = u32;
    const IDENTIFIER: &'static str = "CheckWeight";

    fn weight(&self, _: &T::RuntimeCall) -> Weight {
        <T::ExtensionsWeightInfo as frame_system::ExtensionsWeightInfo>::check_weight()
    }

    fn validate(
        &self,
        origin: T::RuntimeOrigin,
        _call: &T::RuntimeCall,
        info: &DispatchInfoOf<T::RuntimeCall>,
        len: usize,
        _self_implicit: Self::Implicit,
        _inherited_implication: &impl Encode,
        _source: TransactionSource,
    ) -> ValidateResult<Self::Val, T::RuntimeCall> {
        let (validity, next_len) = frame_system::CheckWeight::<T>::do_validate(info, len)?;
        Ok((validity, next_len, origin))
    }

    fn prepare(
        self,
        val: Self::Val,
        _origin: &DispatchOriginOf<T::RuntimeCall>,
        _call: &T::RuntimeCall,
        info: &DispatchInfoOf<T::RuntimeCall>,
        len: usize,
    ) -> Result<Self::Pre, TransactionValidityError> {
        Self::do_prepare(info, len, val)
    }

    fn post_dispatch_details(
        _pre: Self::Pre,
        info: &DispatchInfoOf<T::RuntimeCall>,
        post_info: &PostDispatchInfoOf<T::RuntimeCall>,
        _len: usize,
        _result: &DispatchResult,
    ) -> Result<Weight, TransactionValidityError> {
        frame_system::CheckWeight::<T>::do_post_dispatch(info, post_info)?;
        Ok(Weight::zero())
    }

    fn bare_validate(
        _call: &T::RuntimeCall,
        info: &DispatchInfoOf<T::RuntimeCall>,
        len: usize,
    ) -> TransactionValidity {
        Ok(frame_system::CheckWeight::<T>::do_validate(info, len)?.0)
    }

    fn bare_validate_and_prepare(
        _call: &T::RuntimeCall,
        info: &DispatchInfoOf<T::RuntimeCall>,
        len: usize,
    ) -> Result<(), TransactionValidityError> {
        let (_, next_len) = frame_system::CheckWeight::<T>::do_validate(info, len)?;
        Self::do_prepare(info, len, next_len)
    }

    fn bare_post_dispatch(
        info: &DispatchInfoOf<T::RuntimeCall>,
        post_info: &mut PostDispatchInfoOf<T::RuntimeCall>,
        _len: usize,
        _result: &DispatchResult,
    ) -> Result<(), TransactionValidityError> {
        frame_system::CheckWeight::<T>::do_post_dispatch(info, post_info)
    }
}

impl<T: Config + Send + Sync> core::fmt::Debug for CheckWeight<T> {
    #[cfg(feature = "std")]
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "CheckWeight")
    }

    #[cfg(not(feature = "std"))]
    fn fmt(&self, _: &mut core::fmt::Formatter) -> core::fmt::Result {
        Ok(())
    }
}
