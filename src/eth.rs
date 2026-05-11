use crate::config::CONTRACT_ADDRESS;
use anyhow::{Context, Result};
use ethers::{
    contract::abigen,
    middleware::SignerMiddleware,
    providers::{Http, Middleware, Provider},
    signers::{LocalWallet, Signer},
    types::{
        transaction::{eip1559::Eip1559TransactionRequest, eip2718::TypedTransaction},
        Address, H256, U256, U64,
    },
    utils::parse_units,
};
use std::{str::FromStr, sync::Arc};

abigen!(
    Hash256Contract,
    r#"[
        function getChallenge(address miner) view returns (bytes32)
        function miningState() view returns (uint256 era,uint256 reward,uint256 difficulty,uint256 minted,uint256 remaining,uint256 epoch,uint256 epochBlocksLeft_)
        function genesisState() view returns (uint256,uint256,uint256,bool)
        function mine(uint256 nonce)
    ]"#
);

pub struct MiningState {
    pub era: U256,
    pub reward: U256,
    pub difficulty: U256,
    pub epoch: U256,
}

pub struct Hash256Client {
    address: Option<Address>,
    read_contract: Hash256Contract<Provider<Http>>,
    write_contract: Option<Hash256Contract<SignerMiddleware<Provider<Http>, LocalWallet>>>,
}

#[derive(Debug, Clone)]
pub struct TxReport {
    pub tx_hash: H256,
    pub block_number: U64,
}

impl Hash256Client {
    pub async fn read_only(rpc_url: &str) -> Result<Self> {
        let provider =
            Arc::new(Provider::<Http>::try_from(rpc_url).context("RPC_URL tidak valid")?);
        let contract = Hash256Contract::new(contract_address()?, provider);
        Ok(Self {
            address: None,
            read_contract: contract,
            write_contract: None,
        })
    }

    pub async fn with_wallet(rpc_url: &str, private_key: &str) -> Result<Self> {
        let provider = Provider::<Http>::try_from(rpc_url).context("RPC_URL tidak valid")?;
        let chain_id = provider
            .get_chainid()
            .await
            .context("gagal membaca chain id dari RPC")?
            .as_u64();
        let wallet: LocalWallet = private_key
            .parse::<LocalWallet>()
            .context("PRIVATE_KEY tidak valid")?
            .with_chain_id(chain_id);
        let address = wallet.address();
        let client = Arc::new(SignerMiddleware::new(provider, wallet));
        let read_provider =
            Arc::new(Provider::<Http>::try_from(rpc_url).context("RPC_URL tidak valid")?);

        Ok(Self {
            address: Some(address),
            read_contract: Hash256Contract::new(contract_address()?, read_provider),
            write_contract: Some(Hash256Contract::new(contract_address()?, client)),
        })
    }

    pub fn wallet_address(&self) -> Address {
        self.address.expect("wallet client must have an address")
    }

    pub async fn genesis_state(&self) -> Result<(U256, U256, U256, bool)> {
        self.read_contract
            .genesis_state()
            .call()
            .await
            .context("gagal membaca genesisState")
    }

    pub async fn mining_state(&self) -> Result<MiningState> {
        let state = self
            .read_contract
            .mining_state()
            .call()
            .await
            .context("gagal membaca miningState")?;

        Ok(MiningState {
            era: state.0,
            reward: state.1,
            difficulty: state.2,
            epoch: state.5,
        })
    }

    pub async fn get_challenge(&self) -> Result<[u8; 32]> {
        let address = self.address.context("wallet address belum tersedia")?;
        let challenge = self
            .read_contract
            .get_challenge(address)
            .call()
            .await
            .context("gagal membaca challenge")?;
        Ok(challenge)
    }

    pub async fn submit_solution(&self, nonce: U256, priority_fee_gwei: &str) -> Result<TxReport> {
        let contract = self
            .write_contract
            .as_ref()
            .context("write contract belum tersedia")?;
        let gas = estimate_gas(contract, nonce).await;
        let priority: U256 = parse_units(priority_fee_gwei, "gwei")
            .context("PRIORITY_FEE_GWEI tidak valid")?
            .into();

        let client = contract.client();
        let provider = client.provider();
        let block = provider
            .get_block(ethers::types::BlockNumber::Latest)
            .await
            .context("gagal membaca block latest")?;

        let mut call = contract.mine(nonce).gas(gas);
        let mut tx: Eip1559TransactionRequest = call.tx.clone().into();
        tx.max_priority_fee_per_gas = Some(priority);
        tx.max_fee_per_gas = if let Some(base_fee) = block.and_then(|b| b.base_fee_per_gas) {
            Some(base_fee * U256::from(3) + priority)
        } else {
            let fallback_max_fee: U256 = parse_units("10", "gwei")?.into();
            Some(fallback_max_fee + priority)
        };
        call.tx = TypedTransaction::Eip1559(tx);

        let pending = call.send().await.context("TX failed saat submit mine")?;
        let tx_hash = pending.tx_hash();
        println!("TX sent: {:?}", tx_hash);
        let receipt = pending
            .await
            .context("gagal menunggu receipt")?
            .context("transaksi tidak punya receipt")?;
        let block_number = receipt.block_number.unwrap_or_default();
        println!("Success block: {}", block_number);
        Ok(TxReport {
            tx_hash,
            block_number,
        })
    }
}

async fn estimate_gas(
    contract: &Hash256Contract<SignerMiddleware<Provider<Http>, LocalWallet>>,
    nonce: U256,
) -> U256 {
    match contract.mine(nonce).estimate_gas().await {
        Ok(estimate) => {
            let padded = estimate * U256::from(3) / U256::from(2);
            padded.clamp(U256::from(200_000), U256::from(450_000))
        }
        Err(_) => U256::from(300_000),
    }
}

fn contract_address() -> Result<Address> {
    Address::from_str(CONTRACT_ADDRESS).context("contract address tidak valid")
}
