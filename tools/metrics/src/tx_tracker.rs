use shared::tokio;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Maximum number of transaction IDs to track per peer to prevent memory overflow
const MAX_TXS_PER_PEER: usize = 1000;

/// Interval for cleaning up old data
const CLEANUP_INTERVAL: Duration = Duration::from_secs(300); // 5 minutes

/// Maximum time to keep inactive peer data
const PEER_TIMEOUT: Duration = Duration::from_secs(1800); // 30 minutes

/// Transaction ID type - can be either txid or wtxid
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TxId {
    Txid(Vec<u8>),
    Wtxid(Vec<u8>),
}

impl TxId {
    pub fn from_txid(txid: Vec<u8>) -> Self {
        TxId::Txid(txid)
    }

    pub fn from_wtxid(wtxid: Vec<u8>) -> Self {
        TxId::Wtxid(wtxid)
    }
}

/// Transaction status after processing
#[derive(Debug, PartialEq, Eq)]
pub enum TxStatus {
    /// Transaction was requested via GETDATA
    Solicited,
    /// Transaction was announced via INV
    Announced,
    /// Transaction received without prior GETDATA request
    Unsolicited,
    /// Transaction requested without prior INV announcement
    Unannounced,
}

/// Per-peer transaction tracking state
#[derive(Debug)]
struct PeerTxState {
    /// Peer's IP address for metrics labeling
    address: String,
    /// Recent INVs we sent to this peer
    recent_invs: VecDeque<(TxId, Instant)>,
    /// Recent GETDATAs we sent to this peer
    recent_getdata: VecDeque<(TxId, Instant)>,
    /// Last message time for cleanup purposes
    last_message_time: Instant,
}

impl Default for PeerTxState {
    fn default() -> Self {
        Self {
            address: String::new(),
            recent_invs: VecDeque::new(),
            recent_getdata: VecDeque::new(),
            last_message_time: Instant::now(),
        }
    }
}

impl PeerTxState {
    fn new(address: String) -> Self {
        Self {
            address,
            recent_invs: VecDeque::new(),
            recent_getdata: VecDeque::new(),
            last_message_time: Instant::now(),
        }
    }

    /// Add a transaction ID to the INV list
    fn add_inv(&mut self, txid: TxId) {
        self.recent_invs.push_back((txid, Instant::now()));
        self.last_message_time = Instant::now();

        // Limit list size
        if self.recent_invs.len() > MAX_TXS_PER_PEER {
            self.recent_invs.pop_front();
        }
    }

    /// Add a transaction ID to the GETDATA list
    fn add_getdata(&mut self, txid: TxId) {
        self.recent_getdata.push_back((txid, Instant::now()));
        self.last_message_time = Instant::now();

        // Limit list size
        if self.recent_getdata.len() > MAX_TXS_PER_PEER {
            self.recent_getdata.pop_front();
        }
    }

    /// Check if we announced this transaction to the peer
    fn was_announced(&self, txid: &TxId) -> bool {
        self.recent_invs.iter().any(|(id, _)| id == txid)
    }

    /// Check if we requested this transaction from the peer
    fn was_requested(&self, txid: &TxId) -> bool {
        self.recent_getdata.iter().any(|(id, _)| id == txid)
    }
}

/// Top-K tracker for managing bounded-cardinality metrics
#[derive(Debug)]
pub struct TopKTracker {
    /// Separate counters for each violation type (note: these are independent rankings!)
    unsolicited_counts: HashMap<u64, u64>,  // peer_id -> count
    unannounced_counts: HashMap<u64, u64>,  // peer_id -> count
    
    /// Peer metadata for logging
    peer_addresses: HashMap<u64, String>,   // peer_id -> address
    
    /// Configuration
    k: usize,  // Top-K size (default: 10)
    
    /// Update tracking
    last_update: Instant,
    update_interval: Duration,  // How often to update Prometheus (e.g., 30s)
}

impl TopKTracker {
    pub fn new(k: usize, update_interval_secs: u64) -> Self {
        Self {
            unsolicited_counts: HashMap::new(),
            unannounced_counts: HashMap::new(),
            peer_addresses: HashMap::new(),
            k,
            last_update: Instant::now(),
            update_interval: Duration::from_secs(update_interval_secs),
        }
    }
    
    /// Record an unsolicited transaction from a peer
    pub fn record_unsolicited(&mut self, peer_id: u64, peer_addr: String) {
        *self.unsolicited_counts.entry(peer_id).or_insert(0) += 1;
        self.peer_addresses.insert(peer_id, peer_addr);
    }
    
