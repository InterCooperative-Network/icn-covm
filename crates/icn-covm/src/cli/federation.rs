use crate::federation::messages::{
    FederatedProposal, FederatedVote, ProposalScope, ProposalStatus, VotingModel,
};
use crate::federation::storage::{FederationStorage, FEDERATION_NAMESPACE, VOTES_NAMESPACE};
use crate::federation::{NetworkNode, NodeConfig};
use crate::governance::proposal::{Proposal, ProposalStatus as LocalProposalStatus};
use crate::governance::proposal_lifecycle::VoteChoice;
use crate::storage::auth::AuthContext;
use crate::storage::traits::{Storage, StorageExtensions};
use crate::vm::VM;

use clap::{Arg, ArgAction, ArgMatches, Command};
use libp2p::Multiaddr;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt::Debug;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Path where federated proposals are stored
const FEDERATION_PROPOSALS_PATH: &str = "federation/proposals";
/// Path where remote votes are stored
const FEDERATION_VOTES_PATH: &str = "votes";
/// Path where sync metadata is stored
const FEDERATION_SYNC_PATH: &str = "federation/sync";

/// Metadata about a federated proposal's sync status
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FederationSyncMetadata {
    /// ID of the proposal
    proposal_id: String,
    /// Time when the proposal was last synced
    last_synced: u64,
    /// Source node that provided the proposal
    source_node: String,
    /// Number of comments synced
    comment_count: u32,
    /// Number of votes synced
    vote_count: u32,
}

/// Create the federation command and its subcommands
pub fn federation_command() -> Command {
    Command::new("federation")
        .about("Federation commands for sharing and voting on proposals across nodes")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("share-proposal")
                .about("Share a proposal with another node in the federation")
                .arg(
                    Arg::new("id")
                        .long("id")
                        .value_name("PROPOSAL_ID")
                        .help("ID of the proposal to share")
                        .required(true),
                )
                .arg(
                    Arg::new("to")
                        .long("to")
                        .value_name("NODE_ADDRESS")
                        .help("Address of the node to share with (multiaddr)")
                        .required(true),
                )
                .arg(
                    Arg::new("scope")
                        .long("scope")
                        .value_name("SCOPE")
                        .help("Voting scope: single-coop, multi-coop, or global")
                        .default_value("single-coop"),
                )
                .arg(
                    Arg::new("coops")
                        .long("coops")
                        .value_name("COOPERATIVES")
                        .help("Comma-separated list of cooperative IDs that can vote (for multi-coop scope)"),
                )
                .arg(
                    Arg::new("model")
                        .long("model")
                        .value_name("MODEL")
                        .help("Voting model: one-member-one-vote or one-coop-one-vote")
                        .default_value("one-member-one-vote"),
                )
                .arg(
                    Arg::new("expires-in")
                        .long("expires-in")
                        .value_name("SECONDS")
                        .help("Time in seconds until the proposal expires")
                        .value_parser(clap::value_parser!(u64)),
                ),
        )
        .subcommand(
            Command::new("receive-proposal")
                .about("Receive a proposal from another federation node")
                .arg(
                    Arg::new("source")
                        .long("source")
                        .value_name("NODE_ADDRESS")
                        .help("Address of the node sharing the proposal"),
                )
                .arg(
                    Arg::new("file")
                        .long("file")
                        .value_name("FILE")
                        .help("File containing the proposal JSON")
                        .required(true),
                ),
        )
        .subcommand(
            Command::new("vote")
                .about("Vote on a remote proposal")
                .arg(
                    Arg::new("remote")
                        .long("remote")
                        .value_name("PROPOSAL_ID")
                        .help("ID of the remote proposal to vote on")
                        .required(true),
                )
                .arg(
                    Arg::new("vote")
                        .long("vote")
                        .value_name("CHOICE")
                        .help("Vote choice: yes, no, or abstain")
                        .required(true),
                )
                .arg(
                    Arg::new("node")
                        .long("node")
                        .value_name("NODE_ADDRESS")
                        .help("Address of the node hosting the proposal")
                        .required(true),
                ),
        )
        .subcommand(
            Command::new("sync")
                .about("Sync proposal data with a remote node")
                .arg(
                    Arg::new("id")
                        .long("id")
                        .value_name("PROPOSAL_ID")
                        .help("ID of the proposal to sync")
                        .required(true),
                )
                .arg(
                    Arg::new("from")
                        .long("from")
                        .value_name("NODE_ADDRESS")
                        .help("Address of the node to sync from")
                        .required(true),
                )
                .arg(
                    Arg::new("force")
                        .long("force")
                        .help("Force sync even if the local version is newer")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("list")
                .about("List federated proposals")
                .arg(
                    Arg::new("status")
                        .long("status")
                        .value_name("STATUS")
                        .help("Filter by status: open, closed, executed, rejected, expired"),
                ),
        )
        .subcommand(
            Command::new("status")
                .about("Show the status of the federation node")
        )
        .subcommand(
            Command::new("peers")
                .about("List connected peers in the federation network")
                .arg(
                    Arg::new("verbose")
                        .long("verbose")
                        .short('v')
                        .help("Show more detailed peer information")
                        .action(ArgAction::SetTrue),
                )
        )
        .subcommand(
            Command::new("send-msg")
                .about("Send a message to a specific peer")
                .arg(
                    Arg::new("peer")
                        .help("The peer ID or multiaddress to send the message to")
                        .required(true)
                )
                .arg(
                    Arg::new("json")
                        .help("The JSON message to send")
                        .required(true)
                )
        )
}

