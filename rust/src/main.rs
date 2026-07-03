#![allow(unused)]
use bitcoin::hex::DisplayHex;
use bitcoincore_rpc::bitcoin::{Address, Amount, Network};
use bitcoincore_rpc::{Auth, Client, RpcApi};
use serde::Deserialize;
use serde_json::json;
use std::fs::File;
use std::io::Write;

// Node access params
const RPC_URL: &str = "http://127.0.0.1:18443"; // Default regtest RPC port
const RPC_USER: &str = "alice";
const RPC_PASS: &str = "password";

// You can use calls not provided in RPC lib API using the generic `call` function.
// An example of using the `send` RPC call, which doesn't have exposed API.
// You can also use serde_json `Deserialize` derivation to capture the returned json result.
fn send(rpc: &Client, addr: &str) -> bitcoincore_rpc::Result<String> {
    let args = [
        json!([{addr : 100 }]), // recipient address
        json!(null),            // conf target
        json!(null),            // estimate mode
        json!(null),            // fee rate in sats/vb
        json!(null),            // Empty option object
    ];

    #[derive(Deserialize)]
    struct SendResult {
        complete: bool,
        txid: String,
    }
    let send_result = rpc.call::<SendResult>("send", &args)?;
    assert!(send_result.complete);
    Ok(send_result.txid)
}

// createwallet errors out if the wallet is already on disk, and loadwallet errors out if it's
// already loaded, so just try both and only bail if neither one worked.
fn create_or_load_wallet(rpc: &Client, wallet_name: &str) -> bitcoincore_rpc::Result<()> {
    if rpc
        .create_wallet(wallet_name, None, None, None, None)
        .is_ok()
    {
        println!("Created wallet: {}", wallet_name);
        return Ok(());
    }

    match rpc.load_wallet(wallet_name) {
        Ok(_) => println!("Loaded wallet: {}", wallet_name),
        Err(_) => println!("Wallet {} is already loaded, moving on", wallet_name),
    }
    Ok(())
}

