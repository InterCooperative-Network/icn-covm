use crate::federation::{
    behaviour::{create_behaviour, IcnBehaviour, IcnBehaviourEvent},
    error::FederationError,
    events::NetworkEvent,
    messages::{FederatedProposal, FederatedVote, NetworkMessage, NodeAnnouncement},
    storage::FederationStorage,
};

use futures::{channel::mpsc, stream::StreamExt, SinkExt};
use libp2p::{
    core::upgrade, identity, noise, swarm::SwarmEvent, tcp, yamux, Multiaddr, PeerId, Swarm,
    Transport,
};

// Protocol-specific imports
use libp2p::identify;
use libp2p::kad;
use libp2p::mdns;
use libp2p::ping;

use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use tokio::time::sleep;

/// Maximum number of retry attempts for network operations
const MAX_RETRY_ATTEMPTS: u32 = 5;
/// Base delay for exponential backoff (in milliseconds)
const BASE_RETRY_DELAY_MS: u64 = 500;
/// Maximum delay for exponential backoff (in milliseconds)
const MAX_RETRY_DELAY_MS: u64 = 30_000; // 30 seconds

/// Retry status for operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryStatus {
    /// Last attempt time (unix timestamp)
    pub last_attempt: u64,
    /// Number of attempts so far
    pub attempts: u32,
    /// Whether the operation is in backoff
    pub in_backoff: bool,
    /// Current backoff delay in milliseconds
    pub current_delay_ms: u64,
    /// Retry operation type
    pub operation: String,
    /// Target of the operation (e.g., peer ID)
    pub target: String,
}

/// Manages retries for network operations with exponential backoff
pub struct RetryManager {
    /// Map of operation_key -> retry status
    retries: HashMap<String, RetryStatus>,
    /// Maximum number of retry attempts
    max_attempts: u32,
    /// Base delay for exponential backoff
    base_delay_ms: u64,
    /// Maximum delay for exponential backoff
    max_delay_ms: u64,
}

impl RetryManager {
    /// Create a new retry manager with defaults
    pub fn new() -> Self {
        Self {
            retries: HashMap::new(),
            max_attempts: MAX_RETRY_ATTEMPTS,
            base_delay_ms: BASE_RETRY_DELAY_MS,
            max_delay_ms: MAX_RETRY_DELAY_MS,
        }
    }

    /// Record a failed attempt and get the next retry delay
    pub fn record_failure(&mut self, operation: &str, target: &str) -> Option<Duration> {
        let key = format!("{}:{}", operation, target);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let status = self.retries.entry(key.clone()).or_insert_with(|| RetryStatus {
            last_attempt: now,
            attempts: 0,
            in_backoff: false,
            current_delay_ms: self.base_delay_ms,
            operation: operation.to_string(),
            target: target.to_string(),
        });

        status.attempts += 1;
        status.last_attempt = now;

        if status.attempts >= self.max_attempts {
            // Too many failures, give up
            self.retries.remove(&key);
            return None;
        }

        // Calculate exponential backoff with jitter
        let mut delay_ms = self.base_delay_ms * (2_u64.pow(status.attempts - 1));
        // Add some randomness (10% jitter)
        let jitter = (delay_ms as f64 * 0.1 * (rand::random::<f64>() - 0.5)) as u64;
        delay_ms = (delay_ms + jitter).min(self.max_delay_ms);

        status.current_delay_ms = delay_ms;
        status.in_backoff = true;

        Some(Duration::from_millis(delay_ms))
    }

    /// Record a successful operation
    pub fn record_success(&mut self, operation: &str, target: &str) {
        let key = format!("{}:{}", operation, target);
        self.retries.remove(&key);
    }

    /// Check if an operation is currently in backoff
    pub fn is_in_backoff(&self, operation: &str, target: &str) -> bool {
        let key = format!("{}:{}", operation, target);
        self.retries.get(&key).map_or(false, |status| status.in_backoff)
    }

    /// Get all current retry operations
    pub fn get_all_retries(&self) -> Vec<RetryStatus> {
        self.retries.values().cloned().collect()
    }
}

