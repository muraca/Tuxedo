use super::*;
use money::Coin;
use sp_runtime::traits::{BlakeTwo256, Hash};

#[test]
fn mint_valid_transaction_works() {
    assert_eq!(
        SimpleConstraintChecker::check(
            &Tux0Mint::<0>,
            &[Coin::<0>(1).into(), Coin::<0>(4).into()],
            &[],
            &[
                DAPCoin::<0>(BlakeTwo256::hash_of(&0u8)).into(),
                DAPCoin::<0>(BlakeTwo256::hash_of(&1u8)).into(),
            ]
        ),
        Ok(3)
    );
}

#[test]
fn mint_no_input_fails() {
    assert_eq!(
        SimpleConstraintChecker::check(
            &Tux0Mint::<0>,
            &[],
            &[],
            &[DAPCoin::<0>(BlakeTwo256::hash_of(&0u8)).into()]
        ),
        Err(ConstraintCheckerError::OutputsExceedInputs)
    );
}

#[test]
fn mint_no_output_works() {
    // This should work, as it is a valid transaction, which burns all the input.
    assert_eq!(
        SimpleConstraintChecker::check(&Tux0Mint::<0>, &[Coin::<0>(3).into()], &[], &[]),
        Ok(3)
    );
}

#[test]
fn mint_money_creation_fails() {
    assert_eq!(
        SimpleConstraintChecker::check(
            &Tux0Mint::<0>,
            &[Coin::<0>(1).into()],
            &[],
            &[
                DAPCoin::<0>(BlakeTwo256::hash_of(&1u8)).into(),
                DAPCoin::<0>(BlakeTwo256::hash_of(&2u8)).into(),
            ]
        ),
        Err(ConstraintCheckerError::OutputsExceedInputs)
    );
}

#[test]
fn mint_invalid_input() {
    assert_eq!(
        SimpleConstraintChecker::check(
            &Tux0Mint::<0>,
            &[DAPCoin::<0>(BlakeTwo256::hash_of(&0u8)).into()],
            &[],
            &[DAPCoin::<0>(BlakeTwo256::hash_of(&2u8)).into(),]
        ),
        Err(ConstraintCheckerError::BadlyTyped)
    )
}

#[test]
fn mint_invalid_output() {
    assert_eq!(
        SimpleConstraintChecker::check(
            &Tux0Mint::<0>,
            &[Coin::<0>(5).into()],
            &[],
            &[
                DAPCoin::<0>(BlakeTwo256::hash_of(&1u8)).into(),
                Coin::<0>(4).into(),
            ]
        ),
        Err(ConstraintCheckerError::BadlyTyped)
    )
}

// TODO: implement transfer tests
