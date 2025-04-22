use std::fmt::Debug;
use std::error::Error;
use serde_json::Value;
use std::sync::Arc;
use std::sync::Mutex;

use crate::federation::{NetworkNode, NodeConfig};
use crate::federation::storage::{FederationStorage, FEDERATION_NAMESPACE, VOTES_NAMESPACE};
use crate::storage::auth::AuthContext;
use crate::storage::traits::{Storage, StorageExtensions};
use crate::vm::VM;

/// Extension trait for VM with federation capabilities
pub trait VMFederationExtension<S>
where
    S: Storage + StorageExtensions + Send + Sync + Clone + Debug + 'static,
{
    /// Get the federation network node if available
    fn get_network_node(&self) -> Option<&Arc<Mutex<NetworkNode>>>;
    
    /// Set the federation network node
    fn set_network_node(&mut self, node: NetworkNode);
    
    /// Count the number of federated proposals
    fn count_federated_proposals(&self) -> Result<usize, Box<dyn Error>>;
    
    /// Count the number of federated votes
    fn count_federated_votes(&self) -> Result<usize, Box<dyn Error>>;
    
    /// Log a federation message for audit purposes
    fn federation_log_message(&self, message: Value, auth_context: &AuthContext) -> Result<(), Box<dyn Error>>;
}

impl<S> VMFederationExtension<S> for VM<S>
where
    S: Storage + StorageExtensions + Send + Sync + Clone + Debug + 'static,
{
    fn get_network_node(&self) -> Option<&Arc<Mutex<NetworkNode>>> {
        // Access the network node field that needs to be added to VM
        self.network_node.as_ref()
    }
    
    fn set_network_node(&mut self, node: NetworkNode) {
        // Set the network node in the VM
        self.network_node = Some(Arc::new(Mutex::new(node)));
    }
    
    fn count_federated_proposals(&self) -> Result<usize, Box<dyn Error>> {
        let storage = self.get_storage_backend()
            .ok_or_else(|| "Storage backend not available".to_string())?;
        
        let federation_storage = FederationStorage::new();
        let count = federation_storage.count_proposals(&*storage)?;
        
        Ok(count)
    }
    
    fn count_federated_votes(&self) -> Result<usize, Box<dyn Error>> {
        let storage = self.get_storage_backend()
            .ok_or_else(|| "Storage backend not available".to_string())?;
            
        let keys = storage.list_keys(None, VOTES_NAMESPACE, None)
            .map_err(|e| format!("Failed to list vote keys: {}", e))?;
            
        Ok(keys.len())
    }
    
    fn federation_log_message(&self, message: Value, auth_context: &AuthContext) -> Result<(), Box<dyn Error>> {
        let storage = self.get_storage_backend()
            .ok_or_else(|| "Storage backend not available".to_string())?;
            
        // Create a key for the log message
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
            
        let msg_id = uuid::Uuid::new_v4().to_string();
        let log_key = format!("federation/logs/{}-{}", timestamp, msg_id);
        
        // Serialize the message
        let message_data = serde_json::to_vec(&message)
            .map_err(|e| format!("Failed to serialize log message: {}", e))?;
            
        // Store the message in the federation namespace
        match storage.set(Some(auth_context), FEDERATION_NAMESPACE, &log_key, message_data) {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("Failed to store log message: {}", e).into())
        }
    }
} 