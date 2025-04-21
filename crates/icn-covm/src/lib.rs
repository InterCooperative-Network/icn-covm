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

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;
use thiserror::Error;

pub mod api;
pub mod bytecode;
pub mod cli;
pub mod compiler;
pub mod events;
pub mod federation;
pub mod governance;
pub mod identity;
pub mod storage;
pub mod typed;
pub mod vm;

// Re-export key types for convenience
pub use bytecode::{BytecodeCompiler, BytecodeInterpreter};
pub use compiler::{parse_dsl, parse_dsl_with_stdlib, CompilerError, LifecycleConfig};
pub use identity::Identity;
pub use storage::auth::AuthContext;
pub use storage::implementations::file_storage::FileStorage;
pub use storage::implementations::in_memory::InMemoryStorage;
pub use storage::traits::StorageBackend;
pub use typed::TypedValue;
pub use vm::types::Op;
pub use vm::VM;
pub use vm::VMError;

/// Defines storage backend types for use in VMOptions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StorageBackendType {
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
pub struct VMOptions {
    /// Storage backend configuration
    pub storage_backend: StorageBackendType,
    
    /// Optional identity context for authentication
    pub identity_context: Option<AuthContext>,
    
    /// Whether federation is enabled
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

/// Result returned after executing a program
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Whether execution was successful
    pub success: bool,
    
    /// Final stack values if execution was successful
    pub stack: Option<Vec<TypedValue>>,
    
    /// Memory values if execution was successful
    pub memory: Option<HashMap<String, TypedValue>>,
    
    /// Execution time in milliseconds
    pub execution_time_ms: u128,
    
    /// Error if execution failed
    pub error: Option<String>,
    
    /// Number of operations executed
    pub operations_executed: usize,
}

