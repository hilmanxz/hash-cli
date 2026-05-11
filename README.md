# HASH256 Rust Miner

Rust rewrite untuk miner HASH256, dengan backend GPU OpenCL yang cocok untuk AMD ROCm di VPS dan fallback CPU.

## VPS AMD ROCm

Install dependency umum:

```bash
sudo apt update
sudo apt install -y build-essential pkg-config ocl-icd-opencl-dev clinfo
```

Pastikan ROCm/OpenCL sudah terdeteksi:

```bash
clinfo | head
```

Build:

```bash
cargo build --release
```

Jalankan:

```bash
cp .env.example .env
nano .env
cargo run --release -- --backend opencl
```

Atau pakai binary:

```bash
./target/release/hash256-rust-miner --backend opencl
```

## Opsi

```bash
hash256-rust-miner --backend auto
hash256-rust-miner --backend cpu --workers 8
hash256-rust-miner --backend opencl --gpu-batch 67108864
hash256-rust-miner --once
hash256-rust-miner check
```

Environment:

```env
RPC_URL=https://ethereum-rpc.publicnode.com
PRIVATE_KEY=0x...
MINER_BACKEND=auto
CPU_WORKERS=8
CPU_BATCH_SIZE=50000
GPU_BATCH_SIZE=67108864
PRIORITY_FEE_GWEI=2
KEEP_MINING=true
```

## Catatan

- Mining memakai Ethereum mainnet dan butuh ETH untuk gas.
- Jangan pakai private key wallet utama.
- Backend OpenCL memilih GPU pertama dari platform pertama yang punya GPU.
- Kalau `--backend auto`, OpenCL dicoba dulu lalu fallback ke CPU.

