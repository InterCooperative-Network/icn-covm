//! Cooperative Value Module for the Internet Computer Network
//!
//! The `icn-covm` crate provides a virtual machine for executing
//! cooperative governance operations on the Internet Computer Network.
//!
//! Key features:
//! - Stack-based VM with rich operation types
//! - Serializable operations for storage and transmission
//! - DSL for writing governance programs
//! - Compiler for transforming DSL into VM operations
//! - Runtime for executing operations
//! - Storage abstractions for persistence
//!
//! This crate is intended to be used in contexts where multiple parties
//! need to cooperatively manage resources using programmatic governance.

// Re-export only the key types that form the public API
pub use crate::storage::auth::AuthContext;
#[cfg(feature = "typed-values")]
pub use crate::typed::TypedValue;

// Modules that remain private to the crate
mod api;
mod bytecode;
mod cli;
mod compiler;
mod events;
#[cfg(feature = "federation")]
mod federation;
mod governance;
mod identity;
mod storage;
mod typed;
mod vm;

// Internal imports
use crate::compiler::{parse_dsl, parse_dsl_with_stdlib, CompilerError};
use icn_ledger::DagNode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fmt::Debug;
use std::fs;
use std::path::Path;
use std::time::Instant;
use thiserror::Error;

/// Represents changes to the DAG ledger as a result of executing a DSL program
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagDelta {
    /// New nodes added to the DAG
    pub added_nodes: Vec<DagNode>,
    
    /// Namespace used for the operations
    pub namespace: String,
    
    /// Number of operations executed
    pub operations_executed: usize,
    
    /// Whether the operation was successful
    pub success: bool,
    
    /// Any error message if the operation failed
    pub error: Option<String>,
}

impl DagDelta {
    /// Create a new empty DagDelta
    pub fn new(namespace: String) -> Self {
        Self {
            added_nodes: Vec::new(),
            namespace,
            operations_executed: 0,
            success: true,
            error: None,
        }
    }
    
    /// Create a new DagDelta representing a failed operation
    pub fn error(namespace: String, error: impl Into<String>) -> Self {
        Self {
            added_nodes: Vec::new(),
            namespace,
            operations_executed: 0,
            success: false,
            error: Some(error.into()),
        }
    }
}

/// Defines storage backend types for use in VMOptions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum StorageBackendType {
    /// In-memory storage that doesn't persist between VM instances
    Memory,
    /// File-based storage that persists data to disk
    File(String),
}

impl Default for StorageBackendType {
    fn default() -> Self {
        StorageBackendType::Memory
    }
}

/// Options for configuring the VM execution environment
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct VMOptions {
    /// Storage backend configuration
    pub storage_backend: StorageBackendType,
    
    /// Optional identity context for authentication
    pub identity_context: Option<AuthContext>,
    
    /// Whether federation is enabled
    #[cfg(feature = "federation")]
    pub enable_federation: bool,
    
    /// Whether to compile and execute as bytecode
    pub use_bytecode: bool,
    
    /// Whether to run in debug mode with additional logging
    pub debug_mode: bool,
    
    /// Whether to run in simulation mode (no persistent storage changes)
    pub simulation_mode: bool,
    
    /// Whether to trace execution steps
    pub trace_execution: bool,
    
    /// Whether to generate explanations of operations
    pub explain_operations: bool,
    
    /// Whether to enable verbose storage tracing
    pub verbose_storage_trace: bool,
    
    /// Namespace for storage operations
    pub namespace: String,
    
    /// Parameters to pass to the VM
    pub parameters: HashMap<String, String>,
    
    /// Whether to include the standard library
    pub use_stdlib: bool,
}

/// Errors that can occur during VM execution
#[derive(Debug, Error, Serialize, Deserialize)]
enum VMEngineError {
    /// VM execution error
    #[error("VM error: {0}")]
    VM(String),
    
    /// Compiler error
    #[error("Compiler error: {0}")]
    Compiler(String),
    
    /// IO error
    #[error("IO error: {0}")]
    IO(String),
    
    /// JSON parsing error
    #[error("JSON error: {0}")]
    Json(String),
    
    /// Federation error
    #[error("Federation error: {0}")]
    Federation(String),
    
    /// Other error
    #[error("{0}")]
    Other(String),
}