/// Handle federation commands
pub async fn handle_federation_command<S>(
    vm: &mut VM<S>,
    matches: &ArgMatches,
    auth_context: &AuthContext,
) -> Result<(), Box<dyn Error>>
where
    S: Storage + StorageExtensions + Send + Sync + Clone + Debug + 'static,
{
    match matches.subcommand() {
        Some(("share-proposal", sub_matches)) => {
            let proposal_id = sub_matches
                .get_one::<String>("id")
                .ok_or_else(|| "Missing required argument: id")?;
            let node_address = sub_matches
                .get_one::<String>("to")
                .ok_or_else(|| "Missing required argument: to")?;
            let scope_str = sub_matches
                .get_one::<String>("scope")
                .ok_or_else(|| "Missing required argument: scope")?;
            let coops = sub_matches.get_one::<String>("coops").map(|s| {
                s.split(',')
                    .map(|c| c.trim().to_string())
                    .collect::<Vec<_>>()
            });
            let model_str = sub_matches
                .get_one::<String>("model")
                .ok_or_else(|| "Missing required argument: model")?;
            let expires_in = sub_matches.get_one::<u64>("expires-in").copied();

            // Parse the multiaddress
            let target_addr = node_address
                .parse::<Multiaddr>()
                .map_err(|e| format!("Invalid multiaddress: {}", e))?;

            // Parse scope
            let scope = match scope_str.as_str() {
                "single-coop" => {
                    // Use the current user's coop ID if available
                    let coop_id = auth_context
                        .memberships
                        .first()
                        .ok_or_else(|| "No cooperative membership found for the user")?
                        .namespace
                        .clone();
                    ProposalScope::SingleCoop(coop_id)
                }
                "multi-coop" => {
                    let coop_list = coops.ok_or_else(|| "For multi-coop scope, --coops must be provided with a comma-separated list of cooperative IDs")?;
                    if coop_list.is_empty() {
                        return Err(
                            "At least one cooperative ID must be provided for multi-coop scope"
                                .into(),
                        );
                    }
                    ProposalScope::MultiCoop(coop_list)
                }
                "global" => ProposalScope::GlobalFederation,
                _ => return Err(format!("Invalid scope: {}", scope_str).into()),
            };

            // Parse voting model
            let voting_model = match model_str.as_str() {
                "one-member-one-vote" => VotingModel::OneMemberOneVote,
                "one-coop-one-vote" => VotingModel::OneCoopOneVote,
                _ => return Err(format!("Invalid voting model: {}", model_str).into()),
            };

            share_proposal(
                vm,
                proposal_id,
                &target_addr,
                scope,
                voting_model,
                expires_in,
                auth_context,
            )
            .await
        }
        Some(("receive-proposal", sub_matches)) => {
            let file_path = sub_matches
                .get_one::<String>("file")
                .ok_or_else(|| "Missing required argument: file")?;
            let source_node = sub_matches
                .get_one::<String>("source")
                .map(|s| s.to_string());

            receive_proposal(vm, file_path, source_node, auth_context).await
        }
        Some(("vote", sub_matches)) => {
            let proposal_id = sub_matches
                .get_one::<String>("remote")
                .ok_or_else(|| "Missing required argument: remote")?;
            let vote_str = sub_matches
                .get_one::<String>("vote")
                .ok_or_else(|| "Missing required argument: vote")?;
            let node_address = sub_matches
                .get_one::<String>("node")
                .ok_or_else(|| "Missing required argument: node")?;

            // Parse the vote choice
            let vote_choice = match vote_str.to_lowercase().as_str() {
                "yes" => VoteChoice::Yes,
                "no" => VoteChoice::No,
                "abstain" => VoteChoice::Abstain,
                _ => {
                    return Err(format!(
                        "Invalid vote choice: {}. Must be 'yes', 'no', or 'abstain'",
                        vote_str
                    )
                    .into())
                }
            };

            // Parse the multiaddress
            let target_addr = node_address
                .parse::<Multiaddr>()
                .map_err(|e| format!("Invalid multiaddress: {}", e))?;

            submit_remote_vote(vm, proposal_id, vote_choice, &target_addr, auth_context).await
        }
        Some(("sync", sub_matches)) => {
            let proposal_id = sub_matches
                .get_one::<String>("id")
                .ok_or_else(|| "Missing required argument: id")?;
            let node_address = sub_matches
                .get_one::<String>("from")
                .ok_or_else(|| "Missing required argument: from")?;
            let force = sub_matches.get_flag("force");

            // Parse the multiaddress
            let source_addr = node_address
                .parse::<Multiaddr>()
                .map_err(|e| format!("Invalid multiaddress: {}", e))?;

            sync_proposal(vm, proposal_id, &source_addr, force, auth_context).await
        }
        Some(("list", sub_matches)) => {
            let status_filter = sub_matches
                .get_one::<String>("status")
                .map(|s| s.to_string());
            list_federated_proposals(vm, status_filter, auth_context)
        }
        Some(("status", _)) => {
            display_federation_status(vm).await?;
        }
        Some(("peers", sub_matches)) => {
            let verbose = sub_matches.get_flag("verbose");
            list_federation_peers(vm, verbose).await?;
        }
        Some(("send-msg", sub_matches)) => {
            let peer = sub_matches
                .get_one::<String>("peer")
                .ok_or_else(|| "Missing required argument: peer")?;
            let json_msg = sub_matches
                .get_one::<String>("json")
                .ok_or_else(|| "Missing required argument: json")?;
            send_federation_message(vm, peer, json_msg, auth_context).await?;
        }
        _ => Err("Unknown federation subcommand".into()),
    }
}