/// Configuration options for a network node
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// Optional fixed port to listen on (otherwise uses an ephemeral port)
    pub port: Option<u16>,

    /// List of bootstrap nodes to connect to when starting
    pub bootstrap_nodes: Vec<Multiaddr>,

    /// Node's human-readable name
    pub name: Option<String>,

    /// Node capabilities (services/features provided)
    pub capabilities: Vec<String>,

    /// Protocol version
    pub protocol_version: String,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            port: None,
            bootstrap_nodes: Vec::new(),
            name: None,
            capabilities: Vec::new(),
            protocol_version: "1.0.0".to_string(),
        }
    }
}

/// Main network node for the federation layer
pub struct NetworkNode {
    /// Libp2p swarm that handles network events
    swarm: Swarm<IcnBehaviour>,

    /// Local peer ID
    local_peer_id: PeerId,

    /// Network configuration
    config: NodeConfig,

    /// Flag indicating if the node is running
    running: Arc<AtomicBool>,

    /// Channel for receiving network events
    event_receiver: mpsc::Receiver<NetworkEvent>,

    /// Channel for sending network events
    event_sender: mpsc::Sender<NetworkEvent>,

    /// Store tracking known peers
    known_peers: Arc<Mutex<HashSet<PeerId>>>,

    /// Storage for federation proposals and votes
    federation_storage: Arc<FederationStorage>,
    
    /// Retry manager for network operations
    retry_manager: Arc<Mutex<RetryManager>>,
    
    /// Node start time (for uptime tracking)
    start_time: Option<u64>,
    
    /// Node role (if assigned)
    role: Option<String>,
}

impl NetworkNode {
    /// Create a new network node with the specified configuration
    pub async fn new(config: NodeConfig) -> Result<Self, FederationError> {
        // Generate a random keypair for this node
        let local_key = identity::Keypair::generate_ed25519();
        let local_peer_id = PeerId::from(local_key.public());

        // Create the network behavior
        let behaviour = create_behaviour(&local_key, config.protocol_version.clone())
            .await
            .map_err(|e| {
                FederationError::NetworkError(format!("Failed to create network behavior: {}", e))
            })?;

        // Create the transport and swarm
        let swarm = create_swarm(local_key, behaviour)?;

        // Create a channel for network events
        let (event_sender, event_receiver) = mpsc::channel::<NetworkEvent>(32);

        // Node start timestamp
        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs());

