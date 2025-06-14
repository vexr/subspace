//! Contract creation allow list implementations

use crate::traits::{AccountIdFor, MaybeIntoEthCall, MaybeIntoEvmCall};
use domain_runtime_primitives::{ERR_CONTRACT_CREATION_NOT_ALLOWED, EthereumAccountId};
use frame_support::pallet_prelude::{PhantomData, TypeInfo};
use frame_system::pallet_prelude::{OriginFor, RuntimeCallFor};
use pallet_ethereum::{Transaction as EthereumTransaction, TransactionAction};
use parity_scale_codec::{Decode, Encode};
use scale_info::prelude::fmt;
use sp_core::Get;
use sp_runtime::impl_tx_ext_default;
use sp_runtime::traits::{
    AsSystemOriginSigner, DispatchInfoOf, Dispatchable, TransactionExtension, ValidateResult,
};
use sp_runtime::transaction_validity::{
    InvalidTransaction, TransactionSource, TransactionValidity, TransactionValidityError,
    ValidTransaction,
};
use sp_weights::Weight;
use subspace_runtime_primitives::utility::{MaybeNestedCall, nested_call_iter};

/// Rejects contracts that can't be created under the current allow list.
/// Returns false if the call is a contract call, and the account is *not* allowed to call it.
/// Otherwise, returns true.
pub fn is_create_contract_allowed<Runtime>(
    call: &RuntimeCallFor<Runtime>,
    signer: &EthereumAccountId,
) -> bool
where
    Runtime: frame_system::Config<AccountId = EthereumAccountId>
        + pallet_ethereum::Config
        + pallet_evm::Config
        + crate::Config,
    RuntimeCallFor<Runtime>:
        MaybeIntoEthCall<Runtime> + MaybeIntoEvmCall<Runtime> + MaybeNestedCall<Runtime>,
    Result<pallet_ethereum::RawOrigin, OriginFor<Runtime>>: From<OriginFor<Runtime>>,
{
    // If the account is allowed to create contracts, or it's not a contract call, return true.
    // Only enters allocating code if this account can't create contracts.
    crate::Pallet::<Runtime>::is_allowed_to_create_contracts(signer)
        || !is_create_contract::<Runtime>(call)
}

/// If anyone is allowed to create contracts, allows contracts. Otherwise, rejects contracts.
/// Returns false if the call is a contract call, and there is a specific (possibly empty) allow
/// list. Otherwise, returns true.
pub fn is_create_unsigned_contract_allowed<Runtime>(call: &RuntimeCallFor<Runtime>) -> bool
where
    Runtime: frame_system::Config + pallet_ethereum::Config + pallet_evm::Config + crate::Config,
    RuntimeCallFor<Runtime>:
        MaybeIntoEthCall<Runtime> + MaybeIntoEvmCall<Runtime> + MaybeNestedCall<Runtime>,
    Result<pallet_ethereum::RawOrigin, OriginFor<Runtime>>: From<OriginFor<Runtime>>,
{
    // If any account is allowed to create contracts, or it's not a contract call, return true.
    // Only enters allocating code if there is a contract creation filter.
    crate::Pallet::<Runtime>::is_allowed_to_create_unsigned_contracts()
        || !is_create_contract::<Runtime>(call)
}

/// Returns true if the call is a contract creation call.
pub fn is_create_contract<Runtime>(call: &RuntimeCallFor<Runtime>) -> bool
where
    Runtime: frame_system::Config + pallet_ethereum::Config + pallet_evm::Config,
    RuntimeCallFor<Runtime>:
        MaybeIntoEthCall<Runtime> + MaybeIntoEvmCall<Runtime> + MaybeNestedCall<Runtime>,
    Result<pallet_ethereum::RawOrigin, OriginFor<Runtime>>: From<OriginFor<Runtime>>,
{
    for call in nested_call_iter::<Runtime>(call) {
        if let Some(call) = call.maybe_into_eth_call() {
            match call {
                pallet_ethereum::Call::transact {
                    transaction: EthereumTransaction::Legacy(transaction),
                    ..
                } => {
                    if transaction.action == TransactionAction::Create {
                        return true;
                    }
                }
                pallet_ethereum::Call::transact {
                    transaction: EthereumTransaction::EIP2930(transaction),
                    ..
                } => {
                    if transaction.action == TransactionAction::Create {
                        return true;
                    }
                }
                pallet_ethereum::Call::transact {
                    transaction: EthereumTransaction::EIP1559(transaction),
                    ..
                } => {
                    if transaction.action == TransactionAction::Create {
                        return true;
                    }
                }
                // Inconclusive, other calls might create contracts.
                _ => {}
            }
        }

        if let Some(pallet_evm::Call::create { .. } | pallet_evm::Call::create2 { .. }) =
            call.maybe_into_evm_call()
        {
            return true;
        }
    }

    false
}

