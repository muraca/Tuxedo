//! This module is responsible for maintaining the wallet's local database of blocks
//! and owned UTXOs to the canonical database reported by the node.
//!
//! It is backed by a sled database
//!
//! ## Schema
//!
//! There are 4 tables in the database
//! BlockHashes     block_number:u32 => block_hash:H256
//! Blocks          block_hash:H256 => block:Block
//! UnspentOutputs  output_ref => (owner_pubkey, amount)
//! SpentOutputs    output_ref => (owner_pubkey, amount)

use std::path::PathBuf;

use crate::rpc;
use anyhow::anyhow;
use parity_scale_codec::{Decode, Encode};
use sled::Db;
use sp_core::H256;
use sp_runtime::traits::{BlakeTwo256, Hash};
use tuxedo_core::{
    types::{Input, OutputRef},
    verifier::SigCheck,
};

use jsonrpsee::http_client::HttpClient;
use runtime::{money::Coin, Block, OuterVerifier, Transaction};

/// The identifier for the blocks tree in the db.
const BLOCKS: &str = "blocks";

/// The identifier for the block_hashes tree in the db.
const BLOCK_HASHES: &str = "block_hashes";

/// The identifier for the unspent tree in the db.
const UNSPENT: &str = "unspent";

/// The identifier for the spent tree in the db.
const SPENT: &str = "spent";

/// Open a database at the given location intended for the given genesis block.
///
/// If the database is already populated, make sure it is based on the expected genesis
/// If an empty database is opened, it is initialized with the expected genesis hash and genesis block
pub(crate) fn open_db(
    db_path: PathBuf,
    expected_genesis_hash: H256,
    expected_genesis_block: Block,
) -> anyhow::Result<Db> {
    //TODO figure out why this assertion fails.
    //assert_eq!(BlakeTwo256::hash_of(&expected_genesis_block.encode()), expected_genesis_hash, "expected block hash does not match expected block");

    let db = sled::open(db_path)?;

    // Open the tables we'll need
    let wallet_block_hashes_tree = db.open_tree(BLOCK_HASHES)?;
    let wallet_blocks_tree = db.open_tree("blocks")?;

    // If the database is already populated, just make sure it is for the same genesis block
    if height(&db)?.is_some() {
        // There are database blocks, so do a quick precheck to make sure they use the same genesis block.
        let wallet_genesis_ivec = wallet_block_hashes_tree
            .get(0.encode())?
            .expect("We know there are some blocks, so there should be a 0th block.");
        let wallet_genesis_hash = H256::decode(&mut &wallet_genesis_ivec[..])?;
        log::debug!("Found existing database.");
        if expected_genesis_hash != wallet_genesis_hash {
            log::error!("Wallet's genesis does not match expected. Aborting database opening.");
            return Err(anyhow!("Node reports a different genesis block than wallet. Wallet: {wallet_genesis_hash:?}. Expected: {expected_genesis_hash:?}. Aborting all operations"));
        }
        return Ok(db);
    }

    // If there are no local blocks yet, initialize the tables
    log::info!(
        "Initializing fresh sync from genesis {:?}",
        expected_genesis_hash
    );

    // Update both tables
    wallet_block_hashes_tree.insert(0u32.encode(), expected_genesis_hash.encode())?;
    wallet_blocks_tree.insert(
        expected_genesis_hash.encode(),
        expected_genesis_block.encode(),
    )?;

    Ok(db)
}