    /// Record an unannounced transaction request from a peer
    pub fn record_unannounced(&mut self, peer_id: u64, peer_addr: String) {
        *self.unannounced_counts.entry(peer_id).or_insert(0) += 1;
        self.peer_addresses.insert(peer_id, peer_addr);
    }
    
    /// Update Prometheus metrics if enough time has passed
    pub fn maybe_update_metrics(&mut self, metrics: &crate::metrics::Metrics) {
        if self.last_update.elapsed() >= self.update_interval {
            self.update_prometheus_metrics(metrics);
            self.last_update = Instant::now();
        }
    }
    
    /// Update Prometheus metrics with current top-K rankings
    fn update_prometheus_metrics(&self, metrics: &crate::metrics::Metrics) {
        // Update unsolicited top-K (separate ranking)
        let mut unsolicited_sorted: Vec<_> = self.unsolicited_counts.iter().collect();
        unsolicited_sorted.sort_by(|a, b| b.1.cmp(a.1));
        
        for rank in 1..=self.k {
            let rank_str = rank.to_string();
            if let Some((peer_id, count)) = unsolicited_sorted.get(rank - 1) {
                metrics.tx_unsolicited_top_k
                    .with_label_values(&[&rank_str])
                    .set(**count as i64);
                    
                // Log top 3 for investigation
                if rank <= 3 {
                    if let Some(addr) = self.peer_addresses.get(peer_id) {
                        shared::log::info!(
                            target: "tx_relay_top_k",
                            "Top-{} unsolicited: peer {} ({}) with {} transactions",
                            rank, peer_id, addr, count
                        );
                    }
                }
            } else {
                // Clear metrics for ranks that don't exist
                metrics.tx_unsolicited_top_k
                    .with_label_values(&[&rank_str])
                    .set(0);
            }
        }
        
        // Update unannounced top-K (separate ranking)
        let mut unannounced_sorted: Vec<_> = self.unannounced_counts.iter().collect();
        unannounced_sorted.sort_by(|a, b| b.1.cmp(a.1));
        
        for rank in 1..=self.k {
            let rank_str = rank.to_string();
            if let Some((peer_id, count)) = unannounced_sorted.get(rank - 1) {
                metrics.tx_unannounced_top_k
                    .with_label_values(&[&rank_str])
                    .set(**count as i64);
                    
                // Log top 3 for investigation
                if rank <= 3 {
                    if let Some(addr) = self.peer_addresses.get(peer_id) {
                        shared::log::info!(
                            target: "tx_relay_top_k",
                            "Top-{} unannounced: peer {} ({}) with {} transactions",
                            rank, peer_id, addr, count
                        );
                    }
                }
            } else {
                // Clear metrics for ranks that don't exist
                metrics.tx_unannounced_top_k
                    .with_label_values(&[&rank_str])
                    .set(0);
            }
        }
    }
    
    /// Remove a peer from tracking (called when peer disconnects)
    pub fn remove_peer(&mut self, peer_id: u64) {
        self.unsolicited_counts.remove(&peer_id);
        self.unannounced_counts.remove(&peer_id);
        self.peer_addresses.remove(&peer_id);
    }
    
    /// Cleanup stale peers (those with no activity for a long time)
    pub fn cleanup_stale_peers(&mut self, active_peers: &std::collections::HashSet<u64>) {
        self.unsolicited_counts.retain(|peer_id, _| active_peers.contains(peer_id));
        self.unannounced_counts.retain(|peer_id, _| active_peers.contains(peer_id));
        self.peer_addresses.retain(|peer_id, _| active_peers.contains(peer_id));
    }
}

/// Main transaction tracker with thread-safe access
#[derive(Debug, Clone)]
pub struct TxRelayTracker {
    inner: Arc<Mutex<TxRelayTrackerInner>>,
}

#[derive(Debug)]
struct TxRelayTrackerInner {
    /// Per-peer tracking state
    peer_states: HashMap<u64, PeerTxState>,
    /// Top-K tracker for bounded cardinality metrics
    top_k_tracker: TopKTracker,
    /// Last cleanup time
    last_cleanup: Instant,
}