fn main() -> bitcoincore_rpc::Result<()> {
    // Connect to Bitcoin Core RPC
    let rpc = Client::new(
        RPC_URL,
        Auth::UserPass(RPC_USER.to_owned(), RPC_PASS.to_owned()),
    )?;

    // Get blockchain info
    let blockchain_info = rpc.get_blockchain_info()?;
    println!("Blockchain Info: {:?}", blockchain_info);

    // Create/Load the wallets, named 'Miner' and 'Trader'. Have logic to optionally create/load them if they do not exist or not loaded already.
    create_or_load_wallet(&rpc, "Miner")?;
    create_or_load_wallet(&rpc, "Trader")?;

    // Wallets live at their own endpoint (/wallet/<name>) in Bitcoin Core, so from here on
    // use a client pointed at each wallet instead of passing the name on every call.
    let miner_rpc = Client::new(
        &format!("{RPC_URL}/wallet/Miner"),
        Auth::UserPass(RPC_USER.to_owned(), RPC_PASS.to_owned()),
    )?;
    let trader_rpc = Client::new(
        &format!("{RPC_URL}/wallet/Trader"),
        Auth::UserPass(RPC_USER.to_owned(), RPC_PASS.to_owned()),
    )?;

    // Generate spendable balances in the Miner wallet. How many blocks needs to be mined?
    let miner_address = miner_rpc
        .get_new_address(Some("Mining Reward"), None)?
        .require_network(Network::Regtest)
        .expect("node is running on regtest");

    // Mine one block at a time and check the balance after each, instead of hardcoding a
    // number, so we can actually see how many blocks it takes.
    let mut blocks_mined: u32 = 0;
    let miner_balance = loop {
        rpc.generate_to_address(1, &miner_address)?;
        blocks_mined += 1;
        let balance = miner_rpc.get_balance(None, None)?;
        if balance > Amount::ZERO {
            break balance;
        }
    };
    println!("It took {blocks_mined} blocks for the Miner wallet to show a positive balance.");
    // Coinbase outputs are not spendable right away - Bitcoin Core only counts them towards
    // the wallet balance once they have COINBASE_MATURITY (100) confirmations. So the block
    // that actually paid the Miner wallet stays "immature" until 100 more blocks are mined on
    // top of it, which is why it takes 101 blocks, not 1, before get_balance() moves off zero.

    // Print the balance of the Miner wallet
    println!("Miner wallet balance after mining: {miner_balance}");

    // Load Trader wallet and generate a new address
    let trader_address = trader_rpc
        .get_new_address(Some("Received"), None)?
        .require_network(Network::Regtest)
        .expect("node is running on regtest");

    // Send 20 BTC from Miner to Trader
    let txid = miner_rpc.send_to_address(
        &trader_address,
        Amount::from_btc(20.0)?,
        None,
        None,
        None,
        None,
        None,
        None,
    )?;
    println!("Sent 20 BTC from Miner to Trader. txid: {txid}");

    // Check transaction in mempool
    let mempool_entry = miner_rpc.get_mempool_entry(&txid)?;
    println!("Unconfirmed transaction found in mempool: {mempool_entry:#?}");

    // Mine 1 block to confirm the transaction
    rpc.generate_to_address(1, &miner_address)?;

    // Extract all required transaction details.
    // gettransaction (wallet call) hands us the fee and the confirming block for free.
    let wallet_tx = miner_rpc.get_transaction(&txid, None)?;
    let fee = wallet_tx.fee.expect("a sent transaction always has a fee");
    let block_height = wallet_tx
        .info
        .blockheight
        .expect("transaction was just confirmed, should have a height");
    let block_hash = wallet_tx
        .info
        .blockhash
        .expect("transaction was just confirmed, should have a hash");

    // getrawtransaction (verbose) gives us the actual vin/vout with addresses and amounts.
    let raw_tx = miner_rpc.get_raw_transaction_info(&txid, None)?;

    // Only one input funded this send (checked by the autograder too), spent from the
    // coinbase output that paid the Miner's "Mining Reward" address. Look that output up to
    // get its address and amount.
    let spent_input = &raw_tx.vin[0];
    let prev_txid = spent_input.txid.expect("input is not a coinbase");
    let prev_vout = spent_input.vout.expect("input is not a coinbase") as usize;
    let prev_tx = miner_rpc.get_raw_transaction_info(&prev_txid, None)?;
    let prev_out = &prev_tx.vout[prev_vout];
    let miner_input_address = prev_out
        .script_pub_key
        .address
        .clone()
        .expect("previous output should carry an address")
        .assume_checked();
    let miner_input_amount = prev_out.value;

    // Two outputs: the 20 BTC paid to Trader, and the change that comes back to Miner.
    // Whichever output isn't Trader's address is the change output.
    let mut trader_output_amount = Amount::ZERO;
    let mut miner_change_address: Option<Address> = None;
    let mut miner_change_amount = Amount::ZERO;
    for vout in &raw_tx.vout {
        let vout_address = vout
            .script_pub_key
            .address
            .clone()
            .expect("output should carry an address")
            .assume_checked();
        if vout_address == trader_address {
            trader_output_amount = vout.value;
        } else {
            miner_change_address = Some(vout_address);
            miner_change_amount = vout.value;
        }
    }
    let miner_change_address = miner_change_address.expect("change output should exist");

    // Write the data to ../out.txt in the specified format given in readme.md
    let mut out_file = File::create("../out.txt")?;
    writeln!(out_file, "{txid}")?;
    writeln!(out_file, "{miner_input_address}")?;
    writeln!(out_file, "{:.8}", miner_input_amount.to_btc())?;
    writeln!(out_file, "{trader_address}")?;
    writeln!(out_file, "{:.8}", trader_output_amount.to_btc())?;
    writeln!(out_file, "{miner_change_address}")?;
    writeln!(out_file, "{:.8}", miner_change_amount.to_btc())?;
    writeln!(out_file, "{:.8}", fee.to_btc())?;
    writeln!(out_file, "{block_height}")?;
    writeln!(out_file, "{block_hash}")?;

    println!("Done, wrote transaction details to out.txt");

    Ok(())
}
