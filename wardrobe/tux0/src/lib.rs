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
    ensure, SimpleConstraintChecker,
};

#[cfg(test)]
mod tests;

/// The main constraint checker for the utx0 piece. Allows minting and pouring DAP coins.
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Encode, Decode, Hash, Debug, TypeInfo)]
pub enum Utx0ConstraintChecker<const ID: u8> {
    /// A mint transaction that creates DAPCoins from Coins of the same ID.
    /// The amount of DAPCoins created is equal or less than the value of Coins consumed.
    Mint,
    /// A pour transaction, where DAP coins are consumed, and other DAPCoin or Coin might be created.
    Pour,
}

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
    /// This functionality has not been implemented yet.
    NotImplemented,

    /// The transaction attempts to mint without consuming any inputs.
    MintingFromNothing,
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

impl<const ID: u8> SimpleConstraintChecker for Utx0ConstraintChecker<ID> {
    type Error = ConstraintCheckerError;

    fn check(
        &self,
        input_data: &[DynamicallyTypedData],
        _peeks: &[DynamicallyTypedData],
        output_data: &[DynamicallyTypedData],
    ) -> Result<TransactionPriority, Self::Error> {
        match &self {
            Utx0ConstraintChecker::Mint => {
                // Check that we are consuming at least one input
                ensure!(
                    !input_data.is_empty(),
                    ConstraintCheckerError::MintingFromNothing
                );

                let mut total_input_value: u128 = 0;

                // Check that sum of input values < output values
                for input in input_data {
                    let utxo_value = input
                        .extract::<Coin<ID>>()
                        .map_err(|_| ConstraintCheckerError::BadlyTyped)?
                        .0;
                    total_input_value = total_input_value
                        .checked_add(utxo_value)
                        .ok_or(ConstraintCheckerError::ValueOverflow)?;
                }

                if total_input_value >= output_data.len() as u128 {
                    Ok((total_input_value - output_data.len() as u128).saturated_into())
                } else {
                    Err(ConstraintCheckerError::OutputsExceedInputs)
                }
            }
            Utx0ConstraintChecker::Pour => Err(ConstraintCheckerError::NotImplemented),
        }
    }
}