impl From<vm::errors::VMError> for VMEngineError {
    fn from(err: vm::errors::VMError) -> Self {
        VMEngineError::VM(err.to_string())
    }
}

impl From<CompilerError> for VMEngineError {
    fn from(err: CompilerError) -> Self {
        VMEngineError::Compiler(err.to_string())
    }
}

impl From<std::io::Error> for VMEngineError {
    fn from(err: std::io::Error) -> Self {
        VMEngineError::IO(err.to_string())
    }
}

impl From<serde_json::Error> for VMEngineError {
    fn from(err: serde_json::Error) -> Self {
        VMEngineError::Json(err.to_string())
    }
}

impl From<&str> for VMEngineError {
    fn from(s: &str) -> Self {
        VMEngineError::Other(s.to_string())
    }
}

impl From<String> for VMEngineError {
    fn from(s: String) -> Self {
        VMEngineError::Other(s)
    }
}

impl From<Box<dyn Error>> for VMEngineError {
    fn from(e: Box<dyn Error>) -> Self {
        VMEngineError::Other(e.to_string())
    }
}

/// Extension trait to add creation from options
trait VMExt<S: storage::traits::StorageBackend + Send + Sync + Clone + Debug + 'static> {
    fn create_with_options(options: &VMOptions) -> Result<vm::VM<S>, VMEngineError>;
}

impl<S: storage::traits::StorageBackend + Send + Sync + Clone + Debug + 'static> VMExt<S> for vm::VM<S> {
    fn create_with_options(options: &VMOptions) -> Result<vm::VM<S>, VMEngineError> {
        // Configure VM with options
        let mut vm = match options.storage_backend {
            StorageBackendType::Memory => {
                let storage = storage::implementations::in_memory::InMemoryStorage::new();
                vm::VM::with_storage_backend(storage)
            },
            StorageBackendType::File(ref path) => {
                let storage = storage::implementations::file_storage::FileStorage::new(path)?;
                vm::VM::with_storage_backend(storage)
            }
        };
        
        // Apply additional VM configuration from options
        if let Some(auth) = &options.identity_context {
            vm.set_auth_context(auth.clone());
        }
        
        vm.set_namespace(&options.namespace);
        
        if options.debug_mode {
            println!("VM initialized with namespace: {}", options.namespace);
            if options.identity_context.is_some() {
                println!("Auth context set: {:?}", options.identity_context);
            }
        }
        
        // Configure execution behavior
        if options.simulation_mode {
            if options.debug_mode {
                println!("Running in simulation mode - no persistent changes will be made");
            }
            vm.begin_transaction();
        }
        
        if options.trace_execution {
            if options.debug_mode {
                println!("Execution tracing enabled");
            }
            vm.set_tracing(true);
        }
        
        if options.explain_operations {
            vm.set_explain_operations(true);
        }
        
        if options.verbose_storage_trace {
            vm.set_storage_tracing(true);
        }
        
        Ok(vm)
    }
}

/// Execute DSL source code with the given authorization context
/// 
/// This function parses and executes a DSL program, producing a DagDelta that
/// represents the changes to the DAG ledger as a result of execution.
/// 
/// # Arguments
/// 
/// * `src` - Source code in the DSL format
/// * `ctx` - Authorization context for the execution
/// 
/// # Returns
/// 
/// A Result containing the DagDelta with any changes to the DAG ledger,
/// or an error if parsing or execution failed.
pub fn execute_dsl(src: &str, ctx: &AuthContext) -> Result<DagDelta, VMEngineError> {
    // Create default options with the provided auth context
    let mut options = VMOptions::default();
    options.identity_context = Some(ctx.clone());
    options.namespace = "default".to_string();
    
    // Parse the DSL code
    let ops = if options.use_stdlib {
        parse_dsl_with_stdlib(src)?
    } else {
        let (ops, _) = parse_dsl(src)?;
        ops
    };
    
    // Create a VM with the specified options
    let mut vm = vm::VM::create_with_options(&options)?;
    
    // Start tracking execution time
    let start_time = Instant::now();
    
    // Execute the operations based on the use_bytecode option
    let operations_executed = if options.use_bytecode {
        // Use bytecode compilation and execution
        let mut compiler = bytecode::BytecodeCompiler::new();
        let program = compiler.compile(&ops);
        
        let mut interpreter = bytecode::BytecodeInterpreter::new(vm, program);
        match interpreter.execute() {
            Ok(()) => {
                // Extract the VM from the interpreter
                vm = interpreter.get_vm().clone();
                ops.len() // Use original ops count for consistency
            }
            Err(err) => {
                // Extract the VM from the interpreter
                vm = interpreter.get_vm().clone();
                
                // Return the error
                return Err(err.into());
            }
        }
    } else {
        // Use direct AST execution
        match vm.execute(&ops) {
            Ok(count) => count,
            Err(err) => return Err(err.into()),
        }
    };
    
    // Calculate execution time
    let elapsed = start_time.elapsed();
    let elapsed_ms = elapsed.as_millis();
    
    // Prepare result
    let mut delta = DagDelta::new(options.namespace);
    delta.operations_executed = operations_executed;
    
    // Include any DAG nodes created during execution
    if let Some(dag) = vm.dag {
        // Get the nodes created during this execution
        let all_nodes = dag.nodes();
        delta.added_nodes = all_nodes.clone();
    }
    
    Ok(delta)
}

