use bytes::Bytes;
use commonware_consensus::{
    simplex::{mocks::relay, types::Context},
    types::{Epoch, Round},
    Automaton, CertifiableAutomaton, Relay,
};
use commonware_cryptography::{
    ed25519::PublicKey as Ed25519PublicKey, sha256::Digest as Sha256Digest, Hasher, Sha256,
};
use commonware_macros::select_loop;
use commonware_runtime::{spawn_cell, Clock, ContextCell, Handle, Spawner};
use commonware_utils::channel::{
    fallible::{AsyncFallibleExt, OneshotExt},
    mpsc, oneshot,
};
use std::{collections::HashMap, sync::Arc};

// =============================================================================
// Block and Transaction types
// =============================================================================

/// Number of pre-funded genesis accounts.
pub const NUM_ACCOUNTS: u8 = 4;
/// Initial token balance for each genesis account.
pub const GENESIS_BALANCE: u64 = 1_000_000;

/// A simple token transfer between two accounts.
#[derive(Clone, Debug)]
pub struct Transaction {
    /// Source account index (0..NUM_ACCOUNTS).
    pub from: u8,
    /// Destination account index (0..NUM_ACCOUNTS).
    pub to: u8,
    /// Token amount.
    pub amount: u64,
}

impl Transaction {
    const SIZE: usize = 1 + 1 + 8; // from (1) + to (1) + amount (8)

    fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.push(self.from);
        buf.push(self.to);
        buf.extend_from_slice(&self.amount.to_le_bytes());
    }

    fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }
        Some(Transaction {
            from: data[0],
            to: data[1],
            amount: u64::from_le_bytes(data[2..10].try_into().ok()?),
        })
    }
}

/// A block containing an ordered list of token transfers.
#[derive(Clone, Debug)]
pub struct Block {
    /// Consensus view number this block was proposed for.
    pub view: u64,
    /// Digest of the parent block.
    pub parent: Sha256Digest,
    /// Ordered list of transactions.
    pub transactions: Vec<Transaction>,
}

impl Block {
    fn encode_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.view.to_le_bytes());
        buf.extend_from_slice(self.parent.as_ref());
        buf.extend_from_slice(&(self.transactions.len() as u32).to_le_bytes());
        for tx in &self.transactions {
            tx.encode_into(&mut buf);
        }
        buf
    }

    fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < 8 + 32 + 4 {
            return None;
        }
        let view = u64::from_le_bytes(data[0..8].try_into().ok()?);
        let parent_arr: [u8; 32] = data[8..40].try_into().ok()?;
        let parent = Sha256Digest::from(parent_arr);
        let tx_count = u32::from_le_bytes(data[40..44].try_into().ok()?) as usize;
        let mut offset = 44usize;
        if data.len() < offset + tx_count * Transaction::SIZE {
            return None;
        }
        let mut transactions = Vec::with_capacity(tx_count);
        for _ in 0..tx_count {
            transactions.push(Transaction::decode(&data[offset..])?);
            offset += Transaction::SIZE;
        }
        Some(Block { view, parent, transactions })
    }

    fn hash(&self, hasher: &mut Sha256) -> Sha256Digest {
        hasher.update(&self.encode_bytes());
        hasher.finalize()
    }
}

// =============================================================================
// Actor message channel
// =============================================================================

type AppContext = Context<Sha256Digest, Ed25519PublicKey>;

enum Message {
    Genesis {
        epoch: Epoch,
        response: oneshot::Sender<Sha256Digest>,
    },
    Propose {
        context: AppContext,
        response: oneshot::Sender<Sha256Digest>,
    },
    Verify {
        context: AppContext,
        payload: Sha256Digest,
        response: oneshot::Sender<bool>,
    },
    Certify {
        response: oneshot::Sender<bool>,
    },
    Broadcast {
        payload: Sha256Digest,
    },
}

// =============================================================================
// Mailbox — the Clone+Send handle implementing consensus traits
// =============================================================================

#[derive(Clone)]
pub struct Mailbox {
    tx: mpsc::Sender<Message>,
}

impl Mailbox {
    fn new(tx: mpsc::Sender<Message>) -> Self {
        Self { tx }
    }
}

impl Automaton for Mailbox {
    type Context = AppContext;
    type Digest = Sha256Digest;

    async fn genesis(&mut self, epoch: Epoch) -> Self::Digest {
        let (response, rx) = oneshot::channel();
        self.tx.send_lossy(Message::Genesis { epoch, response }).await;
        rx.await.expect("genesis actor dropped")
    }

