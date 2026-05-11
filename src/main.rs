mod config;
mod eth;
mod format;
mod miner;
mod telegram;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use config::{Config, CONTRACT_ADDRESS};
use eth::{Hash256Client, MiningState};
use ethers::types::U256;
use format::{hash_rate, short_decimal};
use miner::{cpu::CpuMiner, opencl::OpenClMiner, MiningBackend, Solution};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Backend {
    Auto,
    Cpu,
    Opencl,
}

#[derive(Debug, Parser)]
#[command(name = "hash256-rust-miner")]
#[command(about = "Rust HASH256 miner for CPU and AMD ROCm/OpenCL GPU")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(long, value_enum, env = "MINER_BACKEND", default_value = "auto")]
    backend: Backend,

    #[arg(long, env = "CPU_WORKERS")]
    workers: Option<usize>,

    #[arg(long = "cpu-batch", env = "CPU_BATCH_SIZE", default_value = "50000")]
    cpu_batch: u64,

    #[arg(long = "gpu-batch", env = "GPU_BATCH_SIZE", default_value = "67108864")]
    gpu_batch: usize,

    #[arg(long, env = "PRIORITY_FEE_GWEI", default_value = "2")]
    priority_fee_gwei: String,

    #[arg(long)]
    once: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    Check,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    let config = Config::from_cli(&cli)?;

    match cli.command {
        Some(Command::Check) => check_state(&config).await,
        None => mine_loop(&config).await,
    }
}

async fn check_state(config: &Config) -> Result<()> {
    let client = Hash256Client::read_only(&config.rpc_url).await?;
    let genesis = client.genesis_state().await?;
    let state = client.mining_state().await?;

    println!("Contract: {CONTRACT_ADDRESS}");
    println!("genesisState: {:?}", genesis);
    print_state(&state);
    Ok(())
}

async fn mine_loop(config: &Config) -> Result<()> {
    let client = Hash256Client::with_wallet(&config.rpc_url, &config.private_key).await?;
    let wallet = client.wallet_address();

    println!("Wallet: {}", wallet);
    println!("Contract: {CONTRACT_ADDRESS}");
    println!("Backend: {:?}", config.backend);

    loop {
        let state = client.mining_state().await?;
        let challenge = client.get_challenge().await?;

        println!();
        print_state(&state);
        println!("Challenge: 0x{}", hex::encode(challenge));

        let solution = find_solution(challenge, state.difficulty, config)
            .context("failed while searching solution")?;

        println!();
        println!("FOUND via {}", solution.backend.as_str());
        println!("Nonce: {}", solution.nonce);
        println!("Hash: {}", solution.hash);
        println!("Hashes: {}", solution.hashes);

        let tx = client
            .submit_solution(U256::from(solution.nonce), &config.priority_fee_gwei)
            .await?;
        if let Some(telegram) = &config.telegram {
            match telegram::notify_success(telegram, wallet, &solution, &tx).await {
                Ok(()) => println!("Telegram: sent"),
                Err(err) => eprintln!("Telegram gagal: {err:#}"),
            }
        }

        if !config.keep_mining {
            break;
        }
    }

    Ok(())
}

fn print_state(state: &MiningState) {
    println!("Era: {}", state.era);
    println!(
        "Reward: {} HASH",
        ethers::utils::format_units(state.reward, 18).unwrap_or_else(|_| state.reward.to_string())
    );
    println!("Difficulty: {}", state.difficulty);
    println!("Epoch: {}", state.epoch);
}

fn find_solution(challenge: [u8; 32], difficulty: U256, config: &Config) -> Result<Solution> {
    let mut progress = ProgressPrinter::new();

    if matches!(config.backend, Backend::Opencl | Backend::Auto) {
        match OpenClMiner::new(config.gpu_batch) {
            Ok(mut miner) => {
                println!("OpenCL: AMD/ROCm compatible GPU backend");
                match miner.search(challenge, difficulty, &mut |hashes, hashrate| {
                    progress.print(MiningBackend::OpenCl, hashes, hashrate);
                }) {
                    Ok(solution) => return Ok(solution),
                    Err(err) if config.backend == Backend::Auto => {
                        eprintln!("OpenCL gagal, fallback ke CPU: {err:#}");
                    }
                    Err(err) => return Err(err),
                }
            }
            Err(err) if config.backend == Backend::Auto => {
                eprintln!("OpenCL tidak tersedia, fallback ke CPU: {err:#}");
            }
            Err(err) => return Err(err),
        }
    }

    let mut cpu = CpuMiner::new(config.workers, config.cpu_batch);
    println!("CPU workers: {}", config.workers);
    cpu.search(challenge, difficulty, &mut |hashes, hashrate| {
        progress.print(MiningBackend::Cpu, hashes, hashrate);
    })
}

struct ProgressPrinter {
    last: Instant,
}

impl ProgressPrinter {
    fn new() -> Self {
        Self {
            last: Instant::now() - Duration::from_secs(10),
        }
    }

    fn print(&mut self, backend: MiningBackend, hashes: u64, hashrate: f64) {
        if self.last.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last = Instant::now();
        println!(
            "{} | {} | {} hashes",
            backend.as_str(),
            hash_rate(hashrate),
            short_decimal(hashes)
        );
    }
}
