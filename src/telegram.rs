use crate::{config::TelegramConfig, eth::TxReport, miner::Solution};
use anyhow::{Context, Result};
use ethers::types::Address;
use std::time::Duration;

pub async fn notify_success(
    config: &TelegramConfig,
    wallet: Address,
    solution: &Solution,
    tx: &TxReport,
) -> Result<()> {
    let text = format!(
        "HASH256 mined successfully\n\nWallet: {wallet:?}\nBackend: {}\nNonce: {}\nHash: {}\nHashes: {}\nTX: https://etherscan.io/tx/{:?}\nBlock: {}",
        solution.backend.as_str(),
        solution.nonce,
        solution.hash,
        solution.hashes,
        tx.tx_hash,
        tx.block_number,
    );

    send_message(config, &text).await
}

async fn send_message(config: &TelegramConfig, text: &str) -> Result<()> {
    let url = format!(
        "https://api.telegram.org/bot{}/sendMessage",
        config.bot_token
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .connect_timeout(Duration::from_secs(2))
        .build()
        .context("gagal membuat Telegram HTTP client")?;

    let response = client
        .post(url)
        .form(&[
            ("chat_id", config.chat_id.as_str()),
            ("text", text),
            ("disable_web_page_preview", "true"),
        ])
        .send()
        .await
        .context("gagal mengirim Telegram message")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Telegram API error {status}: {body}");
    }

    Ok(())
}