/// Create a default auth context with optional identity name and role
pub fn create_default_auth_context(id_name: Option<&str>, role: Option<&str>) -> AuthContext {
    let id_name = id_name.unwrap_or("anonymous");
    let role = role.unwrap_or("user");
    
    // Create a new auth context with the specified role
    let mut auth = AuthContext::new(id_name);
    
    // Add the role
    auth.add_role("global", role);
    auth.add_role_to_identity(id_name, "global", role);
    
    auth
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_simple_program_execution() {
        let source = r#"
            # Simple test program
            push 10.0
            push 5.0
            add
            emit "hi"
        "#;
        
        let auth_ctx = create_default_auth_context(None, None);
        let result = execute_dsl(source, &auth_ctx).expect("Execution should succeed");
        assert!(result.success);
        assert!(result.operations_executed > 0);
    }
    
    #[test]
    fn test_emit_returns_ok() {
        let source = r#"emit "hi""#;
        
        let auth_ctx = create_default_auth_context(None, None);
        let result = execute_dsl(source, &auth_ctx);
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_emit_creates_dag_vertex() {
        let source = r#"emit "Hello""#;
        
        let auth_ctx = create_default_auth_context(None, None);
        let result = execute_dsl(source, &auth_ctx).expect("Execution should succeed");
        
        // Verify the result
        assert!(result.success);
        assert!(result.operations_executed > 0);
        
        // Verify a DAG node was created
        assert!(!result.added_nodes.is_empty(), "Expected at least one DAG node");
        
        // Examine the first node
        let node = &result.added_nodes[0];
        assert!(!node.id.is_empty(), "Node ID should not be empty");
    }
    
    #[test]
    fn test_bytecode_execution() {
        let source = r#"
            push 1.0
            push 2.0
            add
            emit "bytecode test"
        "#;
        
        let auth_ctx = create_default_auth_context(None, None);
        
        // Create options for bytecode execution
        let mut options = VMOptions::default();
        options.identity_context = Some(auth_ctx.clone());
        options.use_bytecode = true;
        
        // Parse and create a VM
        let (ops, _) = parse_dsl(source).expect("Parsing should succeed");
        let mut vm = vm::VM::create_with_options(&options).expect("VM creation should succeed");
        
        // Compile to bytecode
        let mut compiler = bytecode::BytecodeCompiler::new();
        let program = compiler.compile(&ops);
        
        // Execute via bytecode interpreter
        let mut interpreter = bytecode::BytecodeInterpreter::new(vm, program);
        let result = interpreter.execute();
        assert!(result.is_ok(), "Bytecode execution failed: {:?}", result);
        
        // Test the full execute_dsl with bytecode
        let mut options = VMOptions::default();
        options.identity_context = Some(auth_ctx.clone());
        options.use_bytecode = true;
        
        let mut vm = vm::VM::create_with_options(&options).expect("VM creation should succeed");
        let (ops, _) = parse_dsl(source).expect("Parsing should succeed");
        
        // Create a bytecode program
        let program = compiler.compile(&ops);
        
        // Create and run the interpreter
        let mut interpreter = bytecode::BytecodeInterpreter::new(vm, program);
        let result = interpreter.execute();
        assert!(result.is_ok(), "Bytecode execution failed: {:?}", result);
    }
}