/// Convert a local proposal to a federated proposal
fn local_to_federated_proposal(
    local_proposal: &Proposal,
    scope: ProposalScope,
    voting_model: VotingModel,
    expires_in: Option<u64>,
) -> FederatedProposal {
    let proposal_id = local_proposal.id.clone();
    let namespace = "governance".to_string();

    // Use yes/no options for now - could be extended based on proposal type
    let options = vec!["Yes".to_string(), "No".to_string()];

    let creator = local_proposal.creator.clone();

    // Convert created_at DateTime to Unix timestamp
    let created_at = local_proposal.created_at.timestamp();

    let mut federated_proposal = FederatedProposal {
        proposal_id,
        namespace,
        options,
        creator,
        created_at,
        scope,
        voting_model,
        expires_at: None,
        status: match local_proposal.status {
            LocalProposalStatus::Draft => ProposalStatus::Open,
            LocalProposalStatus::Deliberation => ProposalStatus::Open,
            LocalProposalStatus::Active => ProposalStatus::Open,
            LocalProposalStatus::Voting => ProposalStatus::Open,
            LocalProposalStatus::Approved => ProposalStatus::Closed,
            LocalProposalStatus::Executed => ProposalStatus::Executed,
            LocalProposalStatus::Rejected => ProposalStatus::Rejected,
            LocalProposalStatus::Expired => ProposalStatus::Expired,
        },
    };

    // Add expiration if provided
    if let Some(seconds) = expires_in {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_else(|_| 0);
        federated_proposal.expires_at = Some(now + seconds as i64);
    }

    federated_proposal
}

