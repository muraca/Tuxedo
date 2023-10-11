//! An implementation of a DAP fungible token.

#![cfg_attr(not(feature = "std"), no_std)]

use money::Coin;
use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;
#[cfg(feature = "std")]
use serde::{Deserialize, Serialize};
use sp_core::H256;
use sp_runtime::{transaction_validity::TransactionPriority, SaturatedConversion};
use sp_std::prelude::*;
use tuxedo_core::{
    dynamic_typing::{DynamicallyTypedData, UtxoData},
    SimpleConstraintChecker,
};

#[cfg(test)]
mod tests;

/// The main constraint checker for the utx0 piece.
/// The only operation supported is a transfer of Coins or DAPCoins of the same ID.
/// The value of the consumed Coins must be greater or equal to the value of the created Coins.
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Encode, Decode, Hash, Debug, TypeInfo)]
pub struct Utx0ConstraintChecker<const ID: u8>;

/// A single coin in the DAP money system.
/// A new-type wrapper around a hashed value.
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Encode, Decode, Hash, Debug, TypeInfo)]
pub struct DAPCoin<const ID: u8>(pub H256);

impl<const ID: u8> UtxoData for DAPCoin<ID> {
    const TYPE_ID: [u8; 4] = [b'd', b'a', b'p', ID];
}

/// Errors that can occur when checking money transactions.
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Encode, Decode, Hash, Debug, TypeInfo)]
pub enum ConstraintCheckerError {
    /// Dynamic typing issue.
    /// This error doesn't discriminate between badly typed inputs and outputs.
    BadlyTyped,
    /// The value consumed or created by this transaction overflows the value type.
    /// This could lead to problems like https://bitcointalk.org/index.php?topic=823.0
    ValueOverflow,
    /// The value of the spent input coins is less than the value of the newly created
    /// output coins. This would lead to money creation and is not allowed.
    OutputsExceedInputs,
}

fn total_value<const ID: u8>(
    data: &[DynamicallyTypedData],
) -> Result<u128, ConstraintCheckerError> {
    let mut total: u128 = 0;
    for item in data {
        if item.type_id == Coin::<ID>::TYPE_ID {
            let utxo_value = item
                .extract::<Coin<ID>>()
                .map_err(|_| ConstraintCheckerError::BadlyTyped)?
                .0;
            total = total
                .checked_add(utxo_value)
                .ok_or(ConstraintCheckerError::ValueOverflow)?;
        } else if item.type_id == DAPCoin::<ID>::TYPE_ID {
            total = total
                .checked_add(1)
                .ok_or(ConstraintCheckerError::ValueOverflow)?;
        } else {
            return Err(ConstraintCheckerError::BadlyTyped);
        }
    }
    Ok(total)
}

impl<const ID: u8> SimpleConstraintChecker for Utx0ConstraintChecker<ID> {
    type Error = ConstraintCheckerError;

    fn check(
        &self,
        input_data: &[DynamicallyTypedData],
        _peeks: &[DynamicallyTypedData],
        output_data: &[DynamicallyTypedData],
    ) -> Result<TransactionPriority, Self::Error> {
        // Check that sum of input values < output values
        let total_input_value = total_value::<ID>(input_data)?;
        let total_output_value = total_value::<ID>(output_data)?;

        if total_input_value >= total_output_value {
            Ok((total_input_value - total_output_value).saturated_into())
        } else {
            Err(ConstraintCheckerError::OutputsExceedInputs)
        }
    }
}