/// Synchronize the local database to the database of the running node.
/// The wallet entirely trusts the data the node feeds it. In the bigger
/// picture, that means run your own (light) node.
pub(crate) async fn synchronize<F: Fn(&OuterVerifier) -> bool>(
    db: &Db,
    client: &HttpClient,
    filter: &F,
) -> anyhow::Result<()> {
    log::debug!("Synchronizing wallet with node.");

    // Start the algorithm at the height that the wallet currently thinks is best.
    // Fetch the block hash at that height from both the wallet's local db and the node
    let mut height: u32 = height(db)?.ok_or(anyhow!("tried to sync an uninitialized database"))?;
    let mut wallet_hash = get_block_hash(db, height)?
        .expect("Local database should have a block hash at the height reported as best");
    let mut node_hash: Option<H256> = rpc::node_get_block_hash(height, client).await?;

    // There may have been a re-org since the last time the node synced. So we loop backwards from the
    // best height the wallet knows about checking whether the wallet knows the same block as the node.
    // If not, we roll this block back on the wallet's local db, and then check the next ancestor.
    // When the wallet and the node agree on the best block, the wallet can re-sync following the node.
    // In the best case, where there is no re-org, this loop will execute zero times.
    while Some(wallet_hash) != node_hash {
        log::debug!("Divergence at height {height}. Node reports block: {node_hash:?}. Reverting wallet block: {wallet_hash:?}.");

        unapply_highest_block(db).await?;

        // Update for the next iteration
        height -= 1;
        wallet_hash = get_block_hash(db, height)?
            .expect("Local database should have a block hash at the height reported as best");
        node_hash = rpc::node_get_block_hash(height, client).await?;
    }

    // Orphaned blocks (if any) have been discarded at this point.
    // So we prepare our variables for forward syncing.
    log::debug!("Resyncing from common ancestor {node_hash:?} - {wallet_hash:?}");
    height += 1;
    node_hash = rpc::node_get_block_hash(height, client).await?;

    // Now that we have checked for reorgs and rolled back any orphan blocks, we can go ahead and sync forward.
    while let Some(hash) = node_hash {
        log::debug!("Forward syncing height {height}, hash {hash:?}");

        // Fetch the entire block in order to apply its transactions
        let block = rpc::node_get_block(hash, client)
            .await?
            .expect("Node should be able to return a block whose hash it already returned");

        // Apply the new block
        apply_block(db, block, hash, filter).await?;

        height += 1;

        node_hash = rpc::node_get_block_hash(height, client).await?;
    }

    log::debug!("Done with forward sync up to {}", height - 1);

    Ok(())
}

/// Gets the owner and amount associated with an output ref from the unspent table
///
/// Some if the output ref exists, None if it doesn't
pub(crate) fn get_unspent(db: &Db, output_ref: &OutputRef) -> anyhow::Result<Option<(H256, u128)>> {
    let wallet_unspent_tree = db.open_tree(UNSPENT)?;
    let Some(ivec) = wallet_unspent_tree.get(output_ref.encode())? else {
        return Ok(None);
    };

    Ok(Some(<(H256, u128)>::decode(&mut &ivec[..])?))
}

/// Picks an arbitrary set of unspent outputs from the database for spending.
/// The set's token values must add up to at least the specified target value.
///
/// The return value is None if the total value of the database is less than the target
/// It is Some(Vec![...]) when it is possible
pub(crate) fn get_arbitrary_unspent_set(
    db: &Db,
    target: u128,
) -> anyhow::Result<Option<Vec<OutputRef>>> {
    let wallet_unspent_tree = db.open_tree(UNSPENT)?;

    let mut total = 0u128;
    let mut keepers = Vec::new();

    let mut unspent_iter = wallet_unspent_tree.iter();
    while total < target {
        let Some(pair) = unspent_iter.next() else {
            return Ok(None);
        };

        let (output_ref_ivec, owner_amount_ivec) = pair?;
        let output_ref = OutputRef::decode(&mut &output_ref_ivec[..])?;
        let (_owner_pubkey, amount) = <(H256, u128)>::decode(&mut &owner_amount_ivec[..])?;

        total += amount;
        keepers.push(output_ref);
    }

    Ok(Some(keepers))
}

/// Gets the block hash from the local database given a block height. Similar the Node's RPC.
///
/// Some if the block exists, None if the block does not exist.
pub(crate) fn get_block_hash(db: &Db, height: u32) -> anyhow::Result<Option<H256>> {
    let wallet_block_hashes_tree = db.open_tree(BLOCK_HASHES)?;
    let Some(ivec) = wallet_block_hashes_tree.get(height.encode())? else {
        return Ok(None);
    };

    let hash = H256::decode(&mut &ivec[..])?;

    Ok(Some(hash))
}

// This is part of what I expect to be a useful public interface. For now it is not used.
#[allow(dead_code)]
/// Gets the block from the local database given a block hash. Similar to the Node's RPC.
pub(crate) fn get_block(db: &Db, hash: H256) -> anyhow::Result<Option<Block>> {
    let wallet_blocks_tree = db.open_tree(BLOCKS)?;
    let Some(ivec) = wallet_blocks_tree.get(hash.encode())? else {
        return Ok(None);
    };

    let block = Block::decode(&mut &ivec[..])?;

    Ok(Some(block))
}