/// Convert a vote choice to ranked choices
fn vote_choice_to_ranked_choices(choice: &VoteChoice) -> Vec<f64> {
    match choice {
        VoteChoice::Yes => vec![1.0, 0.0],     // Yes first preference
        VoteChoice::No => vec![0.0, 1.0],      // No first preference
        VoteChoice::Abstain => vec![0.0, 0.0], // Abstain (equally weighted)
    }
}

/// Load a local proposal by ID
async fn load_local_proposal<S>(vm: &VM<S>, proposal_id: &str) -> Result<Proposal, Box<dyn Error>>
where
    S: Storage + StorageExtensions + Send + Sync + Clone + Debug + 'static,
{
    let storage = vm
        .get_storage_backend()
        .ok_or_else(|| "Storage backend not available")?;

    let proposal_key = format!("governance/proposals/{}", proposal_id);

    let proposal_data = storage
        .get(None, "proposals", &proposal_key)
        .map_err(|e| format!("Failed to read proposal: {}", e))?;

    let proposal: Proposal = serde_json::from_slice(&proposal_data)
        .map_err(|e| format!("Failed to parse proposal data: {}", e))?;

    Ok(proposal)
}

/// Share a proposal with another node in the federation
async fn share_proposal<S>(
    vm: &mut VM<S>,
    proposal_id: &str,
    target_addr: &Multiaddr,
    scope: ProposalScope,
    voting_model: VotingModel,
    expires_in: Option<u64>,
    auth_context: &AuthContext,
) -> Result<(), Box<dyn Error>>
where
    S: Storage + StorageExtensions + Send + Sync + Clone + Debug + 'static,
{
    // Load the local proposal
    let local_proposal = load_local_proposal(vm, proposal_id).await?;

    // Convert to federated proposal
    let federated_proposal =
        local_to_federated_proposal(&local_proposal, scope, voting_model, expires_in);

    // Configure the federation node
    let node_config = NodeConfig {
        port: Some(0), // Use any available port
        bootstrap_nodes: vec![target_addr.clone()],
        name: Some(format!("proposal-sharer-{}", Uuid::new_v4())),
        capabilities: vec!["proposal-sharing".to_string()],
        protocol_version: "1.0.0".to_string(),
    };

    // Create and start the network node
    let mut node = NetworkNode::new(node_config)
        .await
        .map_err(|e| format!("Failed to create network node: {}", e))?;

    // Broadcast the proposal
    println!("Sharing proposal {} with node {}", proposal_id, target_addr);
    node.broadcast_proposal(federated_proposal.clone())
        .await
        .map_err(|e| format!("Failed to broadcast proposal: {}", e))?;

    // Create a fork for storage mutations
    let mut forked = vm.fork().map_err(|e| format!("Failed to fork VM: {}", e))?;

    // Get storage backend from the forked VM
    let storage = forked
        .get_storage_backend_mut()
        .ok_or_else(|| "Storage backend not available in forked VM")?;

    // Create a FederationStorage instance
    let federation_storage = FederationStorage::new();

    // Save the proposal using the federation storage's methods
    federation_storage
        .save_proposal_with_auth(storage, Some(auth_context), federated_proposal)
        .map_err(|e| format!("Failed to store federated proposal: {}", e))?;

    // Create sync metadata
    let sync_metadata = FederationSyncMetadata {
        proposal_id: proposal_id.to_string(),
        last_synced: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        source_node: "self".to_string(),
        comment_count: 0,
        vote_count: 0,
    };

    // For now just log the sync metadata information since we have trouble storing it
    println!("✅ Successfully shared proposal {} with node", proposal_id);
    println!(
        "  Sync metadata created but not stored: {:?}",
        sync_metadata
    );

    // Commit the changes from the fork
    vm.commit_fork_transaction()
        .map_err(|e| format!("Failed to commit fork transaction: {}", e))?;

    // Clean up
    node.stop().await;

    Ok(())
}

