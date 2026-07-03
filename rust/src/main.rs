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

// Build an RPC client scoped to a specific wallet (the /wallet/<name> endpoint).
fn wallet_client(name: &str) -> bitcoincore_rpc::Result<Client> {
    Client::new(
        &format!("{RPC_URL}/wallet/{name}"),
        Auth::UserPass(RPC_USER.to_owned(), RPC_PASS.to_owned()),
    )
}

// Ensure a wallet exists and is loaded, whether it is already loaded, exists on
// disk but unloaded, or does not exist yet.
fn ensure_wallet(rpc: &Client, name: &str) -> bitcoincore_rpc::Result<()> {
    let loaded = rpc.list_wallets()?;
    if loaded.iter().any(|w| w == name) {
        return Ok(());
    }
    // Not loaded. Try to create it; if it already exists on disk, load it instead.
    if rpc.create_wallet(name, None, None, None, None).is_err() {
        let _ = rpc.load_wallet(name);
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
    ensure_wallet(&rpc, "Miner")?;
    ensure_wallet(&rpc, "Trader")?;
    let miner = wallet_client("Miner")?;
    let trader = wallet_client("Trader")?;

    let mining_reward_addr = miner
        .get_new_address(Some("Mining Reward"), None)?
        .require_network(Network::Regtest)
        .expect("address should be valid for regtest");

    // Generate spendable balances in the Miner wallet. How many blocks needs to be mined?
    //
    // Block rewards only mature only after 100 confirmations, so the
    // balance stays 0 until the first block's 50 BTC reward matures hence why it
    // takes 101 blocks before the Miner balance becomes positive.
    let mut blocks_mined = 0u64;
    loop {
        rpc.generate_to_address(1, &mining_reward_addr)?;
        blocks_mined += 1;
        if miner.get_balance(None, None)? > Amount::ZERO {
            break;
        }
    }
    println!("Mined {blocks_mined} blocks before the Miner balance became positive.");

    let miner_balance = miner.get_balance(None, None)?;
    println!("Miner wallet balance: {} BTC", miner_balance.to_btc());

    // Load Trader wallet and generate a new address
    let trader_receive_addr = trader
        .get_new_address(Some("Received"), None)?
        .require_network(Network::Regtest)
        .expect("address should be valid for regtest");

    // Send 20 BTC from Miner to Trader
    let send_amount = Amount::from_btc(20.0).unwrap();
    let txid = miner.send_to_address(
        &trader_receive_addr,
        send_amount,
        None,
        None,
        None,
        None,
        None,
        None,
    )?;
    println!("Sent 20 BTC from Miner to Trader. Txid: {txid}");

    // Check transaction in mempool
    let mempool_entry = rpc.get_mempool_entry(&txid)?;
    println!("Mempool entry for {txid}: {mempool_entry:?}");

    // Mine 1 block to confirm the transaction
    let confirm_blocks = rpc.generate_to_address(1, &mining_reward_addr)?;
    let block_hash = confirm_blocks[0];
    let block_height = rpc.get_block_count()?;

    // Extract all required transaction details
    let wallet_tx = miner.get_transaction(&txid, None)?;
    let fee = wallet_tx
        .fee
        .expect("outgoing wallet tx should report a fee")
        .to_btc();

    let raw_tx = rpc.get_raw_transaction_info(&txid, Some(&block_hash))?;

    // Get the single input back to the coinbase output it spends
    let input = &raw_tx.vin[0];
    let prev_txid = input.txid.expect("input must reference a previous txid");
    let prev_vout = input.vout.expect("input must reference a previous vout") as usize;
    let prev_tx = rpc.get_raw_transaction_info(&prev_txid, None)?;
    let prev_out = &prev_tx.vout[prev_vout];
    let miner_input_amount = prev_out.value.to_btc();
    let miner_input_address = prev_out
        .script_pub_key
        .address
        .clone()
        .expect("input's previous output should have an address")
        .assume_checked();

    // Distinguish the trader's payment from the miner's change
    let trader_spk = trader_receive_addr.script_pubkey();
    let mut trader_output_amount = 0.0;
    let mut miner_change_amount = 0.0;
    let mut miner_change_address = String::new();
    for out in &raw_tx.vout {
        let addr = out
            .script_pub_key
            .address
            .clone()
            .expect("output should have an address")
            .assume_checked();
        if addr.script_pubkey() == trader_spk {
            trader_output_amount = out.value.to_btc();
        } else {
            miner_change_amount = out.value.to_btc();
            miner_change_address = addr.to_string();
        }
    }

    // Write the data to ../out.txt in the specified format given in readme.md
    let mut file = File::create("../out.txt").expect("failed to create out.txt");
    writeln!(file, "{txid}").unwrap();
    writeln!(file, "{miner_input_address}").unwrap();
    writeln!(file, "{miner_input_amount}").unwrap();
    writeln!(file, "{trader_receive_addr}").unwrap();
    writeln!(file, "{trader_output_amount}").unwrap();
    writeln!(file, "{miner_change_address}").unwrap();
    writeln!(file, "{miner_change_amount}").unwrap();
    writeln!(file, "{fee}").unwrap();
    writeln!(file, "{block_height}").unwrap();
    writeln!(file, "{block_hash}").unwrap();

    println!("Wrote transaction details to ../out.txt");

    Ok(())
}