/// Apply a block to the local database
pub(crate) async fn apply_block<F: Fn(&OuterVerifier) -> bool>(
    db: &Db,
    b: Block,
    block_hash: H256,
    filter: &F,
) -> anyhow::Result<()> {
    log::debug!("Applying Block {:?}, Block_Hash {:?}", b, block_hash);
    // Write the hash to the block_hashes table
    let wallet_block_hashes_tree = db.open_tree(BLOCK_HASHES)?;
    wallet_block_hashes_tree.insert(b.header.number.encode(), block_hash.encode())?;

    // Write the block to the blocks table
    let wallet_blocks_tree = db.open_tree(BLOCKS)?;
    wallet_blocks_tree.insert(block_hash.encode(), b.encode())?;

    // Iterate through each transaction
    for tx in b.extrinsics {
        apply_transaction(db, tx, filter).await?;
    }

    Ok(())
}

/// Apply a single transaction to the local database
/// The owner-specific tables are mappings from output_refs to coin amounts
async fn apply_transaction<F: Fn(&OuterVerifier) -> bool>(
    db: &Db,
    tx: Transaction,
    filter: &F,
) -> anyhow::Result<()> {
    let tx_hash = BlakeTwo256::hash_of(&tx.encode());
    log::debug!("syncing transaction {tx_hash:?}");

    // Insert all new outputs
    for (index, output) in tx
        .outputs
        .iter()
        .filter(|o| filter(&o.verifier))
        .enumerate()
    {
        // For now the wallet only supports simple coins, so skip anything else
        let amount = match output.payload.extract::<Coin<0>>() {
            Ok(Coin(amount)) => amount,
            Err(_) => continue,
        };

        let output_ref = OutputRef {
            tx_hash,
            index: index as u32,
        };

        match output.verifier {
            OuterVerifier::SigCheck(SigCheck { owner_pubkey }) => {
                // Add it to the global unspent_outputs table
                add_unspent_output(db, &output_ref, &owner_pubkey, &amount)?;
            }
            _ => return Err(anyhow!("{:?}", ())),
        }
    }

    log::debug!("about to spend all inputs");
    // Spend all the inputs
    for Input { output_ref, .. } in tx.inputs {
        spend_output(db, &output_ref)?;
    }

    Ok(())
}

/// Add a new output to the database updating all tables.
fn add_unspent_output(
    db: &Db,
    output_ref: &OutputRef,
    owner_pubkey: &H256,
    amount: &u128,
) -> anyhow::Result<()> {
    let unspent_tree = db.open_tree(UNSPENT)?;
    unspent_tree.insert(output_ref.encode(), (owner_pubkey, amount).encode())?;

    Ok(())
}

/// Remove an output from the database updating all tables.
fn remove_unspent_output(db: &Db, output_ref: &OutputRef) -> anyhow::Result<()> {
    let unspent_tree = db.open_tree(UNSPENT)?;

    unspent_tree.remove(output_ref.encode())?;

    Ok(())
}

/// Mark an existing output as spent. This does not purge all record of the output from the db.
/// It just moves the record from the unspent table to the spent table
fn spend_output(db: &Db, output_ref: &OutputRef) -> anyhow::Result<()> {
    let unspent_tree = db.open_tree(UNSPENT)?;
    let spent_tree = db.open_tree(SPENT)?;

    let Some(ivec) = unspent_tree.remove(output_ref.encode())? else {
        return Ok(());
    };
    let (owner, amount) = <(H256, u128)>::decode(&mut &ivec[..])?;
    spent_tree.insert(output_ref.encode(), (owner, amount).encode())?;

    Ok(())
}

/// Mark an output that was previously spent back as unspent.
fn unspend_output(db: &Db, output_ref: &OutputRef) -> anyhow::Result<()> {
    let unspent_tree = db.open_tree(UNSPENT)?;
    let spent_tree = db.open_tree(SPENT)?;

    let Some(ivec) = spent_tree.remove(output_ref.encode())? else {
        return Ok(());
    };
    let (owner, amount) = <(H256, u128)>::decode(&mut &ivec[..])?;
    unspent_tree.insert(output_ref.encode(), (owner, amount).encode())?;

    Ok(())
}