/// Receive a proposal from another federation node
async fn receive_proposal<S>(
    vm: &mut VM<S>,
    file_path: &str,
    source_node: Option<String>,
    auth_context: &AuthContext,
) -> Result<(), Box<dyn Error>>
where
    S: Storage + StorageExtensions + Send + Sync + Clone + Debug + 'static,
{
    // Read the proposal file
    let proposal_json = std::fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read proposal file: {}", e))?;

    // Parse the proposal
    let federated_proposal: FederatedProposal = serde_json::from_str(&proposal_json)
        .map_err(|e| format!("Failed to parse proposal JSON: {}", e))?;

    let proposal_id = federated_proposal.proposal_id.clone();

    // Create a fork for storage mutations
    let mut forked = vm.fork().map_err(|e| format!("Failed to fork VM: {}", e))?;

    // Get storage backend from the forked VM
    let storage = forked
        .get_storage_backend_mut()
        .ok_or_else(|| "Storage backend not available in forked VM")?;

    // Create a FederationStorage instance
    let federation_storage = FederationStorage::new();

    // Save the proposal using the federation storage's methods
    federation_storage
        .save_proposal_with_auth(storage, Some(auth_context), federated_proposal)
        .map_err(|e| format!("Failed to store federated proposal: {}", e))?;

    // Create sync metadata
    let sync_metadata = FederationSyncMetadata {
        proposal_id: proposal_id.clone(),
        last_synced: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        source_node: source_node.unwrap_or_else(|| "unknown".to_string()),
        comment_count: 0,
        vote_count: 0,
    };

    // For now just log the sync metadata information since we have trouble storing it
    println!(
        "✅ Successfully received and stored federated proposal {}",
        proposal_id
    );
    println!(
        "  Sync metadata created but not stored: {:?}",
        sync_metadata
    );

    // Commit the changes from the fork
    vm.commit_fork_transaction()
        .map_err(|e| format!("Failed to commit fork transaction: {}", e))?;

    Ok(())
}

/// Submit a vote on a remote federated proposal
async fn submit_remote_vote<S>(
    vm: &mut VM<S>,
    proposal_id: &str,
    vote_choice: VoteChoice,
    target_addr: &Multiaddr,
    auth_context: &AuthContext,
) -> Result<(), Box<dyn Error>>
where
    S: Storage + StorageExtensions + Send + Sync + Clone + Debug + 'static,
{
    // Load the federated proposal if it exists locally
    let storage = vm
        .get_storage_backend()
        .ok_or_else(|| "Storage backend not available")?;

    // Get the proposal using a FederationStorage instance
    let federation_storage = FederationStorage::new();
    let federated_proposal = federation_storage
        .get_proposal(&*storage, proposal_id)
        .map_err(|e| {
            println!(
                "Proposal not found locally. Please sync it first with 'federation sync' command."
            );
            format!("Proposal not found locally: {}", e)
        })?;

    // Check if the proposal is still open for voting
    if federated_proposal.status != ProposalStatus::Open {
        return Err(format!(
            "Proposal is not open for voting. Current status: {:?}",
            federated_proposal.status
        )
        .into());
    }

    // Create a vote object
    let voter_id = auth_context.current_identity_did.clone();
    let ranked_choices = vote_choice_to_ranked_choices(&vote_choice);

    // Create a message to sign
    let message = format!(
        "Vote for proposal {} by {} with choices {:?}",
        proposal_id, voter_id, ranked_choices
    );

    // We'd normally sign this with the identity's private key
    // For now, we'll use a placeholder signature
    let signature = format!("placeholder_signature_for_{}", voter_id);

    let federated_vote = FederatedVote {
        proposal_id: proposal_id.to_string(),
        voter: voter_id.clone(),
        ranked_choices,
        message: message.clone(),
        signature,
    };

    // Configure the federation node
    let node_config = NodeConfig {
        port: Some(0), // Use any available port
        bootstrap_nodes: vec![target_addr.clone()],
        name: Some(format!("vote-submitter-{}", Uuid::new_v4())),
        capabilities: vec!["vote-submission".to_string()],
        protocol_version: "1.0.0".to_string(),
    };

    // Create and start the network node
    let mut node = NetworkNode::new(node_config)
        .await
        .map_err(|e| format!("Failed to create network node: {}", e))?;

    // Submit the vote
    println!(
        "Submitting vote for proposal {} to node {}",
        proposal_id, target_addr
    );
    node.submit_vote(federated_vote.clone())
        .await
        .map_err(|e| format!("Failed to submit vote: {}", e))?;

    // Create a fork for storage mutations
    let mut forked = vm.fork().map_err(|e| format!("Failed to fork VM: {}", e))?;

    // Get storage backend from the forked VM
    let storage = forked
        .get_storage_backend_mut()
        .ok_or_else(|| "Storage backend not available in forked VM")?;

    // Set up the vote locally
    let vote_key = format!("{}/{}/{}", FEDERATION_VOTES_PATH, proposal_id, voter_id);

    // Store the raw vote choice for compatibility with local votes
    let vote_data = serde_json::to_vec(&vote_choice)
        .map_err(|e| format!("Failed to serialize vote choice: {}", e))?;

    // Store the vote directly
    storage
        .set(Some(auth_context), VOTES_NAMESPACE, &vote_key, vote_data)
        .map_err(|e| format!("Failed to store vote: {}", e))?;

    // Commit the changes from the fork
    vm.commit_fork_transaction()
        .map_err(|e| format!("Failed to commit fork transaction: {}", e))?;

    println!(
        "✅ Successfully submitted vote on proposal {} and stored locally",
        proposal_id
    );

    // Clean up
    node.stop().await;

    Ok(())
}

