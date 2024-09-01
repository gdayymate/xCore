use crate::transaction::Transaction;
use crate::blockchain::{SignedBlock, FruitHeader, BlockType};
use blake3;
use hex;
use rs_merkle::{MerkleTree, MerkleProof, Hasher};
use std::time::{Duration, Instant};
use std::collections::{HashMap, VecDeque};
use thiserror::Error;

#[derive(Clone)]
pub struct TransactionHasher;

impl Hasher for TransactionHasher {
    type Hash = [u8; 32];

    fn hash(data: &[u8]) -> Self::Hash {
        blake3::hash(data).into()
    }
}

#[derive(Error, Debug)]
pub enum MempoolError {
    #[error("Mempool is full")]
    PoolFull,
    #[error("Failed to serialize transaction: {0}")]
    SerializationError(#[from] bincode::Error),
    #[error("Invalid transaction hash: {0}")]
    InvalidHash(String),
    #[error("Transaction not found")]
    TransactionNotFound,
    #[error("Fruit not found")]
    FruitNotFound,
}

pub struct Mempool {
    transaction_merkle_tree: MerkleTree<TransactionHasher>,
    fruit_merkle_tree: MerkleTree<TransactionHasher>,
    transactions: HashMap<[u8; 32], Transaction>,
    fruits: HashMap<[u8; 32], SignedBlock>,
    transaction_queue: VecDeque<[u8; 32]>,
    fruit_queue: VecDeque<[u8; 32]>,
    size_limit_bytes: usize,
    current_size_bytes: usize,
    transaction_timeout: Duration,
    fruit_timeout: Duration,
    last_cleanup: Instant,
}

impl Mempool {
    pub fn new(size_limit_mb: usize, transaction_timeout_secs: u64, fruit_timeout_secs: u64) -> Self {
        Mempool {
            transaction_merkle_tree: MerkleTree::<TransactionHasher>::new(),
            fruit_merkle_tree: MerkleTree::<TransactionHasher>::new(),
            transactions: HashMap::new(),
            fruits: HashMap::new(),
            transaction_queue: VecDeque::new(),
            fruit_queue: VecDeque::new(),
            size_limit_bytes: size_limit_mb * 1024 * 1024,
            current_size_bytes: 0,
            transaction_timeout: Duration::from_secs(transaction_timeout_secs),
            fruit_timeout: Duration::from_secs(fruit_timeout_secs),
            last_cleanup: Instant::now(),
        }
    }

    pub fn add_transaction(&mut self, transaction: Transaction) -> Result<(), MempoolError> {
        let transaction_size = bincode::serialize(&transaction)?.len();

        if self.current_size_bytes + transaction_size > self.size_limit_bytes {
            return Err(MempoolError::PoolFull);
        }

        let transaction_hash = transaction.hash();

        self.transaction_merkle_tree.insert(transaction_hash);
        self.transactions.insert(transaction_hash, transaction);
        self.transaction_queue.push_back(transaction_hash);
        self.current_size_bytes += transaction_size;
        self.transaction_merkle_tree.commit();

        Ok(())
    }

    pub fn add_fruit(&mut self, fruit: SignedBlock) -> Result<(), MempoolError> {
        if fruit.block.block_type != BlockType::Fruit {
            return Err(MempoolError::InvalidHash("Not a fruit block".to_string()));
        }

        let fruit_size = bincode::serialize(&fruit)?.len();

        if self.current_size_bytes + fruit_size > self.size_limit_bytes {
            return Err(MempoolError::PoolFull);
        }

        let fruit_hash = fruit.block.hash();

        self.fruit_merkle_tree.insert(fruit_hash);
        self.fruits.insert(fruit_hash, fruit);
        self.fruit_queue.push_back(fruit_hash);
        self.current_size_bytes += fruit_size;
        self.fruit_merkle_tree.commit();

        Ok(())
    }

    pub fn get_transactions(&self) -> Vec<Transaction> {
        self.transactions.values().cloned().collect()
    }