        Ok(Self {
            swarm,
            local_peer_id,
            config,
            running: Arc::new(AtomicBool::new(false)),
            event_receiver,
            event_sender,
            known_peers: Arc::new(Mutex::new(HashSet::new())),
            federation_storage: Arc::new(FederationStorage::new()),
            retry_manager: Arc::new(Mutex::new(RetryManager::new())),
            start_time,
            role: None,
        })
    }

    /// Start the network node and begin processing events
    pub async fn start(&mut self) -> Result<(), FederationError> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        // Set the running flag
        self.running.store(true, Ordering::SeqCst);

        // Listen on the provided port or an ephemeral port
        let listen_addr = match self.config.port {
            Some(port) => format!("/ip4/0.0.0.0/tcp/{}", port),
            None => "/ip4/0.0.0.0/tcp/0".to_string(),
        };

        match self.swarm.listen_on(listen_addr.parse()?) {
            Ok(_) => {
                info!("Node listening for connections");
            }
            Err(e) => {
                error!("Failed to listen: {}", e);
                return Err(FederationError::NetworkError(format!(
                    "Failed to listen: {}",
                    e
                )));
            }
        }

        // Connect to bootstrap nodes with retry logic
        self.connect_to_bootstrap_nodes().await;

        // Create node announcement
        let announcement = self.create_node_announcement();
        debug!("Created node announcement: {:?}", announcement);

        // Start the retry processing task
        let retry_manager = self.retry_manager.clone();
        let event_sender = self.event_sender.clone();
        let running = self.running.clone();
        
        tokio::spawn(async move {
            while running.load(Ordering::SeqCst) {
                // Check for expired backoffs every 1 second
                sleep(Duration::from_secs(1)).await;
                
                let mut retries_to_resume = Vec::new();
                {
                    let mut manager = retry_manager.lock().await;
                    let all_retries = manager.get_all_retries();
                    
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                        
                    for retry in all_retries {
                        if retry.in_backoff {
                            // Check if backoff period has elapsed
                            let elapsed_ms = (now - retry.last_attempt) * 1000;
                            if elapsed_ms >= retry.current_delay_ms {
                                retries_to_resume.push((retry.operation.clone(), retry.target.clone()));
                            }
                        }
                    }
                }
                
                // Resume retries outside of the lock
                for (operation, target) in retries_to_resume {
                    // Emit a retry event to be handled by the main event loop
                    let retry_event = NetworkEvent::RetryOperation {
                        operation,
                        target,
                    };
                    if let Err(e) = event_sender.clone().send(retry_event).await {
                        error!("Failed to send retry event: {}", e);
                    }
                }
            }
        });

        // Start the event loop
        self.process_events().await?;

        Ok(())
    }

    /// Stop the network node
    pub async fn stop(&mut self) {
        info!("Stopping network node");
        self.running.store(false, Ordering::SeqCst);
    }

    /// Get the local peer ID
    pub fn local_peer_id(&self) -> &PeerId {
        &self.local_peer_id
    }

    /// Create a node announcement message
    fn create_node_announcement(&self) -> NodeAnnouncement {
        NodeAnnouncement {
            node_id: self.local_peer_id.to_string(),
            capabilities: self.config.capabilities.clone(),
            version: self.config.protocol_version.clone(),
            name: self.config.name.clone(),
        }
    }

    /// Process network events in a loop
    async fn process_events(&mut self) -> Result<(), FederationError> {
        info!("Starting network event processing loop");

        while self.running.load(Ordering::SeqCst) {
            tokio::select! {
                swarm_event = self.swarm.select_next_some() => {
                    if let Err(e) = self.handle_swarm_event(swarm_event).await {
                        error!("Error handling swarm event: {}", e);
                    }
                }
                
                network_event = self.event_receiver.next() => {
                    if let Some(event) = network_event {
                        if let Err(e) = self.handle_network_event(event).await {
                            error!("Error handling network event: {}", e);
                        }
                    }
                }
                
                // Add a timeout to prevent CPU spinning
                _ = sleep(Duration::from_millis(10)) => {}
            }
        }

        Ok(())
    }

    /// Handle events from the libp2p swarm
    async fn handle_swarm_event(
        &mut self,
        event: SwarmEvent<IcnBehaviourEvent, impl std::error::Error + Send + Sync>,
    ) -> Result<(), FederationError> {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Node listening on {}", address);
            }

            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                info!("Connected to {}", peer_id);

                // Add peer to Kademlia routing table if using discovered address
                let remote_addr = endpoint.get_remote_address();
                debug!(
                    "Adding {} with address {} to Kademlia",
                    peer_id, remote_addr
                );
                self.swarm
                    .behaviour_mut()
                    .kademlia
                    .add_address(&peer_id, remote_addr.clone());

                // Add peer to known peers
                let mut peers = self.known_peers.lock().await;
                peers.insert(peer_id);

                // Notify about new connection
                let _ = self
                    .event_sender
                    .send(NetworkEvent::PeerConnected(peer_id))
                    .await;
            }

            SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                if let Some(error) = cause {
                    warn!("Connection to {} closed due to error: {:?}", peer_id, error);
                } else {
                    info!("Disconnected from {}", peer_id);
                }

                // Notify about disconnection
                let _ = self
                    .event_sender
                    .send(NetworkEvent::PeerDisconnected(peer_id))
                    .await;
            }

            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                if let Some(peer) = peer_id {
                    warn!("Error connecting to {}: {}", peer, error);
                } else {
                    warn!("Outgoing connection error: {}", error);
                }
            }

            SwarmEvent::IncomingConnectionError {
                local_addr,
                send_back_addr,
                error,
                ..
            } => {
                warn!(
                    "Error with incoming connection from {} to {}: {}",
                    send_back_addr, local_addr, error
                );
            }

            SwarmEvent::Dialing { peer_id, .. } => {
                if let Some(peer) = peer_id {
                    debug!("Dialing peer: {:?}", peer);
                } else {
                    debug!("Dialing unknown peer");
                }
            }

            SwarmEvent::ListenerClosed {
                addresses, reason, ..
            } => {
                warn!(
                    "Listener closed for addresses {:?}, reason: {:?}",
                    addresses, reason
                );
            }

            SwarmEvent::ListenerError { error, .. } => {
                error!("Listener error: {}", error);
            }

            SwarmEvent::Behaviour(behaviour_event) => {
                // Handle protocol-specific events
                self.handle_behaviour_event(behaviour_event).await?;
            }

            _ => {
                debug!("Unhandled swarm event: {:?}", event);
            }
        }

        Ok(())
    }

    /// Handle events from the network behavior
    async fn handle_behaviour_event(
        &mut self,
        event: IcnBehaviourEvent,
    ) -> Result<(), FederationError> {
        match event {
            IcnBehaviourEvent::Ping(ping_event) => self.handle_ping_event(ping_event).await,

            IcnBehaviourEvent::Kademlia(kad_event) => self.handle_kademlia_event(kad_event).await,

            IcnBehaviourEvent::Mdns(mdns_event) => self.handle_mdns_event(mdns_event).await,

            IcnBehaviourEvent::Identify(identify_event) => {
                self.handle_identify_event(*identify_event).await
            }
        }
    }

    /// Handle events from the ping protocol
    async fn handle_ping_event(&mut self, event: ping::Event) -> Result<(), FederationError> {
        match event {
            ping::Event {
                peer,
                result: Ok(rtt),
                ..
            } => {
                info!("Ping success from {}: RTT = {:?}", peer, rtt);
            }

            ping::Event {
                peer,
                result: Err(error),
                ..
            } => {
                warn!("Ping failure with {}: {}", peer, error);
            }
        }

        Ok(())
    }

    /// Handle events from the Kademlia DHT
    async fn handle_kademlia_event(&mut self, event: kad::Event) -> Result<(), FederationError> {
        match event {
            kad::Event::OutboundQueryProgressed {
                id,
                result: kad::QueryResult::GetClosestPeers(Ok(peers)),
                stats: _,
                ..
            } => {
                info!("Kademlia query {:?} found {} peers", id, peers.peers.len());

                let _ = self
                    .event_sender
                    .send(NetworkEvent::DhtQueryCompleted {
                        peers_found: peers.peers.clone(),
                        success: true,
                    })
                    .await;

                // Optionally dial discovered peers
                for peer in &peers.peers {
                    if !self.known_peers.lock().await.contains(peer) {
                        debug!("Discovered new peer via DHT: {}", peer);
                    }
                }
            }

            kad::Event::OutboundQueryProgressed {
                id,
                result: kad::QueryResult::Bootstrap(Ok(stats)),
                ..
            } => {
                info!(
                    "Kademlia bootstrap query {:?} completed with {} remaining peers",
                    id, stats.num_remaining
                );
            }

            kad::Event::OutboundQueryProgressed {
                id,
                result: kad::QueryResult::GetClosestPeers(Err(err)),
                ..
            } => {
                warn!("Kademlia GetClosestPeers query {:?} failed: {}", id, err);

                let _ = self
                    .event_sender
                    .send(NetworkEvent::DhtQueryCompleted {
                        peers_found: Vec::new(),
                        success: false,
                    })
                    .await;
            }

            kad::Event::OutboundQueryProgressed {
                id,
                result: kad::QueryResult::Bootstrap(Err(err)),
                ..
            } => {
                warn!("Kademlia bootstrap query {:?} failed: {}", id, err);
            }

            kad::Event::RoutingUpdated {
                peer,
                is_new_peer,
                addresses,
                ..
            } => {
                if is_new_peer {
                    debug!(
                        "New peer in routing table: {} with {} addresses",
                        peer,
                        addresses.len()
                    );
                    let _ = self
                        .event_sender
                        .send(NetworkEvent::PeerDiscovered(peer))
                        .await;
                }
            }

            _ => {
                debug!("Unhandled Kademlia event: {:?}", event);
            }
        }

        Ok(())
    }

    /// Handle events from the mDNS discovery
    async fn handle_mdns_event(&mut self, event: mdns::Event) -> Result<(), FederationError> {
        match event {
            mdns::Event::Discovered(list) => {
                for (peer, addr) in list {
                    info!("mDNS discovered peer {} at {}", peer, addr);

                    // Add address to Kademlia
                    self.swarm
                        .behaviour_mut()
                        .kademlia
                        .add_address(&peer, addr.clone());

                    // Notify about discovery
                    let _ = self
                        .event_sender
                        .send(NetworkEvent::PeerDiscovered(peer))
                        .await;

                    // Optionally, dial the peer if not already connected
                    let is_known = self.known_peers.lock().await.contains(&peer);
                    if !is_known {
                        debug!("Dialing newly discovered peer: {}", peer);
                        if let Err(e) = self.swarm.dial(addr.clone()) {
                            warn!("Failed to dial discovered peer {}: {}", peer, e);
                        }
                    }
                }
            }

            mdns::Event::Expired(list) => {
                for (peer, addr) in list {
                    debug!("mDNS peer {} at {} expired", peer, addr);
                }
            }
        }

        Ok(())
    }

    /// Handle events from the identify protocol
    async fn handle_identify_event(
        &mut self,
        event: identify::Event,
    ) -> Result<(), FederationError> {
        match event {
            identify::Event::Received { peer_id, info } => {
                info!(
                    "Received Identify info from {}: agent={}, protocol={}",
                    peer_id, info.agent_version, info.protocol_version
                );

                debug!("Protocols supported by {}: {:?}", peer_id, info.protocols);

                // Add all listen addresses to Kademlia
                for addr in info.listen_addrs {
                    debug!("Adding address {} for peer {}", addr, peer_id);
                    self.swarm
                        .behaviour_mut()
                        .kademlia
                        .add_address(&peer_id, addr.clone());
                }
            }

            identify::Event::Sent { peer_id } => {
                debug!("Sent Identify info to {}", peer_id);
            }

            identify::Event::Error { peer_id, error } => {
                warn!("Identify error with {}: {}", peer_id, error);
            }

            identify::Event::Pushed { .. } => {
                debug!("Identify push event received");
            }
        }

        Ok(())
    }

    /// Get a reference to the federation storage
    pub fn federation_storage(&self) -> Arc<FederationStorage> {
        self.federation_storage.clone()
    }

    /// Broadcast a proposal to the network
    pub async fn broadcast_proposal(
        &mut self,
        proposal: FederatedProposal,
    ) -> Result<(), FederationError> {
        info!("Broadcasting proposal: {}", proposal.proposal_id);

        // Create the proposal broadcast message
        let _message = NetworkMessage::ProposalBroadcast(proposal);

        // Get all connected peers
        let peer_ids = {
            let peers = self.known_peers.lock().await;
            peers.iter().cloned().collect::<Vec<_>>()
        };

        // Broadcast to all peers
        for peer_id in peer_ids {
            debug!("Sending proposal to peer: {}", peer_id);
            // In a real implementation, we would use a proper broadcast mechanism
            // For now, we're just simulating by sending to each peer individually
        }

        // Emit an event to notify listeners
        self.event_sender
            .try_send(NetworkEvent::ProposalBroadcasted)
            .map_err(|e| FederationError::NetworkError(format!("Failed to emit event: {}", e)))?;

        Ok(())
    }

    /// Submit a vote to the network
    pub async fn submit_vote(&mut self, vote: FederatedVote) -> Result<(), FederationError> {
        info!("Submitting vote from {}", vote.voter);

        // Create the vote submission message
        let _message = NetworkMessage::VoteSubmission(vote);

        // In a real implementation, we would send this to peers who have the proposal
        // For now, we just emit an event
        self.event_sender
            .try_send(NetworkEvent::VoteSubmitted)
            .map_err(|e| FederationError::NetworkError(format!("Failed to emit event: {}", e)))?;

        Ok(())
    }

    /// Handle proposal broadcast message
    async fn handle_proposal_broadcast(
        &mut self,
        proposal: FederatedProposal,
    ) -> Result<(), FederationError> {
        info!("Received proposal broadcast: {}", proposal.proposal_id);

        // Store the proposal
        // In a real implementation, we would have access to the storage backend
        // For now, just add it to the in-memory cache

        // Emit an event to notify listeners
        self.event_sender
            .try_send(NetworkEvent::ProposalReceived)
            .map_err(|e| FederationError::NetworkError(format!("Failed to emit event: {}", e)))?;

        Ok(())
    }

    /// Handle vote submission message
    async fn handle_vote_submission(&mut self, vote: FederatedVote) -> Result<(), FederationError> {
        info!("Received vote from {}", vote.voter);

        // Store the vote
        // In a real implementation, we would have access to the storage backend
        // For now, just log that we received it

        // Emit an event to notify listeners
        self.event_sender
            .try_send(NetworkEvent::VoteReceived)
            .map_err(|e| FederationError::NetworkError(format!("Failed to emit event: {}", e)))?;

        Ok(())
    }

    /// Connect to bootstrap nodes with retry logic
    async fn connect_to_bootstrap_nodes(&mut self) {
        for addr in &self.config.bootstrap_nodes {
            debug!("Dialing bootstrap node: {}", addr);
            if let Err(e) = self.dial_with_retry(addr.clone()).await {
                warn!("Failed to dial bootstrap node {}: {}", addr, e);
                
                // Schedule retry via the retry manager
                let addr_str = addr.to_string();
                let mut retry_manager = self.retry_manager.lock().await;
                if let Some(delay) = retry_manager.record_failure("dial", &addr_str) {
                    let addr_clone = addr.clone();
                    let event_sender = self.event_sender.clone();
                    
                    tokio::spawn(async move {
                        sleep(delay).await;
                        let retry_event = NetworkEvent::RetryOperation {
                            operation: "dial".to_string(),
                            target: addr_str,
                        };
                        if let Err(e) = event_sender.send(retry_event).await {
                            error!("Failed to send retry event: {}", e);
                        }
                    });
                }
            }
        }
    }
    
    /// Dial a peer with the current attempt
    async fn dial_with_retry(&mut self, addr: Multiaddr) -> Result<(), FederationError> {
        match self.swarm.dial(addr.clone()) {
            Ok(_) => {
                // Reset retry counter on success
                let mut retry_manager = self.retry_manager.lock().await;
                retry_manager.record_success("dial", &addr.to_string());
                Ok(())
            }
            Err(e) => {
                Err(FederationError::NetworkError(format!(
                    "Failed to dial {}: {}",
                    addr, e
                )))
            }
        }
    }

    /// Get node start time (for uptime calculations)
    pub fn get_start_time(&self) -> Option<u64> {
        self.start_time
    }
    
    /// Get node role
    pub fn get_role(&self) -> Option<&str> {
        self.role.as_deref()
    }
    
    /// Set node role
    pub fn set_role(&mut self, role: String) {
        self.role = Some(role);
    }

    /// Handle internal network events (including retries)
    async fn handle_network_event(&mut self, event: NetworkEvent) -> Result<(), FederationError> {
        match event {
            NetworkEvent::RetryOperation { operation, target } => {
                debug!("Retrying operation '{}' for target '{}'", operation, target);
                
                // Clear backoff status
                {
                    let mut retry_manager = self.retry_manager.lock().await;
                    // We'll set in_backoff to false to allow the operation to proceed
                    if let Some(entry) = retry_manager.retries.get_mut(&format!("{}:{}", operation, target)) {
                        entry.in_backoff = false;
                    }
                }
                
                // Handle different operation types
                match operation.as_str() {
                    "dial" => {
                        // Try to parse the target as a multiaddress
                        match target.parse::<Multiaddr>() {
                            Ok(addr) => {
                                if let Err(e) = self.dial_with_retry(addr.clone()).await {
                                    // If it fails again, retry manager will handle it
                                    let mut retry_manager = self.retry_manager.lock().await;
                                    if let Some(delay) = retry_manager.record_failure(&operation, &target) {
                                        debug!("Dial failed again, will retry in {:?}", delay);
                                    } else {
                                        warn!("Giving up on dialing {} after too many attempts", addr);
                                    }
                                }
                            }
                            Err(e) => {
                                error!("Failed to parse address {}: {}", target, e);
                            }
                        }
                    }
                    "send_message" => {
                        // Format is "peer_id:message_id"
                        if let Some((peer_id_str, message_id)) = target.split_once(':') {
                            if let Ok(peer_id) = peer_id_str.parse::<PeerId>() {
                                // Retry sending the message
                                if let Err(e) = self.retry_send_message(peer_id, message_id).await {
                                    let mut retry_manager = self.retry_manager.lock().await;
                                    if let Some(delay) = retry_manager.record_failure(&operation, &target) {
                                        debug!("Send message failed again, will retry in {:?}", delay);
                                    } else {
                                        warn!("Giving up on sending message {} to {} after too many attempts", 
                                            message_id, peer_id);
                                    }
                                }
                            }
                        }
                    }
                    // Handle other operation types as needed
                    _ => {
                        warn!("Unknown retry operation type: {}", operation);
                    }
                }
            }
            // Handle other network events as needed
            _ => {}
        }
        
        Ok(())
    }
    
    /// Retry sending a message identified by its ID
    async fn retry_send_message(&mut self, peer_id: PeerId, message_id: &str) -> Result<(), FederationError> {
        // In a real implementation, you would look up the message from storage
        // and attempt to resend it
        debug!("Retrying sending message {} to {}", message_id, peer_id);
        
        // This is a stub - would need actual message storage/retrieval
        Ok(())
    }

    /// Get all connected peers with additional information
    pub async fn connected_peers(&self) -> Vec<PeerInfo> {
        // This is a simplified implementation - you would want to enhance this
        // with more peer metadata from your actual implementation
        let mut peers = Vec::new();
        
        for peer_id in self.swarm.connected_peers() {
            let mut info = PeerInfo {
                peer_id: peer_id.to_string(),
                addresses: None,
                role: None,
                ping_ms: None,
                supported_protocols: None,
                last_seen: None,
            };
            
            // Add more details if available
            
            peers.push(info);
        }
        
        peers
    }
    
    /// Get all listen addresses
    pub async fn listen_addresses(&self) -> Vec<String> {
        self.swarm.listeners()
            .map(|addr| addr.to_string())
            .collect()
    }
    
    /// Send a custom message to a peer (with retry)
    pub async fn send_custom_message(
        &self,
        peer: &str,
        message: serde_json::Value,
    ) -> Result<(), FederationError> {
        // Determine if peer is an address or peer ID
        let target_peer = if peer.starts_with("/ip4/") || peer.starts_with("/ip6/") {
            // It's a multiaddress - need to resolve to peer ID
            // This is simplified - you'd want a proper lookup mechanism
            return Err(FederationError::NetworkError(
                "Sending by multiaddress not implemented yet".to_string()
            ));
        } else {
            // Assume it's a peer ID
            peer.parse::<PeerId>()
                .map_err(|_| FederationError::NetworkError(format!("Invalid peer ID: {}", peer)))?
        };
        
        // Create a unique message ID
        let message_id = format!("msg-{}", uuid::Uuid::new_v4());
        
        // In a real implementation, you'd store the message for retry purposes
        // and handle actual sending via libp2p
        
        // This is a stub implementation
        debug!("Would send message {} to {}: {}", message_id, target_peer, message);
        
        Ok(())
    }
}