/// Sync a proposal with a remote node
async fn sync_proposal<S>(
    vm: &mut VM<S>,
    proposal_id: &str,
    source_addr: &Multiaddr,
    force: bool,
    auth_context: &AuthContext,
) -> Result<(), Box<dyn Error>>
where
    S: Storage + StorageExtensions + Send + Sync + Clone + Debug + 'static,
{
    // This would normally be implemented with direct federation communication
    // For now, we'll simulate the sync with local operations

    println!("Syncing proposal {} from node {}", proposal_id, source_addr);

    // Check if we have the proposal locally
    let storage = vm
        .get_storage_backend()
        .ok_or_else(|| "Storage backend not available")?;
    let federation_storage = FederationStorage::new();

    let local_exists = match federation_storage.get_proposal(&*storage, proposal_id) {
        Ok(_) => true,
        Err(_) => false,
    };

    // In a real implementation, we would:
    // 1. Query the remote node for the proposal data
    // 2. Query for comments and votes
    // 3. Compare timestamps and merge data

    println!("Simulating sync of proposal, comments, and votes");
    println!("Local copy exists: {}", local_exists);
    println!("Force mode: {}", force);

    if local_exists && !force {
        println!("Proposal already exists locally. Use --force to override.");
    } else {
        println!("Proposal would be synced from the remote node.");
        println!("Comments and votes would be merged with local data if any.");
    }

    // Create sync metadata
    let sync_metadata = FederationSyncMetadata {
        proposal_id: proposal_id.to_string(),
        last_synced: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        source_node: source_addr.to_string(),
        comment_count: 0, // In a real implementation, this would be the actual count
        vote_count: 0,    // In a real implementation, this would be the actual count
    };

    // For now, skip saving the sync metadata
    println!(
        "✅ Successfully simulated sync for proposal {}",
        proposal_id
    );
    println!(
        "  Sync metadata created but not stored: {:?}",
        sync_metadata
    );

    Ok(())
}

