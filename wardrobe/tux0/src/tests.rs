use super::*;
use money::Coin;
use sp_runtime::traits::{BlakeTwo256, Hash};

#[test]
fn mint_valid_transaction_works() {
    assert_eq!(
        Utx0ConstraintChecker::<0>::Mint.check(
            &[Coin::<0>(3).into()],
            &[],
            &[
                DAPCoin::<1>(BlakeTwo256::hash_of(&0u8)).into(),
                DAPCoin::<1>(BlakeTwo256::hash_of(&1u8)).into(),
            ]
        ),
        Ok(1)
    );
}

#[test]
fn mint_no_input_fails() {
    assert_eq!(
        Utx0ConstraintChecker::<0>::Mint.check(
            &[],
            &[],
            &[DAPCoin::<1>(BlakeTwo256::hash_of(&0u8)).into(),]
        ),
        Err(ConstraintCheckerError::MintingFromNothing)
    );
}

#[test]
fn mint_no_output_works() {
    // This should work, as it is a valid transaction, which burns all the input.
    assert_eq!(
        Utx0ConstraintChecker::<0>::Mint.check(&[Coin::<0>(3).into()], &[], &[]),
        Ok(3)
    );
}

#[test]
fn mint_money_creation_fails() {
    assert_eq!(
        Utx0ConstraintChecker::<0>::Mint.check(
            &[Coin::<0>(1).into()],
            &[],
            &[
                DAPCoin::<1>(BlakeTwo256::hash_of(&0u8)).into(),
                DAPCoin::<1>(BlakeTwo256::hash_of(&1u8)).into(),
            ]
        ),
        Err(ConstraintCheckerError::OutputsExceedInputs)
    );
}