/// Errors that can occur during VM execution
#[derive(Debug, Error, Serialize, Deserialize)]
pub enum VMEngineError {
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

impl From<VMError> for VMEngineError {
    fn from(err: VMError) -> Self {
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

/// Execute a program from a file path
pub fn execute_program_from_path<P: AsRef<Path>>(
    path: P,
    options: VMOptions,
) -> Result<ExecutionResult, VMEngineError> {
    let path = path.as_ref();

    // Check if file exists
    if !path.exists() {
        return Err(VMEngineError::from(format!("Program file not found: {}", path.display())));
    }

    // Parse operations based on file extension
    let ops = if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        match extension.to_lowercase().as_str() {
            "dsl" => {
                if options.debug_mode {
                    println!("Parsing DSL program from {}", path.display());
                }
                let program_source = fs::read_to_string(path)?;

                // Check if we should include the standard library
                if options.debug_mode && options.use_stdlib {
                    println!("Including standard library functions");
                }

                if options.use_stdlib {
                    parse_dsl_with_stdlib(&program_source)?
                } else {
                    let (ops, _lifecycle) = parse_dsl(&program_source)?;
                    ops
                }
            }
            "json" => {
                if options.debug_mode {
                    println!("Parsing JSON program from {}", path.display());
                }
                let program_json = fs::read_to_string(path)?;
                serde_json::from_str(&program_json)?
            }
            _ => return Err(VMEngineError::from(format!("Unsupported file extension: {}", extension))),
        }
    } else {
        return Err(VMEngineError::from("File has no extension"));
    };

    if options.debug_mode {
        println!("Program loaded with {} operations", ops.len());
    }

    execute_program(&ops, options)
}

/// Execute a program from a list of operations
pub fn execute_program(
    ops: &[Op],
    options: VMOptions,
) -> Result<ExecutionResult, VMEngineError> {
    // Print execution mode information if in debug mode
    if options.debug_mode || options.simulation_mode || options.trace_execution || options.explain_operations {
        if options.simulation_mode {
            println!("Running in SIMULATION mode (no persistent storage changes)");
        }
        if options.trace_execution {
            println!("Tracing enabled (will show operations and stack state)");
        }
        if options.explain_operations {
            println!("Explanation enabled (will describe each operation)");
        }
    }

    // Setup auth context (use provided or create default)
    let auth_context = options.identity_context.unwrap_or_else(|| {
        // Create a default identity with admin role
        create_default_auth_context(None, Some("admin"))
    });

    // Get or create storage backend
    let storage = match &options.storage_backend {
        StorageBackendType::Memory => InMemoryStorage::new(),
        StorageBackendType::File(path) => {
            // For simplicity, we're only supporting InMemoryStorage for now
            // In a real implementation, this would use FileStorage with the specified path
            InMemoryStorage::new()
        }
    };

    let start = Instant::now();
    let mut operations_executed = 0;

    let result = if options.use_bytecode {
        // Bytecode execution
        let mut compiler = BytecodeCompiler::new();
        let program = compiler.compile(ops);

        if options.debug_mode {
            println!("Compiled bytecode program:\n{}", program.dump());
        }

        // Create bytecode interpreter with proper auth context and storage
        let mut vm: VM<InMemoryStorage> = VM::new()
            .set_simulation_mode(options.simulation_mode)
            .set_tracing(options.trace_execution)
            .set_explanation(options.explain_operations)
            .set_verbose_storage_trace(options.verbose_storage_trace);

        vm.set_auth_context(auth_context);
        vm.set_namespace(options.namespace.as_str());
        vm.set_storage_backend(storage);

        let mut interpreter = BytecodeInterpreter::new(vm, program);

        // Set parameters
        interpreter
            .get_vm_mut()
            .set_parameters(options.parameters.clone())?;

        // Execute
        let execute_result = interpreter.execute();
        
        if let Ok(count) = execute_result {
            operations_executed = count;
            
            let stack = interpreter.get_vm().get_stack().clone();
            let memory_map = interpreter.get_vm().get_memory_map();
            
            Ok((stack, memory_map))
        } else {
            Err(execute_result.err().unwrap())
        }
    } else {
        // AST execution
        let mut vm: VM<InMemoryStorage> = VM::new();

        // Set the execution options
        vm.set_simulation_mode(options.simulation_mode);
        vm.set_tracing(options.trace_execution);
        vm.set_explanation(options.explain_operations);
        vm.set_verbose_storage_trace(options.verbose_storage_trace);

        vm.set_auth_context(auth_context);
        vm.set_namespace(options.namespace.as_str());
        vm.set_storage_backend(storage);

        // Set parameters
        vm.set_parameters(options.parameters)?;

        if options.debug_mode {
            println!("Executing program in AST interpreter mode...");
            println!("-----------------------------------");
        }

        let execute_result = vm.execute(ops);
        
        if let Ok(count) = execute_result {
            operations_executed = count;
            
            let stack = vm.get_stack().clone();
            let memory_map = vm.get_memory_map();
            
            Ok((stack, memory_map))
        } else {
            Err(execute_result.err().unwrap())
        }
    };

    let duration = start.elapsed();
    
    if options.debug_mode {
        println!("Execution completed in {:?}", duration);
    }

    match result {
        Ok((stack, memory)) => {
            if options.debug_mode {
                println!("Final stack: {:?}", stack);
                println!("Final memory:");
                for (key, value) in &memory {
                    println!("  {}: {}", key, value);
                }
                if memory.is_empty() {
                    println!("  (empty)");
                }
            }
            
            Ok(ExecutionResult {
                success: true,
                stack: Some(stack),
                memory: Some(memory),
                execution_time_ms: duration.as_millis(),
                error: None,
                operations_executed,
            })
        },
        Err(err) => {
            if options.debug_mode {
                println!("Execution failed: {}", err);
            }
            
            Ok(ExecutionResult {
                success: false,
                stack: None,
                memory: None,
                execution_time_ms: duration.as_millis(),
                error: Some(err.to_string()),
                operations_executed,
            })
        }
    }
}

/// Create a default authentication context
pub fn create_default_auth_context(id_name: Option<&str>, role: Option<&str>) -> AuthContext {
    let id_name = id_name.unwrap_or("default-user");
    let role = role.unwrap_or("user");
    
    // Create a new Ed25519 identity
    let identity = Identity::new_ed25519();
    
    // Create and return the auth context
    AuthContext::new(identity, role.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_simple_program_execution() {
        let test_program_path = PathBuf::from("demo/governance/ranked_vote.dsl");
        
        // Skip test if the file doesn't exist in test environment
        if !test_program_path.exists() {
            println!("Test program not found, skipping test");
            return;
        }
        
        let options = VMOptions {
            storage_backend: StorageBackendType::Memory,
            identity_context: None, // Use default
            enable_federation: false,
            use_bytecode: false,
            debug_mode: false,
            simulation_mode: true, // Use simulation mode for tests
            trace_execution: false,
            explain_operations: false,
            verbose_storage_trace: false,
            namespace: "test".to_string(),
            parameters: HashMap::new(),
            use_stdlib: true,
        };
        
        let result = execute_program_from_path(test_program_path, options);
        
        assert!(result.is_ok(), "Program execution failed: {:?}", result.err());
        
        let exec_result = result.unwrap();
        assert!(exec_result.success, "Execution not successful: {:?}", exec_result.error);
        assert!(exec_result.stack.is_some(), "Stack should not be None");
        assert!(exec_result.memory.is_some(), "Memory should not be None");
    }
}