/// List all federated proposals
fn list_federated_proposals<S>(
    vm: &mut VM<S>,
    status_filter: Option<String>,
    auth_context: &AuthContext,
) -> Result<(), Box<dyn Error>>
where
    S: Storage + StorageExtensions + Send + Sync + Clone + Debug + 'static,
{
    let storage = vm
        .get_storage_backend()
        .ok_or_else(|| "Storage backend not available")?;

    // Get all proposals in the federation namespace
    let proposals_path = FEDERATION_PROPOSALS_PATH;

    let proposal_keys = match storage.list_keys(
        Some(auth_context),
        FEDERATION_NAMESPACE,
        Some(proposals_path),
    ) {
        Ok(keys) => keys,
        Err(e) => {
            println!("No federated proposals found: {}", e);
            return Ok(());
        }
    };

    if proposal_keys.is_empty() {
        println!("No federated proposals found");
        return Ok(());
    }

    println!("=== Federated Proposals ===");

    let mut found_any = false;

    for key in proposal_keys {
        let proposal_id = key.split('/').last().unwrap_or("unknown");

        // Read the proposal
        let full_key = FederationStorage::make_proposal_key(proposal_id);
        match storage.get(Some(auth_context), FEDERATION_NAMESPACE, &full_key) {
            Ok(data) => {
                if let Ok(proposal) = {
                    let data_vec = data.to_vec(); // Convert to Vec<u8> to avoid [u8] sizing issues
                    serde_json::from_slice::<FederatedProposal>(&data_vec)
                } {
                    // Filter by status if requested
                    if let Some(ref status) = status_filter {
                        let status_matches = match status.to_lowercase().as_str() {
                            "open" => proposal.status == ProposalStatus::Open,
                            "closed" => proposal.status == ProposalStatus::Closed,
                            "executed" => proposal.status == ProposalStatus::Executed,
                            "rejected" => proposal.status == ProposalStatus::Rejected,
                            "expired" => proposal.status == ProposalStatus::Expired,
                            _ => true, // Invalid status filter, show all
                        };

                        if !status_matches {
                            continue;
                        }
                    }

                    found_any = true;

                    // Get sync metadata if available
                    let sync_key = FederationStorage::make_sync_key(proposal_id);
                    let sync_info =
                        match storage.get(Some(auth_context), FEDERATION_NAMESPACE, &sync_key) {
                            Ok(data) => {
                                if let Ok(metadata) =
                                    serde_json::from_slice::<FederationSyncMetadata>(&data)
                                {
                                    Some(metadata)
                                } else {
                                    None
                                }
                            }
                            Err(_) => None,
                        };

                    // Calculate vote counts
                    let votes_key = FederationStorage::make_votes_key(proposal_id);
                    let vote_count = match storage.list_keys(
                        Some(auth_context),
                        VOTES_NAMESPACE,
                        Some(&votes_key),
                    ) {
                        Ok(keys) => keys.len(),
                        Err(_) => 0,
                    };

                    // Display proposal info
                    println!("\nID:        {}", proposal_id);
                    println!("Creator:   {}", proposal.creator);
                    println!("Status:    {:?}", proposal.status);
                    println!(
                        "Created:   {}",
                        chrono::DateTime::from_timestamp(proposal.created_at, 0)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_else(|| proposal.created_at.to_string())
                    );

                    if let Some(expires) = proposal.expires_at {
                        println!(
                            "Expires:   {}",
                            chrono::DateTime::from_timestamp(expires, 0)
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_else(|| expires.to_string())
                        );
                    }

                    println!("Scope:     {:?}", proposal.scope);
                    println!("Model:     {:?}", proposal.voting_model);
                    println!("Votes:     {}", vote_count);

                    if let Some(metadata) = sync_info {
                        println!(
                            "Last Sync: {} from {}",
                            chrono::DateTime::from_timestamp(metadata.last_synced as i64, 0)
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_else(|| metadata.last_synced.to_string()),
                            metadata.source_node
                        );
                    }
                }
            }
            Err(_) => continue,
        }
    }

    if !found_any {
        if status_filter.is_some() {
            println!(
                "No proposals found matching status filter: {:?}",
                status_filter
            );
        } else {
            println!("No proposals found");
        }
    }

    Ok(())
}

/// Display the status of the federation node
async fn display_federation_status<S>(vm: &VM<S>) -> Result<(), Box<dyn Error>>
where
    S: Storage + StorageExtensions + Send + Sync + Clone + Debug + 'static,
{
    // Get the network node from the VM
    let node = vm.get_network_node().ok_or_else(|| {
        "Federation node is not initialized. Run with --federation flag to enable federation."
            .to_string()
    })?;

    // Get the node's status
    let local_peer_id = node.local_peer_id();
    let connected_peers = node.connected_peers().await;
    let listen_addrs = node.listen_addresses().await;
    
    println!("Federation Node Status:");
    println!("----------------------");
    println!("Local Peer ID: {}", local_peer_id);
    println!("Connected Peers: {}", connected_peers.len());
    println!("Listening on:");
    for addr in listen_addrs {
        println!("  - {}", addr);
    }
    
    // Get node uptime if available
    if let Some(start_time) = node.get_start_time() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let uptime_seconds = now.saturating_sub(start_time);
        let days = uptime_seconds / 86400;
        let hours = (uptime_seconds % 86400) / 3600;
        let minutes = (uptime_seconds % 3600) / 60;
        let seconds = uptime_seconds % 60;
        
        println!("Uptime: {}d {}h {}m {}s", days, hours, minutes, seconds);
    }
    
    // Display node role if available
    if let Some(role) = node.get_role() {
        println!("Node Role: {}", role);
    }
    
    // Show federation storage status
    println!("Federation Storage:");
    println!("  Proposals: {}", vm.count_federated_proposals()?);
    println!("  Votes: {}", vm.count_federated_votes()?);
    
    Ok(())
}