    pub fn get_fruits(&self) -> Vec<SignedBlock> {
        self.fruits.values().cloned().collect()
    }

    pub fn cleanup_expired(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_cleanup) < self.transaction_timeout.min(self.fruit_timeout) {
            return;
        }

        self.transaction_queue.retain(|hash| {
            if let Some(tx) = self.transactions.get(hash) {
                now.duration_since(tx.timestamp()) < self.transaction_timeout
            } else {
                false
            }
        });

        self.fruit_queue.retain(|hash| {
            if let Some(fruit) = self.fruits.get(hash) {
                now.duration_since(Instant::now() - Duration::from_secs(fruit.block.header.timestamp)) < self.fruit_timeout
            } else {
                false
            }
        });

        self.rebuild_merkle_trees();
        self.last_cleanup = now;
    }

    pub fn current_size_mb(&self) -> f64 {
        self.current_size_bytes as f64 / (1024.0 * 1024.0)
    }

    pub fn get_transaction_merkle_root(&self) -> [u8; 32] {
        self.transaction_merkle_tree.root().unwrap_or([0; 32])
    }

    pub fn get_fruit_merkle_root(&self) -> [u8; 32] {
        self.fruit_merkle_tree.root().unwrap_or([0; 32])
    }

    pub fn get_transaction_proof(&self, transaction_hash: &[u8; 32]) -> Option<Vec<[u8; 32]>> {
        let leaves = self.transaction_merkle_tree.leaves()?;
        let leaf_index = leaves.iter().position(|&x| x == *transaction_hash)?;
        Some(self.transaction_merkle_tree.proof(&[leaf_index]).proof_hashes().to_vec())
    }

    pub fn get_fruit_proof(&self, fruit_hash: &[u8; 32]) -> Option<Vec<[u8; 32]>> {
        let leaves = self.fruit_merkle_tree.leaves()?;
        let leaf_index = leaves.iter().position(|&x| x == *fruit_hash)?;
        Some(self.fruit_merkle_tree.proof(&[leaf_index]).proof_hashes().to_vec())
    }

    pub fn remove_transactions(&mut self, transactions: &[Transaction]) {
        for tx in transactions {
            let hash = tx.hash();
            self.transactions.remove(&hash);
            self.transaction_queue.retain(|&x| x != hash);
            if let Some(size) = bincode::serialize(tx).ok().map(|v| v.len()) {
                self.current_size_bytes = self.current_size_bytes.saturating_sub(size);
            }
        }
        self.rebuild_merkle_trees();
    }

    pub fn remove_fruits(&mut self, fruit_headers: &[FruitHeader]) {
        for header in fruit_headers {
            if let Some(fruit) = self.fruits.values().find(|f| f.block.fruit_header.as_ref() == Some(header)) {
                let hash = fruit.block.hash();
                self.fruits.remove(&hash);
                self.fruit_queue.retain(|&x| x != hash);
                if let Some(size) = bincode::serialize(fruit).ok().map(|v| v.len()) {
                    self.current_size_bytes = self.current_size_bytes.saturating_sub(size);
                }
            }
        }
        self.rebuild_merkle_trees();
    }

    fn rebuild_merkle_trees(&mut self) {
        self.transaction_merkle_tree = MerkleTree::<TransactionHasher>::new();
        self.fruit_merkle_tree = MerkleTree::<TransactionHasher>::new();

        for hash in &self.transaction_queue {
            self.transaction_merkle_tree.insert(*hash);
        }
        for hash in &self.fruit_queue {
            self.fruit_merkle_tree.insert(*hash);
        }

        self.transaction_merkle_tree.commit();
        self.fruit_merkle_tree.commit();
    }

    pub fn calculate_merkle_root(items: &[impl AsRef<[u8]>]) -> [u8; 32] {
        let mut tree = MerkleTree::<TransactionHasher>::new();
        for item in items {
            tree.insert(TransactionHasher::hash(item.as_ref()));
        }
        tree.commit();
        tree.root().unwrap_or([0; 32])
    }
}