    async fn propose(&mut self, context: Self::Context) -> oneshot::Receiver<Self::Digest> {
        let (response, rx) = oneshot::channel();
        self.tx.send_lossy(Message::Propose { context, response }).await;
        rx
    }

    async fn verify(
        &mut self,
        context: Self::Context,
        payload: Self::Digest,
    ) -> oneshot::Receiver<bool> {
        let (response, rx) = oneshot::channel();
        self.tx
            .send_lossy(Message::Verify { context, payload, response })
            .await;
        rx
    }
}

impl CertifiableAutomaton for Mailbox {
    async fn certify(
        &mut self,
        _round: Round,
        _payload: Self::Digest,
    ) -> oneshot::Receiver<bool> {
        let (response, rx) = oneshot::channel();
        self.tx.send_lossy(Message::Certify { response }).await;
        rx
    }
}

impl Relay for Mailbox {
    type Digest = Sha256Digest;

    async fn broadcast(&mut self, payload: Self::Digest) {
        self.tx.send_lossy(Message::Broadcast { payload }).await;
    }
}

// =============================================================================
// Application actor — runs the state machine
// =============================================================================

/// Krypto state machine: maintains account balances and produces/validates blocks.
pub struct Application<E: Clock + Spawner> {
    context: ContextCell<E>,
    /// This validator's public key (for relay registration and broadcast).
    me: Ed25519PublicKey,
    /// Validator index, used as the "sender" account when proposing.
    my_index: u8,
    hasher: Sha256,

    relay: Arc<relay::Relay<Sha256Digest, Ed25519PublicKey>>,
    /// Incoming (digest, bytes) pairs from other validators via the relay.
    broadcast_rx: mpsc::UnboundedReceiver<(Sha256Digest, Bytes)>,
    mailbox: mpsc::Receiver<Message>,

    /// Committed account balances. Starts at genesis values.
    /// Phase 2 note: balances are only updated at genesis; cross-block
    /// state tracking will be added in a later phase.
    balances: HashMap<u8, u64>,

    /// Blocks we have proposed but not yet broadcast.
    pending: HashMap<Sha256Digest, Bytes>,
    /// Blocks received from other validators.
    seen: HashMap<Sha256Digest, Bytes>,
    /// Verify requests waiting for block content to arrive from the relay.
    waiters: HashMap<Sha256Digest, Vec<(AppContext, oneshot::Sender<bool>)>>,
}

impl<E: Clock + Spawner> Application<E> {
    /// Create a new Application actor and its corresponding `Mailbox` handle.
    pub fn new(
        context: E,
        me: Ed25519PublicKey,
        my_index: u8,
        relay: Arc<relay::Relay<Sha256Digest, Ed25519PublicKey>>,
    ) -> (Self, Mailbox) {
        let broadcast_rx = relay.register(me.clone());
        let (tx, rx) = mpsc::channel(1024);
        let balances = (0..NUM_ACCOUNTS).map(|i| (i, GENESIS_BALANCE)).collect();
        let app = Self {
            context: ContextCell::new(context),
            me,
            my_index,
            hasher: Sha256::default(),
            relay,
            broadcast_rx,
            mailbox: rx,
            balances,
            pending: HashMap::new(),
            seen: HashMap::new(),
            waiters: HashMap::new(),
        };
        (app, Mailbox::new(tx))
    }

    /// Compute the genesis digest (deterministic hash of the initial state).
    fn do_genesis(&mut self, epoch: Epoch) -> Sha256Digest {
        // Hash: tag || epoch || (account_index || balance)*
        let mut buf = Vec::new();
        buf.extend_from_slice(b"krypto_genesis_v1");
        buf.extend_from_slice(&epoch.get().to_le_bytes());
        for i in 0..NUM_ACCOUNTS {
            buf.push(i);
            buf.extend_from_slice(&GENESIS_BALANCE.to_le_bytes());
        }
        self.hasher.update(&buf);
        let digest = self.hasher.finalize();
        println!("[validator_{}] genesis digest: {:?}", self.my_index, digest);
        digest
    }