/// Information about a peer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    /// Peer ID as string
    pub peer_id: String,
    /// Known addresses for this peer
    pub addresses: Option<Vec<String>>,
    /// Role of the peer if known
    pub role: Option<String>,
    /// Ping latency in milliseconds
    pub ping_ms: Option<u64>,
    /// Protocols supported by this peer
    pub supported_protocols: Option<Vec<String>>,
    /// Last time this peer was seen (UNIX timestamp)
    pub last_seen: Option<u64>,
}

/// Create a new Swarm with the provided identity
fn create_swarm(
    local_key: identity::Keypair,
    behaviour: IcnBehaviour,
) -> Result<Swarm<IcnBehaviour>, FederationError> {
    // Create a TCP transport
    let transport = {
        let tcp = tcp::tokio::Transport::new(tcp::Config::default().nodelay(true));
        let transport_upgrade = upgrade::Version::V1;

        // Create the noise keys
        let noise_config = noise::Config::new(&local_key)
            .map_err(|e| FederationError::NetworkError(format!("Noise config error: {:?}", e)))?;

        tcp.upgrade(transport_upgrade)
            .authenticate(noise_config)
            .multiplex(yamux::Config::default())
            .timeout(Duration::from_secs(20))
            .boxed()
    };

    // Create a Swarm to manage peers and events
    let config = libp2p::swarm::Config::with_tokio_executor()
        .with_idle_connection_timeout(Duration::from_secs(60));

    Ok(Swarm::new(
        transport,
        behaviour,
        local_key.public().to_peer_id(),
        config,
    ))
}
