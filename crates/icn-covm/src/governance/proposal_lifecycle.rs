use crate::compiler::parse_dsl;
use crate::identity::Identity;
use crate::storage::auth::AuthContext;
use crate::storage::errors::StorageError;
use crate::storage::traits::{Storage, StorageBackend};
use crate::vm::Op;
use crate::vm::VM;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json; // Import serde_json for serialization
use std::collections::HashMap;
use std::fmt::Debug; // Import the actual Identity struct
                     // Placeholder for attachment metadata, replace with actual type later
type Attachment = String;
// Use String for IDs
type CommentId = String;
type ProposalId = String;

// Define the Vote type
pub type Vote = u64; // Just an example, replace with your actual Vote type

// Define result and preview types
pub struct ProposalExecutionPreview {
    pub side_effects: Vec<String>,
    pub success_probability: f64,
}

pub enum ProposalExecutionResult {
    Success { log: Vec<String> },
    Failure { reason: String },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ProposalState {
    Draft,
    OpenForFeedback,
    Voting,
    Executed,
    Rejected,
    Expired,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ExecutionStatus {
    Success,
    Failure(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum VoteChoice {
    Yes,
    No,
    Abstain,
}

/// Vote weighting strategy for a proposal
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum VoteWeighting {
    /// Each vote has equal weight
    Equal,
    /// Votes are weighted based on a reputation score
    Reputation,
    /// Votes are weighted based on stake/token amount
    Stake,
    /// Votes are weighted based on tenure in the organization
    Tenure,
    /// Quadratic voting - vote weight is square root of cost
    Quadratic,
    /// Delegated voting - vote weight includes delegated votes
    Delegated,
    /// Custom weighting with a specified algorithm
    Custom(String),
}

/// Quorum calculation method
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum QuorumMethod {
    /// Fixed percentage of total possible voters 
    Percentage(u8), // 0-100
    /// Fixed number of voters required
    FixedNumber(u64),
    /// No quorum requirement
    None,
}

// Implement FromStr to parse from CLI string input
use std::str::FromStr;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseVoteChoiceError;
impl std::fmt::Display for ParseVoteChoiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid vote choice. Must be 'yes', 'no', or 'abstain'.")
    }
}
impl std::error::Error for ParseVoteChoiceError {}

impl FromStr for VoteChoice {
    type Err = ParseVoteChoiceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "yes" => Ok(VoteChoice::Yes),
            "no" => Ok(VoteChoice::No),
            "abstain" => Ok(VoteChoice::Abstain),
            _ => Err(ParseVoteChoiceError),
        }
    }
}

/// Parse error for VoteWeighting
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseVoteWeightingError;
impl std::fmt::Display for ParseVoteWeightingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid vote weighting. Must be 'equal', 'reputation', 'stake', 'tenure', 'quadratic', 'delegated', or 'custom:<name>'.")
    }
}
impl std::error::Error for ParseVoteWeightingError {}

impl FromStr for VoteWeighting {
    type Err = ParseVoteWeightingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "equal" => Ok(VoteWeighting::Equal),
            "reputation" => Ok(VoteWeighting::Reputation),
            "stake" => Ok(VoteWeighting::Stake),
            "tenure" => Ok(VoteWeighting::Tenure),
            "quadratic" => Ok(VoteWeighting::Quadratic),
            "delegated" => Ok(VoteWeighting::Delegated),
            s if s.starts_with("custom:") => {
                let custom_name = s.strip_prefix("custom:").unwrap().to_string();
                Ok(VoteWeighting::Custom(custom_name))
            }
            _ => Err(ParseVoteWeightingError),
        }
    }
}

/// Parse error for QuorumMethod
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseQuorumMethodError;
impl std::fmt::Display for ParseQuorumMethodError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid quorum method. Must be 'percentage:<0-100>', 'fixed:<number>', or 'none'.")
    }
}
impl std::error::Error for ParseQuorumMethodError {}