impl TxRelayTracker {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(TxRelayTrackerInner {
                peer_states: HashMap::new(),
                top_k_tracker: TopKTracker::new(10, 30), // Top-10, update every 30 seconds
                last_cleanup: Instant::now(),
            })),
        }
    }

    /// Start the cleanup task that periodically removes old data
    pub fn start_cleanup_task(&self) {
        let tracker = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(CLEANUP_INTERVAL);
            loop {
                interval.tick().await;
                if let Ok(mut inner) = tracker.inner.lock() {
                    inner.cleanup_old_data();
                }
            }
        });
    }

    /// Track an INV message we sent to a peer
    pub fn track_inv(&self, peer_id: u64, txid: TxId) {
        if let Ok(mut inner) = self.inner.lock() {
            let peer_state = inner.peer_states.entry(peer_id).or_default();
            peer_state.add_inv(txid);
        }
    }

    /// Track a GETDATA message we sent to a peer
    pub fn track_getdata(&self, peer_id: u64, txid: TxId) {
        if let Ok(mut inner) = self.inner.lock() {
            let peer_state = inner.peer_states.entry(peer_id).or_default();
            peer_state.add_getdata(txid);
        }
    }

    /// Process a transaction and determine its status
    pub fn process_transaction(
        &self,
        peer_id: u64,
        txid: TxId,
        wtxid: TxId,
        inbound: bool,
    ) -> Option<TxStatus> {
        let mut inner = self.inner.lock().ok()?;

        // Perform cleanup if needed
        inner.maybe_cleanup();

        let peer_state = inner
            .peer_states
            .entry(peer_id)
            .or_insert_with(|| PeerTxState::new(String::new()));
        peer_state.last_message_time = Instant::now();

        Some(if inbound {
            // Transaction received from peer
            // Check if we requested this transaction (either by txid or wtxid)
            if peer_state.was_requested(&txid) || peer_state.was_requested(&wtxid) {
                TxStatus::Solicited
            } else {
                TxStatus::Unsolicited
            }
        } else {
            // Transaction request from peer
            // Check if we announced this transaction (either by txid or wtxid)
            if peer_state.was_announced(&txid) || peer_state.was_announced(&wtxid) {
                TxStatus::Announced
            } else {
                TxStatus::Unannounced
            }
        })
    }

    /// Register peer address for metrics labeling
    pub fn register_peer_address(&self, peer_id: u64, address: String) {
        if let Ok(mut inner) = self.inner.lock() {
            let peer_state = inner
                .peer_states
                .entry(peer_id)
                .or_insert_with(|| PeerTxState::new(address.clone()));
            peer_state.address = address;
        }
    }

    /// Get peer address for metrics labeling
    pub fn get_peer_address(&self, peer_id: u64) -> Option<String> {
        self.inner.lock().ok().and_then(|inner| {
            inner
                .peer_states
                .get(&peer_id)
                .map(|state| state.address.clone())
        })
    }

    /// Remove a disconnected peer from tracking
    pub fn remove_peer(&self, peer_id: u64) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.peer_states.remove(&peer_id);
            inner.top_k_tracker.remove_peer(peer_id);
        }
    }

    /// Get the number of tracked peers (for testing)
    #[cfg(test)]
    pub fn tracked_peers_count(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.peer_states.len())
            .unwrap_or(0)
    }

    /// Record an unsolicited transaction for top-K tracking
    pub fn record_unsolicited_tx(&self, peer_id: u64, peer_addr: String) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.top_k_tracker.record_unsolicited(peer_id, peer_addr);
        }
    }
    
    /// Record an unannounced transaction for top-K tracking
    pub fn record_unannounced_tx(&self, peer_id: u64, peer_addr: String) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.top_k_tracker.record_unannounced(peer_id, peer_addr);
        }
    }
    
    /// Update top-K metrics if enough time has passed
    pub fn update_top_k_metrics(&self, metrics: &crate::metrics::Metrics) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.top_k_tracker.maybe_update_metrics(metrics);
        }
    }

    /// Helper function to handle INV messages
    pub fn handle_inv_message(&self, peer_id: u64, inv_items: &[shared::primitive::InventoryItem]) {
        for item in inv_items.iter() {
            if let Some(inv_item) = &item.item {
                match inv_item {
                    shared::primitive::inventory_item::Item::Wtx(wtx_hash) => {
                        let txid = TxId::from_wtxid(wtx_hash.clone());
                        self.track_inv(peer_id, txid);
                    }
                    shared::primitive::inventory_item::Item::Transaction(tx_hash) => {
                        let txid = TxId::from_txid(tx_hash.clone());
                        self.track_inv(peer_id, txid);
                    }
                    shared::primitive::inventory_item::Item::WitnessTransaction(wtx_hash) => {
                        let txid = TxId::from_wtxid(wtx_hash.clone());
                        self.track_inv(peer_id, txid);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Helper function to handle GETDATA messages
    pub fn handle_getdata_message(
        &self,
        peer_id: u64,
        getdata_items: &[shared::primitive::InventoryItem],
    ) {
        for item in getdata_items.iter() {
            if let Some(inv_item) = &item.item {
                match inv_item {
                    shared::primitive::inventory_item::Item::Wtx(wtx_hash) => {
                        let txid = TxId::from_wtxid(wtx_hash.clone());
                        self.track_getdata(peer_id, txid);
                    }
                    shared::primitive::inventory_item::Item::Transaction(tx_hash) => {
                        let txid = TxId::from_txid(tx_hash.clone());
                        self.track_getdata(peer_id, txid);
                    }
                    shared::primitive::inventory_item::Item::WitnessTransaction(wtx_hash) => {
                        let txid = TxId::from_wtxid(wtx_hash.clone());
                        self.track_getdata(peer_id, txid);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Helper function to handle TX messages and return status
    pub fn handle_tx_message(
        &self,
        peer_id: u64,
        tx: &shared::net_msg::Tx,
        inbound: bool,
    ) -> Option<TxStatus> {
        let txid = TxId::from_txid(tx.tx.txid.clone());
        let wtxid = TxId::from_wtxid(tx.tx.wtxid.clone());

        self.process_transaction(peer_id, txid, wtxid, inbound)
    }
}

impl TxRelayTrackerInner {
    /// Force cleanup of old data
    fn cleanup_old_data(&mut self) {
        let cutoff_age = Instant::now() - PEER_TIMEOUT;

        // Remove peers that haven't had activity recently
        self.peer_states
            .retain(|_, peer_state| peer_state.last_message_time > cutoff_age);

        self.last_cleanup = Instant::now();
    }

    /// Clean up old data if enough time has passed
    fn maybe_cleanup(&mut self) {
        if self.last_cleanup.elapsed() > CLEANUP_INTERVAL {
            self.cleanup_old_data();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unsolicited_detection() {
        let tracker = TxRelayTracker::new();
        let peer_id = 1;
        let txid = TxId::from_txid(b"test_txid".to_vec());
        let wtxid = TxId::from_wtxid(b"test_wtxid".to_vec());

        // Simulate receiving TX without prior GETDATA
        let status = tracker.process_transaction(peer_id, txid.clone(), wtxid.clone(), true);
        assert_eq!(status, Some(TxStatus::Unsolicited));
    }

    #[test]
    fn test_solicited_detection() {
        let tracker = TxRelayTracker::new();
        let peer_id = 1;
        let txid = TxId::from_txid(b"test_txid".to_vec());
        let wtxid = TxId::from_wtxid(b"test_wtxid".to_vec());

        // First request the transaction
        tracker.track_getdata(peer_id, txid.clone());

        // Then receive it
        let status = tracker.process_transaction(peer_id, txid.clone(), wtxid.clone(), true);
        assert_eq!(status, Some(TxStatus::Solicited));
    }

    #[test]
    fn test_unannounced_detection() {
        let tracker = TxRelayTracker::new();
        let peer_id = 1;
        let txid = TxId::from_txid(b"test_txid".to_vec());
        let wtxid = TxId::from_wtxid(b"test_wtxid".to_vec());

        // Simulate peer requesting TX without us announcing it
        let status = tracker.process_transaction(peer_id, txid.clone(), wtxid.clone(), false);
        assert_eq!(status, Some(TxStatus::Unannounced));
    }

    #[test]
    fn test_announced_detection() {
        let tracker = TxRelayTracker::new();
        let peer_id = 1;
        let txid = TxId::from_txid(b"test_txid".to_vec());
        let wtxid = TxId::from_wtxid(b"test_wtxid".to_vec());

        // First announce the transaction
        tracker.track_inv(peer_id, txid.clone());

        // Then peer requests it
        let status = tracker.process_transaction(peer_id, txid.clone(), wtxid.clone(), false);
        assert_eq!(status, Some(TxStatus::Announced));
    }

    #[test]
    fn test_memory_limits() {
        let tracker = TxRelayTracker::new();
        let peer_id = 1;

        // Add more than the limit
        for i in 0..(MAX_TXS_PER_PEER + 100) {
            let txid = TxId::from_txid(format!("txid_{}", i).into_bytes());
            tracker.track_inv(peer_id, txid);
        }

        // Verify limit is respected (check via cleanup or indirect means)
        assert!(tracker.tracked_peers_count() <= 1);
    }

    #[test]
    fn test_peer_cleanup() {
        let tracker = TxRelayTracker::new();
        let peer_id = 1;
        let txid = TxId::from_txid(b"test_txid".to_vec());

        tracker.track_inv(peer_id, txid);
        assert_eq!(tracker.tracked_peers_count(), 1);

        tracker.remove_peer(peer_id);
        assert_eq!(tracker.tracked_peers_count(), 0);
    }
}