/// List connected peers in the federation network
async fn list_federation_peers<S>(vm: &VM<S>, verbose: bool) -> Result<(), Box<dyn Error>>
where
    S: Storage + StorageExtensions + Send + Sync + Clone + Debug + 'static,
{
    // Get the network node from the VM
    let node = vm.get_network_node().ok_or_else(|| {
        "Federation node is not initialized. Run with --federation flag to enable federation."
            .to_string()
    })?;

    // Get connected peers information
    let peers = node.connected_peers().await;
    
    if peers.is_empty() {
        println!("No connected peers found.");
        return Ok(());
    }
    
    println!("Connected Peers ({})", peers.len());
    println!("----------------");
    
    for (i, peer_info) in peers.iter().enumerate() {
        println!("{}. Peer ID: {}", i + 1, peer_info.peer_id);
        
        if verbose {
            if let Some(addresses) = &peer_info.addresses {
                println!("   Addresses:");
                for addr in addresses {
                    println!("     - {}", addr);
                }
            }
            
            if let Some(role) = &peer_info.role {
                println!("   Role: {}", role);
            }
            
            if let Some(ping) = peer_info.ping_ms {
                println!("   Ping: {}ms", ping);
            }
            
            if let Some(protocols) = &peer_info.supported_protocols {
                println!("   Supported Protocols:");
                for protocol in protocols {
                    println!("     - {}", protocol);
                }
            }
            
            if let Some(last_seen) = peer_info.last_seen {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let seconds_ago = now.saturating_sub(last_seen);
                
                if seconds_ago < 60 {
                    println!("   Last Seen: {} seconds ago", seconds_ago);
                } else if seconds_ago < 3600 {
                    println!("   Last Seen: {} minutes ago", seconds_ago / 60);
                } else {
                    println!("   Last Seen: {} hours ago", seconds_ago / 3600);
                }
            }
            
            println!(); // Add blank line between peers for readability
        }
    }
    
    Ok(())
}

/// Send a message to a specific peer
async fn send_federation_message<S>(
    vm: &VM<S>,
    peer: &str, 
    json_msg: &str,
    auth_context: &AuthContext,
) -> Result<(), Box<dyn Error>>
where
    S: Storage + StorageExtensions + Send + Sync + Clone + Debug + 'static,
{
    // Get the network node from the VM
    let node = vm.get_network_node().ok_or_else(|| {
        "Federation node is not initialized. Run with --federation flag to enable federation."
            .to_string()
    })?;
    
    // Parse the peer address or ID
    let peer_target = if peer.starts_with("/ip4/") || peer.starts_with("/ip6/") {
        // It's a multiaddress
        let addr: Multiaddr = peer.parse().map_err(|_| format!("Invalid multiaddress: {}", peer))?;
        peer.to_string()
    } else {
        // Assume it's a peer ID
        format!("{}", peer)
    };
    
    // Parse the JSON message
    let json_value: serde_json::Value = serde_json::from_str(json_msg)
        .map_err(|e| format!("Invalid JSON message: {}", e))?;
    
    // Send the message
    println!("Sending message to peer {}...", peer_target);
    
    let result = node.send_custom_message(&peer_target, json_value).await;
    
    match result {
        Ok(_) => {
            println!("Message sent successfully.");
            
            // Log the message in local storage for audit purposes
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            
            let log_entry = serde_json::json!({
                "type": "outgoing_message",
                "timestamp": timestamp,
                "peer": peer,
                "message": json_value,
                "sender": auth_context.identity.id_string(),
            });
            
            vm.federation_log_message(log_entry, auth_context)?;
            
            Ok(())
        },
        Err(e) => {
            Err(format!("Failed to send message: {}", e).into())
        }
    }
}