impl FromStr for QuorumMethod {
    type Err = ParseQuorumMethodError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase() {
            s if s.starts_with("percentage:") => {
                let percentage_str = s.strip_prefix("percentage:").unwrap();
                let percentage = percentage_str.parse::<u8>().map_err(|_| ParseQuorumMethodError)?;
                if percentage > 100 {
                    return Err(ParseQuorumMethodError);
                }
                Ok(QuorumMethod::Percentage(percentage))
            }
            s if s.starts_with("fixed:") => {
                let number_str = s.strip_prefix("fixed:").unwrap();
                let number = number_str.parse::<u64>().map_err(|_| ParseQuorumMethodError)?;
                Ok(QuorumMethod::FixedNumber(number))
            }
            "none" => Ok(QuorumMethod::None),
            _ => Err(ParseQuorumMethodError),
        }
    }
}

// Implement Display to serialize for storage?
// Or maybe store as string directly is better for simplicity/flexibility?
// Let's stick to storing the string for now, less migration hassle.

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProposalLifecycle {
    pub id: ProposalId,
    pub creator: Identity,
    pub created_at: DateTime<Utc>,
    pub state: ProposalState,
    pub title: String, // Added title based on CLI command
    // Enhanced quorum and threshold
    pub quorum_method: QuorumMethod,
    pub approval_threshold: u8, // Percentage (0-100) needed for approval
    pub expires_at: Option<DateTime<Utc>>, // Voting expiration
    pub discussion_duration: Option<Duration>, // For macro integration
    pub vote_weighting: VoteWeighting, // How votes are weighted
    pub required_participants: Option<u64>, // For macro integration
    pub current_version: u64,
    // Additional settings for expanded governance
    pub auto_execute: bool, // Whether to execute automatically when passed
    pub allow_early_execution: bool, // Whether to allow execution before expiry
    pub min_voting_period: Option<Duration>, // Minimum time votes must be open
    // attachments: Vec<Attachment>, // Store attachment metadata or links? Store in storage layer.
    // comments: Vec<CommentId>, // Store comment IDs? Store in storage layer.
    pub history: Vec<(DateTime<Utc>, ProposalState)>, // Track state transitions
    pub execution_status: Option<ExecutionStatus>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Comment {
    pub id: CommentId,           // Now String
    pub proposal_id: ProposalId, // Now String
    pub author: Identity,
    pub timestamp: DateTime<Utc>,
    pub content: String,
    pub reply_to: Option<CommentId>, // Now Option<String>
}

impl ProposalLifecycle {
    pub fn new(
        id: ProposalId,
        creator: Identity,
        title: String,
        quorum_method: QuorumMethod,
        approval_threshold: u8,
        vote_weighting: VoteWeighting,
        discussion_duration: Option<Duration>,
        required_participants: Option<u64>,
    ) -> Self {
        let now = Utc::now();
        ProposalLifecycle {
            id,
            creator,
            created_at: now,
            state: ProposalState::Draft,
            title,
            quorum_method,
            approval_threshold,
            expires_at: None, // Set when voting starts
            discussion_duration,
            vote_weighting,
            required_participants,
            current_version: 1,
            auto_execute: false,
            allow_early_execution: false,
            min_voting_period: None,
            history: vec![(now, ProposalState::Draft)],
            execution_status: None,
        }
    }

    // New helper for backwards compatibility
    pub fn new_backwards_compatible(
        id: ProposalId,
        creator: Identity,
        title: String,
        quorum: u64,
        threshold: u64,
        discussion_duration: Option<Duration>,
        required_participants: Option<u64>,
    ) -> Self {
        // Convert old quorum to percentage (assuming quorum was a percentage)
        let quorum_method = if quorum == 0 {
            QuorumMethod::None
        } else if quorum <= 100 {
            QuorumMethod::Percentage(quorum as u8)
        } else {
            QuorumMethod::FixedNumber(quorum)
        };

        // Convert old threshold to percentage (capped at 100)
        let approval_threshold = (threshold as u8).min(100);

        Self::new(
            id,
            creator,
            title,
            quorum_method,
            approval_threshold,
            VoteWeighting::Equal, // Default to equal weighting
            discussion_duration,
            required_participants,
        )
    }

    // Placeholder methods for state transitions - logic to be added later
    pub fn open_for_feedback(&mut self) {
        if self.state == ProposalState::Draft {
            self.state = ProposalState::OpenForFeedback;
            self.history.push((Utc::now(), self.state.clone()));
            // If discussion duration is set, calculate when it should move to voting
            if let Some(duration) = self.discussion_duration {
                // This could be used by an external scheduler to auto-transition
                let _feedback_end_time = Utc::now() + duration;
                // In a real implementation, store this for auto-transition
            }
        }
    }

    pub fn start_voting(&mut self, voting_duration: Duration) {
        // Only transition if in the OpenForFeedback state
        if self.state == ProposalState::OpenForFeedback {
            self.state = ProposalState::Voting;
            
            // Set expiration time
            self.expires_at = Some(Utc::now() + voting_duration);
            
            // Ensure minimum voting period is respected if set
            if let Some(min_period) = self.min_voting_period {
                if voting_duration < min_period {
                    // Override with minimum duration
                    self.expires_at = Some(Utc::now() + min_period);
                }
            }
            
            self.history.push((Utc::now(), self.state.clone()));
        }
    }

    pub fn execute(&mut self) {
        if self.state == ProposalState::Voting {
            // Check if early execution is allowed
            if !self.allow_early_execution {
                // Check if we're still within the voting period
                if let Some(expires_at) = self.expires_at {
                    if Utc::now() < expires_at {
                        // Cannot execute early
                        return;
                    }
                }
            }
            
            // Add logic for successful vote
            self.state = ProposalState::Executed;
            self.history.push((Utc::now(), self.state.clone()));
        }
    }

    pub fn reject(&mut self) {
        if self.state == ProposalState::Voting {
            // Add logic for failed vote
            self.state = ProposalState::Rejected;
            self.history.push((Utc::now(), self.state.clone()));
        }
    }

    pub fn expire(&mut self) {
        if self.state == ProposalState::Voting
            && self.expires_at.map_or(false, |exp| Utc::now() > exp)
        {
            self.state = ProposalState::Expired;
            self.history.push((Utc::now(), self.state.clone()));
        }
    }

    // Check if proposal has expired and needs state update
    pub fn check_expiry(&mut self) -> bool {
        if self.state == ProposalState::Voting {
            if let Some(expires_at) = self.expires_at {
                if Utc::now() > expires_at {
                    self.state = ProposalState::Expired;
                    self.history.push((Utc::now(), self.state.clone()));
                    return true;
                }
            }
        }
        false
    }

    pub fn update_version(&mut self) {
        // Logic for handling updates, potentially resetting state or requiring new votes?
        self.current_version += 1;
        // Maybe move back to Draft or OpenForFeedback? Depends on governance rules.
        self.history.push((Utc::now(), self.state.clone()));
    }

    // Calculate vote weight based on the proposal's weighting strategy
    pub fn calculate_vote_weight<S>(
        &self,
        vm: &mut VM<S>,
        voter: &Identity,
        vote_choice: &VoteChoice,
        auth_context: Option<&AuthContext>,
    ) -> Result<f64, Box<dyn std::error::Error>>
    where
        S: Storage + Send + Sync + Clone + Debug + 'static,
    {
        // Basic vote value (1.0 for yes/no, 0.0 for abstain)
        let base_value = match vote_choice {
            VoteChoice::Yes | VoteChoice::No => 1.0,
            VoteChoice::Abstain => 0.0,
        };
        
        // Apply weighting based on strategy
        match &self.vote_weighting {
            VoteWeighting::Equal => {
                // Every vote has the same weight
                Ok(base_value)
            },
            VoteWeighting::Reputation => {
                // Get reputation score from storage
                let reputation_key = format!("identity:{}/reputation", voter.id_string());
                let reputation = vm.get_f64(reputation_key.as_str(), auth_context)?
                    .unwrap_or(1.0); // Default 1.0 if not found
                
                Ok(base_value * reputation)
            },
            VoteWeighting::Stake => {
                // Get stake amount from storage
                let stake_key = format!("identity:{}/stake", voter.id_string());
                let stake = vm.get_f64(stake_key.as_str(), auth_context)?
                    .unwrap_or(1.0); // Default 1.0 if not found
                
                Ok(base_value * stake)
            },
            VoteWeighting::Tenure => {
                // Get join date from storage and calculate tenure in years
                let join_date_key = format!("identity:{}/join_date", voter.id_string());
                if let Some(join_timestamp) = vm.get_f64(join_date_key.as_str(), auth_context)? {
                    let now = Utc::now().timestamp() as f64;
                    let seconds_in_year = 31_536_000.0;
                    let years = (now - join_timestamp) / seconds_in_year;
                    // Cap at 10 years max weight
                    let weight = 1.0 + (years.min(10.0) * 0.1); // 10% per year
                    Ok(base_value * weight)
                } else {
                    // Default to 1.0 if join date not found
                    Ok(base_value)
                }
            },
            VoteWeighting::Quadratic => {
                // Get vote cost/credits from storage
                let credits_key = format!("proposal:{}/vote_credits/{}", self.id, voter.id_string());
                let credits = vm.get_f64(credits_key.as_str(), auth_context)?
                    .unwrap_or(1.0); // Default 1.0 if not found
                
                // Square root of credits for quadratic voting
                let weight = credits.sqrt();
                Ok(base_value * weight)
            },
            VoteWeighting::Delegated => {
                // Get direct weight plus delegated votes
                let direct_weight = 1.0; // Base weight
                
                // Get delegations from storage
                let delegations_key = format!("identity:{}/delegated_votes", voter.id_string());
                let delegated_count = vm.get_f64(delegations_key.as_str(), auth_context)?
                    .unwrap_or(0.0);
                
                Ok(base_value * (direct_weight + delegated_count))
            },
            VoteWeighting::Custom(algo) => {
                // Call a custom weight function stored in the VM
                let function_key = format!("weight_algo:{}", algo);
                if let Some(weight_code) = vm.get_string(function_key.as_str(), auth_context)? {
                    // In a real implementation, would execute this code through DSL
                    // For now, default to 1.0
                    Ok(base_value)
                } else {
                    // Algorithm not found, use default
                    Ok(base_value)
                }
            }
        }
    }

    // Tally votes from storage
    pub fn tally_votes<S>(
        &self,
        vm: &mut VM<S>,
        auth_context: Option<&AuthContext>,
    ) -> Result<HashMap<String, Vote>, Box<dyn std::error::Error>>
    where
        S: Storage + Send + Sync + Clone + Debug + 'static,
    {
        if self.state != ProposalState::Voting {
            return Err(format!("Proposal {} is not in Voting state", self.id).into());
        }
        let storage = vm
            .get_storage_backend()
            .ok_or_else(|| "Storage backend not available")?;
        let auth_context = vm.get_auth_context();
        let namespace = "governance";
        let prefix = format!("proposals/{}/votes/", self.id);
        let vote_keys = storage.list_keys(auth_context, namespace, Some(&prefix))?;

        let mut weighted_votes = HashMap::new();

        for key in vote_keys {
            if !key.starts_with(&prefix) || key.split('/').count() != 4 {
                eprintln!("Skipping unexpected key in votes directory: {}", key);
                continue;
            }
            match storage.get(auth_context, namespace, &key) {
                Ok(vote_bytes) => {
                    let vote_str = String::from_utf8(vote_bytes).unwrap_or_default();
                    // Parse the stored string into VoteChoice
                    match VoteChoice::from_str(&vote_str) {
                        Ok(vote_choice) => {
                            let weight = self.calculate_vote_weight(vm, &self.creator, &vote_choice, auth_context)?;
                            weighted_votes.insert(vote_choice.to_string(), weight.round() as Vote);
                        }
                        Err(_) => eprintln!(
                            "Warning: Invalid vote choice string '{}' found in storage for key {}",
                            vote_str, key
                        ),
                    }
                }
                Err(e) => {
                    eprintln!("Error reading vote key {}: {}", key, e);
                }
            }
        }

        Ok(weighted_votes)
    }
    
    // Check if quorum is reached based on quorum_method
    pub fn is_quorum_reached<S>(
        &self,
        vm: &mut VM<S>,
        auth_context: Option<&AuthContext>,
        vote_count: usize,
        total_eligible: usize,
    ) -> Result<bool, Box<dyn std::error::Error>>
    where
        S: Storage + Send + Sync + Clone + Debug + 'static,
    {
        match self.quorum_method {
            QuorumMethod::None => {
                // No quorum requirement
                Ok(true)
            },
            QuorumMethod::Percentage(percentage) => {
                if total_eligible == 0 {
                    return Ok(false); // No eligible voters
                }
                
                // Calculate participation percentage
                let participation_percent = (vote_count as f64 / total_eligible as f64) * 100.0;
                Ok(participation_percent >= percentage as f64)
            },
            QuorumMethod::FixedNumber(required_count) => {
                // Check if we have enough votes
                Ok(vote_count as u64 >= required_count)
            }
        }
    }
    
    // Check if approval threshold is met
    pub fn is_threshold_met<S>(
        &self,
        vm: &mut VM<S>,
        auth_context: Option<&AuthContext>,
        yes_weight: f64,
        total_weight: f64,
    ) -> Result<bool, Box<dyn std::error::Error>>
    where
        S: Storage + Send + Sync + Clone + Debug + 'static,
    {
        if total_weight <= 0.0 {
            return Ok(false); // No votes
        }
        
        // Calculate approval percentage
        let approval_percent = (yes_weight / total_weight) * 100.0;
        Ok(approval_percent >= self.approval_threshold as f64)
    }

    // Execute the proposal's logic attachment within the given VM context
    // Returns Ok(ExecutionStatus) on completion (success or failure)
    // Returns Err only if loading/parsing fails before execution starts
    fn execute_proposal_logic<S>(
        &self,
        vm: &mut VM<S>, // Pass original VM mutably to allow commit/rollback
        auth_context: Option<&AuthContext>,
    ) -> Result<ExecutionStatus, Box<dyn std::error::Error>>
    where
        S: Storage + Send + Sync + Clone + Debug + 'static,
    {
        println!(
            "[EXEC] Preparing sandboxed execution for proposal {}",
            self.id
        );

        // --- Create VM Fork ---
        let mut fork_vm = vm.fork()?; // fork() begins the transaction on original VM's storage
        println!("[EXEC] VM Fork created.");

        // --- Logic Loading (using fork's context) ---
        let logic_dsl = {
            let storage = fork_vm
                .get_storage_backend()
                .ok_or_else(|| "Storage backend not available")?;
            let auth_context = fork_vm.get_auth_context();
            let namespace = "governance"; // Assuming logic is always in governance namespace
            let logic_key = format!("proposals/{}/attachments/logic", self.id);
            println!(
                "[EXEC] Loading logic from {}/{} within fork...",
                namespace, logic_key
            );

            match storage.get(auth_context, namespace, &logic_key) {
                Ok(bytes) => {
                    let dsl = String::from_utf8(bytes)
                        .map_err(|e| format!("Logic attachment is not valid UTF-8: {}", e))?;
                    if dsl.trim().is_empty() {
                        println!("[EXEC] Logic attachment is empty. Skipping execution.");
                        None // Treat empty DSL as skippable
                    } else {
                        println!("[EXEC] Logic DSL loaded ({} bytes) within fork.", dsl.len());
                        Some(dsl)
                    }
                }
                Err(StorageError::NotFound { .. }) => {
                    println!(
                        "[EXEC] No logic attachment found at {}. Skipping execution.",
                        logic_key
                    );
                    None // Treat missing logic as skippable
                }
                Err(e) => return Err(format!("Failed to load logic attachment: {}", e).into()),
            }
        };

        // --- Execution (within Fork) & Transaction Handling ---
        let execution_status = if let Some(dsl) = logic_dsl {
            println!("[EXEC] Parsing logic DSL within fork...");
            let (ops, _) =
                parse_dsl(&dsl).map_err(|e| format!("Failed to parse logic DSL: {}", e))?;
            println!("[EXEC] Logic parsed into {} Ops within fork.", ops.len());

            println!("[EXEC] Executing parsed Ops within fork VM...");
            match fork_vm.execute(&ops) {
                Ok(_) => {
                    println!("[EXEC] Fork execution successful. Committing transaction on original VM...");
                    vm.commit_fork_transaction()?;
                    ExecutionStatus::Success
                }
                Err(e) => {
                    let error_message = format!("Runtime error during fork execution: {}", e);
                    eprintln!("[EXEC] {}", error_message);
                    println!(
                        "[EXEC] Rolling back transaction on original VM due to fork failure..."
                    );
                    vm.rollback_fork_transaction()?; // Rollback original VM's transaction
                    ExecutionStatus::Failure(error_message)
                }
            }
        } else {
            // No logic to execute, commit the (empty) transaction
            println!(
                "[EXEC] No logic DSL found/loaded. Committing empty transaction on original VM."
            );
            vm.commit_fork_transaction()?;
            ExecutionStatus::Success
        };

        Ok(execution_status)
    }

    // Updated state transition for execution
    pub fn transition_to_executed<S>(
        &mut self,
        vm: &mut VM<S>,
        auth_context: Option<&AuthContext>,
    ) -> Result<bool, Box<dyn std::error::Error>>
    where
        S: Storage + Send + Sync + Clone + Debug + 'static,
    {
        if self.state == ProposalState::Voting {
            let votes = self.tally_votes(vm, auth_context)?;
            let passed = self.check_passed(vm, auth_context, &votes)?;
            if passed {
                self.state = ProposalState::Executed;
                self.history.push((Utc::now(), self.state.clone()));
                println!("Proposal {} state transitioning to Executed.", self.id);

                // Attempt to execute associated logic
                let exec_result = self.execute_proposal_logic(vm, auth_context);

                // Update status based on execution result
                match exec_result {
                    Ok(_) => {
                        println!("Proposal {} execution completed successfully.", self.id);
                    }
                    Err(e) => {
                        println!("Proposal {} execution failed: {}", self.id, e);
                        // TODO: Set execution_status to Failed
                    }
                }

                Ok(true)
            } else {
                println!(
                    "Proposal {} did not meet the voting requirements to execute.",
                    self.id
                );
                Ok(false)
            }
        } else {
            println!(
                "Proposal {} not in Voting state, cannot transition to Executed.",
                self.id
            );
            Ok(false)
        }
    }

    // Updated state transition for rejection
    pub fn transition_to_rejected<S>(
        &mut self,
        vm: &mut VM<S>,
        auth_context: Option<&AuthContext>,
    ) -> Result<bool, Box<dyn std::error::Error>>
    where
        S: Storage + Send + Sync + Clone + Debug + 'static,
    {
        if self.state == ProposalState::Voting {
            let votes = self.tally_votes(vm, auth_context)?;
            let passed = self.check_passed(vm, auth_context, &votes)?;
            if !passed {
                self.state = ProposalState::Rejected;
                self.history.push((Utc::now(), self.state.clone()));
                println!("Proposal {} state transitioning to Rejected.", self.id);
                Ok(true)
            } else {
                println!(
                    "Proposal {} met the voting requirements to execute, cannot reject.",
                    self.id
                );
                Ok(false)
            }
        } else {
            println!(
                "Proposal {} not in Voting state, cannot transition to Rejected.",
                self.id
            );
            Ok(false)
        }
    }

    // Updated state transition for expiration
    pub fn transition_to_expired<S>(
        &mut self,
        vm: &mut VM<S>,
        auth_context: Option<&AuthContext>,
    ) -> Result<bool, Box<dyn std::error::Error>>
    where
        S: Storage + Send + Sync + Clone + Debug + 'static,
    {
        if self.state == ProposalState::Voting
            && self.expires_at.map_or(false, |exp| Utc::now() > exp)
        {
            let votes = self.tally_votes(vm, auth_context)?;
            let passed = self.check_passed(vm, auth_context, &votes)?;
            if passed {
                println!("Proposal {} passed but expired before execution.", self.id);
                // Leave execution_status as None or set to Failure("Expired")?
            } else {
                println!(
                    "Proposal {} did not have enough votes before expiry.",
                    self.id
                );
            }
            self.state = ProposalState::Expired;
            self.history.push((Utc::now(), self.state.clone()));
            println!("Proposal {} state transitioning to Expired.", self.id);
            Ok(true)
        } else {
            println!(
                "Proposal {} not in Voting state, cannot transition to Expired.",
                self.id
            );
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*; // Import parent module content
    use crate::identity::Identity;
    use chrono::Duration;

    // Helper to create a dummy Identity for testing
    fn test_identity(username: &str) -> Identity {
        Identity::new(username.to_string(), None, "test_member".to_string(), None)
            .expect("Failed to create test identity")
    }

    // Helper to create a basic proposal for tests
    fn create_test_proposal() -> ProposalLifecycle {
        let creator_id = test_identity("test_creator");
        ProposalLifecycle::new(
            "prop-123".to_string(),
            creator_id,
            "Test Proposal".to_string(),
            10,                      // quorum
            5,                       // threshold
            Some(Duration::days(7)), // discussion_duration
            None,                    // required_participants
        )
    }

    #[test]
    fn test_proposal_creation_state() {
        let proposal = create_test_proposal();
        assert_eq!(proposal.state, ProposalState::Draft);
        assert_eq!(proposal.current_version, 1);
        assert_eq!(proposal.history.len(), 1);
        assert_eq!(proposal.history[0].1, ProposalState::Draft);
    }

    #[test]
    fn test_open_for_feedback_transition() {
        let mut proposal = create_test_proposal();
        assert_eq!(proposal.state, ProposalState::Draft);

        proposal.open_for_feedback();

        assert_eq!(proposal.state, ProposalState::OpenForFeedback);
        assert_eq!(proposal.history.len(), 2);
        assert_eq!(proposal.history[1].1, ProposalState::OpenForFeedback);
    }

    #[test]
    fn test_start_voting_transition() {
        let mut proposal = create_test_proposal();
        proposal.open_for_feedback(); // Must be in OpenForFeedback first
        assert_eq!(proposal.state, ProposalState::OpenForFeedback);
        assert!(proposal.expires_at.is_none());

        let voting_duration = Duration::days(3);
        let expected_expiry_min = Utc::now() + voting_duration - Duration::seconds(1);
        let expected_expiry_max = Utc::now() + voting_duration + Duration::seconds(1);

        proposal.start_voting(voting_duration);

        assert_eq!(proposal.state, ProposalState::Voting);
        assert_eq!(proposal.history.len(), 3);
        assert_eq!(proposal.history[2].1, ProposalState::Voting);
        assert!(proposal.expires_at.is_some());
        let expires_at = proposal
            .expires_at
            .expect("Expiry time should be set after start_voting");
        assert!(
            expires_at > expected_expiry_min && expires_at < expected_expiry_max,
            "Expiry time not within expected range"
        );
    }

    #[test]
    fn test_invalid_transitions() {
        let mut proposal = create_test_proposal();

        // Can't start voting from Draft
        let initial_state = proposal.state.clone();
        let initial_history_len = proposal.history.len();
        proposal.start_voting(Duration::days(1));
        assert_eq!(proposal.state, initial_state); // State should not change
        assert_eq!(proposal.history.len(), initial_history_len); // History should not change
        assert!(proposal.expires_at.is_none());

        // Can't open for feedback from Voting
        proposal.open_for_feedback(); // Move to OpenForFeedback
        proposal.start_voting(Duration::days(1)); // Move to Voting
        assert_eq!(proposal.state, ProposalState::Voting);
        let state_before_invalid = proposal.state.clone();
        let history_len_before_invalid = proposal.history.len();

        proposal.open_for_feedback(); // Attempt invalid transition

        assert_eq!(proposal.state, state_before_invalid); // State should not change
        assert_eq!(proposal.history.len(), history_len_before_invalid); // History should not change
    }

    // TODO: Add tests for tally_votes and check_passed (might require mocking storage or VM)
    // TODO: Add tests for execute/reject/expire transitions (likely better in integration tests)
}