/// Reject contract creation, unless the account is in the current evm contract allow list.
#[derive(Debug, Encode, Decode, Clone, Eq, PartialEq, TypeInfo)]
pub struct CheckContractCreation<Runtime>(PhantomData<Runtime>);

impl<Runtime> CheckContractCreation<Runtime> {
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<Runtime> Default for CheckContractCreation<Runtime> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Runtime> CheckContractCreation<Runtime>
where
    Runtime: frame_system::Config<AccountId = EthereumAccountId>
        + pallet_ethereum::Config
        + pallet_evm::Config
        + crate::Config
        + scale_info::TypeInfo
        + fmt::Debug
        + Send
        + Sync,
    RuntimeCallFor<Runtime>:
        MaybeIntoEthCall<Runtime> + MaybeIntoEvmCall<Runtime> + MaybeNestedCall<Runtime>,
    Result<pallet_ethereum::RawOrigin, OriginFor<Runtime>>: From<OriginFor<Runtime>>,
    <RuntimeCallFor<Runtime> as Dispatchable>::RuntimeOrigin:
        AsSystemOriginSigner<AccountIdFor<Runtime>> + Clone,
{
    fn do_validate_unsigned(call: &RuntimeCallFor<Runtime>) -> TransactionValidity {
        if !is_create_unsigned_contract_allowed::<Runtime>(call) {
            Err(InvalidTransaction::Custom(ERR_CONTRACT_CREATION_NOT_ALLOWED).into())
        } else {
            Ok(ValidTransaction::default())
        }
    }

    fn do_validate(
        origin: &OriginFor<Runtime>,
        call: &RuntimeCallFor<Runtime>,
    ) -> TransactionValidity {
        let Some(who) = origin.as_system_origin_signer() else {
            // Reject unsigned contract creation unless anyone is allowed to create them.
            return Self::do_validate_unsigned(call);
        };
        // Reject contract creation unless the account is in the allow list.
        if !is_create_contract_allowed::<Runtime>(call, who) {
            Err(InvalidTransaction::Custom(ERR_CONTRACT_CREATION_NOT_ALLOWED).into())
        } else {
            Ok(ValidTransaction::default())
        }
    }
}

// Unsigned calls can't create contracts. Only pallet-evm and pallet-ethereum can create contracts.
// For pallet-evm all contracts are signed extrinsics, for pallet-ethereum there is only one
// extrinsic that is self-contained.
impl<Runtime> TransactionExtension<RuntimeCallFor<Runtime>> for CheckContractCreation<Runtime>
where
    Runtime: frame_system::Config<AccountId = EthereumAccountId>
        + pallet_ethereum::Config
        + pallet_evm::Config
        + crate::Config
        + scale_info::TypeInfo
        + fmt::Debug
        + Send
        + Sync,
    RuntimeCallFor<Runtime>:
        MaybeIntoEthCall<Runtime> + MaybeIntoEvmCall<Runtime> + MaybeNestedCall<Runtime>,
    Result<pallet_ethereum::RawOrigin, OriginFor<Runtime>>: From<OriginFor<Runtime>>,
    <RuntimeCallFor<Runtime> as Dispatchable>::RuntimeOrigin:
        AsSystemOriginSigner<AccountIdFor<Runtime>> + Clone,
{
    const IDENTIFIER: &'static str = "CheckContractCreation";
    type Implicit = ();
    type Val = ();
    type Pre = ();

    // TODO: calculate proper weight for this extension
    //  Currently only accounts for storage read
    fn weight(&self, _: &RuntimeCallFor<Runtime>) -> Weight {
        // there will always be one storage read for this call
        <Runtime as frame_system::Config>::DbWeight::get().reads(1)
    }

    fn validate(
        &self,
        origin: OriginFor<Runtime>,
        call: &RuntimeCallFor<Runtime>,
        _info: &DispatchInfoOf<RuntimeCallFor<Runtime>>,
        _len: usize,
        _self_implicit: Self::Implicit,
        _inherited_implication: &impl Encode,
        _source: TransactionSource,
    ) -> ValidateResult<Self::Val, RuntimeCallFor<Runtime>> {
        let validity = Self::do_validate(&origin, call)?;
        Ok((validity, (), origin))
    }

    impl_tx_ext_default!(RuntimeCallFor<Runtime>; prepare);

    fn bare_validate(
        call: &RuntimeCallFor<Runtime>,
        _info: &DispatchInfoOf<RuntimeCallFor<Runtime>>,
        _len: usize,
    ) -> TransactionValidity {
        Self::do_validate_unsigned(call)
    }

    fn bare_validate_and_prepare(
        call: &RuntimeCallFor<Runtime>,
        _info: &DispatchInfoOf<RuntimeCallFor<Runtime>>,
        _len: usize,
    ) -> Result<(), TransactionValidityError> {
        Self::do_validate_unsigned(call)?;
        Ok(())
    }
}
