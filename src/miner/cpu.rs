use super::{MiningBackend, Solution};
use anyhow::{anyhow, Result};
use ethers::types::U256;
use rand::RngCore;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Instant,
};
use tiny_keccak::{Hasher, Keccak};

pub struct CpuMiner {
    workers: usize,
    batch_size: u64,
}

impl CpuMiner {
    pub fn new(workers: usize, batch_size: u64) -> Self {
        Self {
            workers,
            batch_size,
        }
    }

    pub fn search<F>(
        &mut self,
        challenge: [u8; 32],
        difficulty: U256,
        on_progress: &mut F,
    ) -> Result<Solution>
    where
        F: FnMut(u64, f64),
    {
        let start_nonce = rand::thread_rng().next_u64();
        let next_nonce = Arc::new(AtomicU64::new(start_nonce));
        let stopped = Arc::new(AtomicBool::new(false));
        let total_hashes = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel();
        let started = Instant::now();

        let mut handles = Vec::with_capacity(self.workers);
        for _ in 0..self.workers {
            let tx = tx.clone();
            let next_nonce = Arc::clone(&next_nonce);
            let stopped = Arc::clone(&stopped);
            let total_hashes = Arc::clone(&total_hashes);
            let batch_size = self.batch_size;

            handles.push(thread::spawn(move || {
                while !stopped.load(Ordering::Relaxed) {
                    let start = next_nonce.fetch_add(batch_size, Ordering::Relaxed);
                    if let Some((nonce, hash)) =
                        search_batch(challenge, difficulty, start, batch_size, &stopped)
                    {
                        stopped.store(true, Ordering::Relaxed);
                        let _ = tx.send(WorkerMessage::Found { nonce, hash });
                        return;
                    }

                    let hashes = total_hashes.fetch_add(batch_size, Ordering::Relaxed) + batch_size;
                    let _ = tx.send(WorkerMessage::Progress { hashes });
                }
            }));
        }
        drop(tx);

        while let Ok(message) = rx.recv() {
            match message {
                WorkerMessage::Found { nonce, hash } => {
                    stopped.store(true, Ordering::Relaxed);
                    for handle in handles {
                        let _ = handle.join();
                    }
                    return Ok(Solution {
                        backend: MiningBackend::Cpu,
                        nonce,
                        hash: format!("0x{}", hex::encode(hash)),
                        hashes: total_hashes.load(Ordering::Relaxed),
                    });
                }
                WorkerMessage::Progress { hashes } => {
                    let elapsed = started.elapsed().as_secs_f64().max(0.001);
                    on_progress(hashes, hashes as f64 / elapsed);
                }
            }
        }

        Err(anyhow!("CPU workers stopped before finding a solution"))
    }
}

enum WorkerMessage {
    Found { nonce: u64, hash: [u8; 32] },
    Progress { hashes: u64 },
}

fn search_batch(
    challenge: [u8; 32],
    difficulty: U256,
    start: u64,
    count: u64,
    stopped: &AtomicBool,
) -> Option<(u64, [u8; 32])> {
    for offset in 0..count {
        if stopped.load(Ordering::Relaxed) {
            return None;
        }

        let nonce = start.wrapping_add(offset);
        let hash = hash_challenge_nonce(challenge, nonce);
        if U256::from_big_endian(&hash) < difficulty {
            return Some((nonce, hash));
        }
    }

    None
}

fn hash_challenge_nonce(challenge: [u8; 32], nonce: u64) -> [u8; 32] {
    let mut input = [0u8; 64];
    input[..32].copy_from_slice(&challenge);
    input[56..64].copy_from_slice(&nonce.to_be_bytes());

    let mut output = [0u8; 32];
    let mut keccak = Keccak::v256();
    keccak.update(&input);
    keccak.finalize(&mut output);
    output
}
