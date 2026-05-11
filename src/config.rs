use crate::Backend;
use anyhow::{bail, Context, Result};
use std::env;

pub const CONTRACT_ADDRESS: &str = "0xAC7b5d06fa1e77D08aea40d46cB7C5923A87A0cc";

#[derive(Debug, Clone)]
pub struct Config {
    pub rpc_url: String,
    pub private_key: String,
    pub backend: Backend,
    pub workers: usize,
    pub cpu_batch: u64,
    pub gpu_batch: usize,
    pub priority_fee_gwei: String,
    pub keep_mining: bool,
    pub telegram: Option<TelegramConfig>,
}

#[derive(Debug, Clone)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub chat_id: String,
}

impl Config {
    pub fn from_cli(cli: &crate::Cli) -> Result<Self> {
        let rpc_url = env::var("RPC_URL").context("Isi RPC_URL di file .env dulu")?;
        let private_key = env::var("PRIVATE_KEY").unwrap_or_default();
        let needs_wallet = !matches!(cli.command, Some(crate::Command::Check));

        if needs_wallet {
            if private_key.is_empty() {
                bail!("Isi PRIVATE_KEY di file .env dulu");
            }
            if !private_key.starts_with("0x") {
                bail!("PRIVATE_KEY harus diawali 0x");
            }
        }

        let workers = cli
            .workers
            .unwrap_or_else(|| num_cpus::get().saturating_sub(1).max(1))
            .clamp(1, 64);
        let env_keep = env::var("KEEP_MINING").unwrap_or_else(|_| "true".to_string());

        Ok(Self {
            rpc_url,
            private_key,
            backend: cli.backend,
            workers,
            cpu_batch: cli.cpu_batch.max(1),
            gpu_batch: normalize_gpu_batch(cli.gpu_batch),
            priority_fee_gwei: cli.priority_fee_gwei.clone(),
            keep_mining: !cli.once && env_keep != "false",
            telegram: read_telegram_config(),
        })
    }
}

fn normalize_gpu_batch(batch: usize) -> usize {
    let batch = batch.max(65_536);
    batch - (batch % 64)
}

fn read_telegram_config() -> Option<TelegramConfig> {
    let bot_token = env::var("TELEGRAM_BOT_TOKEN").ok()?;
    let chat_id = env::var("TELEGRAM_CHAT_ID").ok()?;

    if bot_token.trim().is_empty() || chat_id.trim().is_empty() {
        return None;
    }

    Some(TelegramConfig { bot_token, chat_id })
}
