//! An implementation of a DAP fungible token.

#![cfg_attr(not(feature = "std"), no_std)]

use money::Coin;
use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::{transaction_validity::TransactionPriority, SaturatedConversion};
use sp_std::prelude::*;
use tuxedo_core::{
    dynamic_typing::{DynamicallyTypedData, UtxoData},
    ensure,
    types::{Output, OutputRef, Transaction},
    utxo_set::TransparentUtxoSet,
    ConstraintChecker, SimpleConstraintChecker, Verifier,
};

#[cfg(feature = "std")]
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

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

/// Computes the total value from a list of DynamicallyTypedData.
/// If allow_money is true, then Coins are allowed in the list, otherwise it fails.
/// If allow_dap is true, then DAPCoins are allowed in the list, otherwise it fails.
fn total_value<const ID: u8>(
    data: &[DynamicallyTypedData],
    allow_money: bool,
    allow_dap: bool,
) -> Result<u128, ConstraintCheckerError> {
    let mut total: u128 = 0;
    for item in data {
        if item.type_id == Coin::<ID>::TYPE_ID {
            ensure!(allow_money, ConstraintCheckerError::BadlyTyped);
            let utxo_value = item
                .extract::<Coin<ID>>()
                .map_err(|_| ConstraintCheckerError::BadlyTyped)?
                .0;
            total = total
                .checked_add(utxo_value)
                .ok_or(ConstraintCheckerError::ValueOverflow)?;
        } else if item.type_id == DAPCoin::<ID>::TYPE_ID {
            ensure!(allow_dap, ConstraintCheckerError::BadlyTyped);
            total = total
                .checked_add(1)
                .ok_or(ConstraintCheckerError::ValueOverflow)?;
        } else {
            return Err(ConstraintCheckerError::BadlyTyped);
        }
    }
    Ok(total)
}

/// The only operation supported by this ConstraintChecker is the mint,
/// that consumes Coins to produce DAPCoins of the same ID.
/// The number of the produced DAPCoins must be less or equal to the Value of the consumed Coins.
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Encode, Decode, Hash, Debug, TypeInfo)]
pub struct Tux0Mint<const ID: u8>;

impl<const ID: u8> SimpleConstraintChecker for Tux0Mint<ID> {
    type Error = ConstraintCheckerError;

    fn check(
        &self,
        input_data: &[DynamicallyTypedData],
        _peeks: &[DynamicallyTypedData],
        output_data: &[DynamicallyTypedData],
    ) -> Result<TransactionPriority, Self::Error> {
        // Only allow Coins as inputs.
        let total_input_value = total_value::<ID>(&input_data, true, false)?;
        // Only allow DAPCoins as outputs.
        let total_output_value = total_value::<ID>(&output_data, false, true)?;

        if total_input_value >= total_output_value {
            Ok((total_input_value - total_output_value).saturated_into())
        } else {
            Err(ConstraintCheckerError::OutputsExceedInputs)
        }
    }
}

/// A mock random number generator that always returns 0.
struct MockRng;
impl rand::CryptoRng for MockRng {}
impl rand::RngCore for MockRng {
    fn next_u32(&mut self) -> u32 {
        0
    }

    fn next_u64(&mut self) -> u64 {
        0
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for i in dest.iter_mut() {
            *i = 0;
        }
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

#[derive(Debug, Encode, Decode, Clone, TypeInfo)]
pub struct SpendData {
    pub pubkey: H256,
    pub secret: H256,
    pub utxo_ref: OutputRef,
}

/// The Verifier used along Tux0Transfer, to check that a DAP Coin's secret is revealed correctly before spending it.
#[derive(Debug, Encode, Decode, Clone, TypeInfo)]
pub struct Tux0TransferVerifier<const ID: u8>;

impl<const ID: u8> Verifier for Tux0TransferVerifier<ID> {
    fn verify(&self, simplified_tx: &[u8], redeemer: &[u8]) -> bool {
        // Check that the transaction is valid and uses the right ConstraintChecker.
        let Ok(transaction) =
            Transaction::<Self, Tux0Transfer<ID>>::decode(&mut &simplified_tx[..])
        else {
            return false;
        };

        let Ok(spend_data) = SpendData::decode(&mut &redeemer[..]) else {
            return false;
        };

        if transaction
            .inputs
            .iter()
            .find(|input| input.output_ref == spend_data.utxo_ref)
            .is_none()
        {
            return false;
        };

        let Ok(pubkey) = ecies_ed25519::PublicKey::from_bytes(&spend_data.pubkey.0) else {
            return false;
        };

        ecies_ed25519::encrypt(&pubkey, &spend_data.secret.0, &mut MockRng {}).unwrap_or_default()
            == TransparentUtxoSet::<Self>::peek_utxo(&spend_data.utxo_ref)
                .expect("existence of UTXO already verified by executive")
                .payload
                .data
    }
}

/// The only operation supported by this checker is a transfer,
/// which consumes DAPCoins only, and produces Coins or DAPCoins of the same ID.
/// The value of the consumed Coins must be greater or equal to the value of the created Coins.
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Encode, Decode, Hash, Debug, TypeInfo)]
pub struct Tux0Transfer<const ID: u8>;

// This is a ConstraintChecker instead of a SimpleConstraintChecker to only allow the Tux0TransferVerifier.
impl<const ID: u8> ConstraintChecker<Tux0TransferVerifier<ID>> for Tux0Transfer<ID> {
    type Error = ConstraintCheckerError;

    fn check(
        &self,
        inputs: &[Output<Tux0TransferVerifier<ID>>],
        _peeks: &[Output<Tux0TransferVerifier<ID>>],
        outputs: &[Output<Tux0TransferVerifier<ID>>],
    ) -> Result<TransactionPriority, Self::Error> {
        let input_data: Vec<DynamicallyTypedData> =
            inputs.iter().map(|i| i.payload.clone()).collect();
        let output_data: Vec<DynamicallyTypedData> =
            outputs.iter().map(|o| o.payload.clone()).collect();

        // Only allow DAPCoins as inputs.
        let total_input_value = total_value::<ID>(&input_data, false, true)?;
        // Allow both Coins and DAPCoins as outputs.
        let total_output_value = total_value::<ID>(&output_data, true, true)?;

        if total_input_value >= total_output_value {
            Ok((total_input_value - total_output_value).saturated_into())
        } else {
            Err(ConstraintCheckerError::OutputsExceedInputs)
        }
    }
}