/// Run a transaction backwards against a database. Mark all of the Inputs
/// as unspent, and drop all of the outputs.
fn unapply_transaction(db: &Db, tx: &Transaction) -> anyhow::Result<()> {
    // Loop through the inputs moving each from spent to unspent
    for Input { output_ref, .. } in &tx.inputs {
        unspend_output(db, output_ref)?;
    }

    // Loop through the outputs pruning them from unspent and dropping all record
    let tx_hash = BlakeTwo256::hash_of(&tx.encode());

    for i in 0..tx.outputs.len() {
        let output_ref = OutputRef {
            tx_hash,
            index: i as u32,
        };
        remove_unspent_output(db, &output_ref)?;
    }

    Ok(())
}

/// Unapply the best block that the wallet currently knows about
pub(crate) async fn unapply_highest_block(db: &Db) -> anyhow::Result<Block> {
    let wallet_blocks_tree = db.open_tree(BLOCKS)?;
    let wallet_block_hashes_tree = db.open_tree(BLOCK_HASHES)?;

    // Find the best height
    let height = height(db)?.ok_or(anyhow!("Cannot unapply block from uninitialized database"))?;

    // Take the hash from the block_hashes tables
    let Some(ivec) = wallet_block_hashes_tree.remove(height.encode())? else {
        return Err(anyhow!(
            "No block hash found at height reported as best. DB is inconsistent."
        ));
    };
    let hash = H256::decode(&mut &ivec[..])?;

    // Take the block from the blocks table
    let Some(ivec) = wallet_blocks_tree.remove(hash.encode())? else {
        return Err(anyhow!(
            "Block was not present in db but block hash was. DB is corrupted."
        ));
    };

    let block = Block::decode(&mut &ivec[..])?;

    // Loop through the transactions in reverse order calling unapply
    for tx in block.extrinsics.iter().rev() {
        unapply_transaction(db, tx)?;
    }

    Ok(block)
}

/// Get the block height that the wallet is currently synced to
///
/// None means the db is not yet initialized with a genesis block
pub(crate) fn height(db: &Db) -> anyhow::Result<Option<u32>> {
    let wallet_block_hashes_tree = db.open_tree(BLOCK_HASHES)?;
    let num_blocks = wallet_block_hashes_tree.len();

    Ok(if num_blocks == 0 {
        None
    } else {
        Some(num_blocks as u32 - 1)
    })
}

// This is part of what I expect to be a useful public interface. For now it is not used.
#[allow(dead_code)]
/// Debugging use. Print out the entire block_hashes tree.
pub(crate) fn print_block_hashes_tree(db: &Db) -> anyhow::Result<()> {
    for height in 0..height(db)?.unwrap() {
        let hash = get_block_hash(db, height)?;
        println!("height: {height}, hash: {hash:?}");
    }

    Ok(())
}

/// Debugging use. Print the entire unspent outputs tree.
pub(crate) fn print_unspent_tree(db: &Db) -> anyhow::Result<()> {
    let wallet_unspent_tree = db.open_tree(UNSPENT)?;
    for x in wallet_unspent_tree.iter() {
        let (output_ref_ivec, owner_amount_ivec) = x?;
        let output_ref = hex::encode(output_ref_ivec);
        let (owner_pubkey, amount) = <(H256, u128)>::decode(&mut &owner_amount_ivec[..])?;

        println!("{output_ref}: owner {owner_pubkey:?}, amount {amount}");
    }

    Ok(())
}

/// Iterate the entire unspent set summing the values of the coins
/// on a per-address basis.
pub(crate) fn get_balances(db: &Db) -> anyhow::Result<impl Iterator<Item = (H256, u128)>> {
    let mut balances = std::collections::HashMap::<H256, u128>::new();

    let wallet_unspent_tree = db.open_tree(UNSPENT)?;

    for raw_data in wallet_unspent_tree.iter() {
        let (_output_ref_ivec, owner_amount_ivec) = raw_data?;
        let (owner, amount) = <(H256, u128)>::decode(&mut &owner_amount_ivec[..])?;

        balances
            .entry(owner)
            .and_modify(|old| *old += amount)
            .or_insert(amount);
    }

    Ok(balances.into_iter())
}