    /// Build a new block for the given consensus context.
    fn do_propose(&mut self, context: AppContext) -> Sha256Digest {
        let view = context.round.view().get();
        let to = (self.my_index + 1) % NUM_ACCOUNTS;
        let block = Block {
            view,
            parent: context.parent.1,
            transactions: vec![Transaction {
                from: self.my_index,
                to,
                amount: 1,
            }],
        };
        let encoded = Bytes::from(block.encode_bytes());
        let digest = block.hash(&mut self.hasher);
        println!(
            "[validator_{}] propose view={} tx: {} -> {} (1 token) digest={:?}",
            self.my_index, view, self.my_index, to, digest
        );
        self.pending.insert(digest, encoded);
        digest
    }

    /// Verify a block's content against the committed state.
    fn do_verify(
        &self,
        context: &AppContext,
        digest: Sha256Digest,
        data: &Bytes,
    ) -> bool {
        let Some(block) = Block::decode(data) else {
            eprintln!("[validator_{}] verify: decode failed", self.my_index);
            return false;
        };

        // Check view matches the consensus context.
        let expected_view = context.round.view().get();
        if block.view != expected_view {
            eprintln!(
                "[validator_{}] verify: view mismatch (block={} expected={})",
                self.my_index, block.view, expected_view
            );
            return false;
        }

        // Check parent digest matches the consensus context.
        if block.parent != context.parent.1 {
            eprintln!("[validator_{}] verify: parent mismatch", self.my_index);
            return false;
        }

        // Simulate transaction application against current committed balances.
        let mut sim = self.balances.clone();
        for tx in &block.transactions {
            if tx.from >= NUM_ACCOUNTS || tx.to >= NUM_ACCOUNTS || tx.from == tx.to {
                eprintln!(
                    "[validator_{}] verify: invalid accounts from={} to={}",
                    self.my_index, tx.from, tx.to
                );
                return false;
            }
            let bal = sim.get(&tx.from).copied().unwrap_or(0);
            if bal < tx.amount {
                eprintln!(
                    "[validator_{}] verify: insufficient balance (bal={} amount={})",
                    self.my_index, bal, tx.amount
                );
                return false;
            }
            *sim.get_mut(&tx.from).unwrap() -= tx.amount;
            *sim.entry(tx.to).or_insert(0) += tx.amount;
        }

        // Verify that the block content matches the announced digest.
        let mut h = Sha256::default();
        if block.hash(&mut h) != digest {
            eprintln!("[validator_{}] verify: digest mismatch", self.my_index);
            return false;
        }

        println!(
            "[validator_{}] verified block view={} txs={}",
            self.my_index,
            block.view,
            block.transactions.len()
        );
        true
    }

    /// Send a pending block's content to all other validators via the relay.
    fn do_broadcast(&mut self, payload: Sha256Digest) {
        if let Some(data) = self.pending.remove(&payload) {
            self.relay.broadcast(&self.me, (payload, data));
        }
    }

    /// Spawn the application actor. Consumes `self`.
    pub fn start(mut self) -> Handle<()> {
        spawn_cell!(self.context, self.run().await)
    }

    async fn run(mut self) {
        select_loop! {
            self.context,
            on_stopped => {
                // Clean shutdown — nothing to flush.
            },
            Some(msg) = self.mailbox.recv() else break => {
                match msg {
                    Message::Genesis { epoch, response } => {
                        let d = self.do_genesis(epoch);
                        response.send_lossy(d);
                    }
                    Message::Propose { context, response } => {
                        let d = self.do_propose(context);
                        response.send_lossy(d);
                    }
                    Message::Verify { context, payload, response } => {
                        let data = self.seen.get(&payload).cloned()
                            .or_else(|| self.pending.get(&payload).cloned());
                        if let Some(data) = data {
                            let ok = self.do_verify(&context, payload, &data);
                            response.send_lossy(ok);
                        } else {
                            // Block not yet received — wait for relay broadcast.
                            self.waiters
                                .entry(payload)
                                .or_default()
                                .push((context, response));
                        }
                    }
                    Message::Certify { response } => {
                        // Always certify for now.
                        response.send_lossy(true);
                    }
                    Message::Broadcast { payload } => {
                        self.do_broadcast(payload);
                    }
                }
            },
            Some((digest, data)) = self.broadcast_rx.recv() else break => {
                self.seen.insert(digest, data.clone());
                // Resolve any pending verify requests for this block.
                if let Some(waiters) = self.waiters.remove(&digest) {
                    for (ctx, sender) in waiters {
                        let ok = self.do_verify(&ctx, digest, &data);
                        sender.send_lossy(ok);
                    }
                }
            },
        }
    }
}
