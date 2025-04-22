//! Federation support for VM
//! 
//! This module provides integration between the VM and federation network node
//! to support federated operations like proposal synchronization and DAG state sync.

use std::fmt::Debug;
use std::error::Error;
use std::sync::{Arc, Mutex};
use log::{debug, info, warn, error};
use serde_json::Value;

use crate::federation::NetworkNode;
use crate::federation::messages::{FederatedProposal, FederatedVote, ProposalScope, VotingModel};
use crate::federation::storage::{FederationStorage, FEDERATION_NAMESPACE, VOTES_NAMESPACE};
use crate::storage::auth::AuthContext;
use crate::storage::traits::{Storage, StorageExtensions};
use crate::vm::VM;

/// Error type for federation operations
#[derive(Debug, thiserror::Error)]
pub enum FederationVMError {
    #[error("Federation is not enabled")]
    NotEnabled,
    
    #[error("Network error: {0}")]
    NetworkError(String),
    
    #[error("Storage error: {0}")]
    StorageError(String),
    
    #[error("Message error: {0}")]
    MessageError(String),
}

/// Federation VM extension traits
pub trait FederationVM {
    /// Get the network node if federation is enabled
    fn get_network_node(&self) -> Option<Arc<Mutex<NetworkNode>>>;
    
    /// Count federated proposals
    fn count_federated_proposals(&self) -> Result<usize, Box<dyn Error>>;
    
    /// Count federated votes
    fn count_federated_votes(&self) -> Result<usize, Box<dyn Error>>;
    
    /// Log federation message
    fn federation_log_message(&self, message: Value, auth_context: &AuthContext) -> Result<(), Box<dyn Error>>;
}

impl<S> FederationVM for VM<S> 
where 
    S: Storage + StorageExtensions + Send + Sync + Clone + Debug + 'static
{
    fn get_network_node(&self) -> Option<Arc<Mutex<NetworkNode>>> {
        self.federation_node.clone()
    }
    
    fn count_federated_proposals(&self) -> Result<usize, Box<dyn Error>> {
        if let Some(node) = &self.federation_node {
            let storage = node.lock().unwrap().federation_storage();
            let count = storage.count_proposals()?;
            Ok(count)
        } else {
            Err(FederationVMError::NotEnabled.into())
        }
    }
    
    fn count_federated_votes(&self) -> Result<usize, Box<dyn Error>> {
        if let Some(node) = &self.federation_node {
            let storage = node.lock().unwrap().federation_storage();
            let count = storage.count_votes()?;
            Ok(count)
        } else {
            Err(FederationVMError::NotEnabled.into())
        }
    }
    
    fn federation_log_message(&self, message: Value, auth_context: &AuthContext) -> Result<(), Box<dyn Error>> {
        if let Some(node) = &self.federation_node {
            // Create an entry in the federation log
            let timestamp = chrono::Utc::now().timestamp();
            let log_key = format!("fed_log:{}", timestamp);
            
            // Add metadata
            let mut log_entry = message;
            if let Value::Object(ref mut map) = log_entry {
                map.insert("timestamp".to_string(), timestamp.into());
                map.insert("sender_did".to_string(), auth_context.current_identity_did.clone().into());
            }
            
            // Store in federation storage
            let storage = node.lock().unwrap().federation_storage();
            storage.store_raw("logs", &log_key, &log_entry.to_string())?;
            
            debug!("Federation log message stored: {}", log_key);
            Ok(())
        } else {
            Err(FederationVMError::NotEnabled.into())
        }
    }
}

/// Extension for federation proposal executor
pub struct ProposalExecutor<S>
where
    S: Storage + StorageExtensions + Send + Sync + Clone + Debug + 'static
{
    vm: VM<S>,
    auth_context: AuthContext,
    dsl_code: Option<String>,
    bytecode: Option<Vec<u8>>,
    parent_hash: Option<String>,
    scope: Option<ProposalScope>,
    voting_model: Option<VotingModel>,
    expires_in: Option<u64>,
}

impl<S> ProposalExecutor<S>
where
    S: Storage + StorageExtensions + Send + Sync + Clone + Debug + 'static
{
    /// Create a new proposal executor with the given VM
    pub fn new(vm: VM<S>, auth_context: AuthContext) -> Self {
        Self {
            vm,
            auth_context,
            dsl_code: None,
            bytecode: None,
            parent_hash: None,
            scope: None, 
            voting_model: None,
            expires_in: None,
        }
    }
    
    /// Set the parent hash for DAG context
    pub fn with_parent_hash(mut self, parent_hash: String) -> Self {
        self.parent_hash = Some(parent_hash);
        self
    }
    
    /// Set the scope for the proposal
    pub fn with_scope(mut self, scope: ProposalScope) -> Self {
        self.scope = Some(scope);
        self
    }
    
    /// Set the voting model
    pub fn with_voting_model(mut self, model: VotingModel) -> Self {
        self.voting_model = Some(model);
        self
    }
    
    /// Set the expiration time in seconds from now
    pub fn with_expiry(mut self, seconds: u64) -> Self {
        self.expires_in = Some(seconds);
        self
    }
    
    /// Load DSL code from a string
    pub fn load_dsl(mut self, dsl_code: String) -> Self {
        self.dsl_code = Some(dsl_code);
        self
    }
    
    /// Load DSL code from a file
    pub fn load_dsl_file(mut self, file_path: &str) -> Result<Self, Box<dyn Error>> {
        let dsl_code = std::fs::read_to_string(file_path)?;
        self.dsl_code = Some(dsl_code);
        Ok(self)
    }
    
    /// Load bytecode
    pub fn load_bytecode(mut self, bytecode: Vec<u8>) -> Self {
        self.bytecode = Some(bytecode);
        self
    }
    
    /// Execute the proposal
    pub async fn execute(mut self) -> Result<VM<S>, Box<dyn Error>> {
        // Check if federation is enabled
        if self.vm.get_network_node().is_none() {
            return Err("Federation is not enabled on this VM".into());
        }
        
        // Use default voting model if not specified
        let voting_model = self.voting_model.unwrap_or(VotingModel::Simple);
        
        // Use default scope if not specified
        let scope = self.scope.unwrap_or(ProposalScope::Local);
        
        // Compile DSL if provided
        if let Some(dsl_code) = &self.dsl_code {
            // Parse and compile the DSL code
            // This would normally use the DSL compiler
            info!("Compiling DSL code for proposal execution");
            
            // For now, we'll just log this and would integrate with the actual compiler
            // In a real implementation, this would compile DSL into bytecode or ops
            // self.bytecode = Some(compile_dsl(dsl_code)?);
        }
        
        // Execute bytecode if provided
        if let Some(_bytecode) = &self.bytecode {
            // Execute the bytecode
            info!("Executing proposal bytecode");
            
            // In a real implementation, this would execute the bytecode
            // vm.execute_bytecode(bytecode)?;
        }
        
        // Log the execution in the federation log
        let log_entry = serde_json::json!({
            "type": "proposal_execution",
            "scope": format!("{:?}", scope),
            "voting_model": format!("{:?}", voting_model),
            "parent_hash": self.parent_hash,
            "executed_at": chrono::Utc::now().timestamp(),
        });
        
        self.vm.federation_log_message(log_entry, &self.auth_context)?;
        
        Ok(self.vm)
    }
} 