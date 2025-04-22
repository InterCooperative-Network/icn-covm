//! Governance CLI commands
//!
//! This module provides CLI commands for working with the governance system,
//! including proposal management, voting, and template handling.

use crate::cli::helpers::Output;
use crate::cli::governance_template::{TemplateCommand, execute as execute_template};
use clap::{Args, Subcommand};
use std::path::PathBuf;

/// CLI commands for governance operations
#[derive(Debug, Args)]
pub struct GovernanceCommand {
    /// Subcommand for governance operations
    #[command(subcommand)]
    pub command: GovernanceSubcommand,
}

/// Subcommands for governance operations
#[derive(Debug, Subcommand)]
pub enum GovernanceSubcommand {
    /// Governance proposal commands
    Proposal {
        /// Proposal subcommand
        #[command(subcommand)]
        command: ProposalSubcommand,
    },
    
    /// Governance template commands
    Template(TemplateCommand),
    
    /// Vote on a proposal
    Vote {
        /// Proposal ID
        id: String,
        
        /// Vote value (yes, no, abstain)
        #[arg(short, long)]
        choice: String,
        
        /// Identity file for signing
        #[arg(short, long)]
        identity: PathBuf,
        
        /// Optional comment on vote
        #[arg(short, long)]
        comment: Option<String>,
        
        /// Optional vote weight (for custom weighting)
        #[arg(short, long)]
        weight: Option<f64>,
    },
}

/// Subcommands for proposal operations
#[derive(Debug, Subcommand)]
pub enum ProposalSubcommand {
    /// Create a new proposal
    Create {
        /// Title of the proposal
        title: String,
        
        /// Description of the proposal
        description: String,
        
        /// Program file (JSON or DSL)
        #[arg(short, long)]
        program: PathBuf,
        
        /// Template ID to use (optional)
        #[arg(short, long)]
        template: Option<String>,
        
        /// Identity file for signing
        #[arg(short, long)]
        identity: PathBuf,
        
        /// Quorum method (percentage:<0-100>, fixed:<number>, none)
        #[arg(long, default_value = "percentage:50")]
        quorum: String,
        
        /// Approval threshold percentage (0-100)
        #[arg(long, default_value = "51")]
        threshold: u8,
        
        /// Vote weighting strategy (equal, reputation, stake, tenure, quadratic, delegated, custom:<name>)
        #[arg(long, default_value = "equal")]
        weighting: String,
        
        /// Optional expiry time in seconds from now
        #[arg(long)]
        expires_in: Option<u64>,
        
        /// Whether to execute automatically when passed
        #[arg(long)]
        auto_execute: bool,
    },
    
    /// Submit a proposal from a DSL file
    Submit {
        /// Path to the proposal DSL file
        #[arg(short, long)]
        file: PathBuf,
        
        /// Identity file for signing
        #[arg(short, long)]
        identity: PathBuf,
        
        /// Optional title override
        #[arg(short, long)]
        title: Option<String>,
        
        /// Optional description override
        #[arg(short, long)]
        description: Option<String>,
    },
    
    /// List all proposals
    List {
        /// Filter by status (active, pending, approved, rejected, expired, all)
        #[arg(short, long, default_value = "active")]
        status: String,
        
        /// Show detailed information
        #[arg(short, long)]
        verbose: bool,
        
        /// Sort by field (created, expires, votes)
        #[arg(long, default_value = "created")]
        sort: String,
        
        /// Maximum number of proposals to show
        #[arg(long)]
        limit: Option<usize>,
    },
    
    /// View a proposal
    View {
        /// Proposal ID
        id: String,
        
        /// Show detailed information including votes
        #[arg(short, long)]
        verbose: bool,
        
        /// Show vote distribution
        #[arg(long)]
        votes: bool,
    },
    
    /// Execute an approved proposal
    Execute {
        /// Proposal ID
        id: String,
        
        /// Identity file for signing
        #[arg(short, long)]
        identity: PathBuf,
        
        /// Force execution even if conditions aren't met
        #[arg(long)]
        force: bool,
    },
}

/// Execute governance CLI commands
pub fn execute(cmd: GovernanceCommand, data_dir: PathBuf) -> Output {
    match cmd.command {
        GovernanceSubcommand::Proposal { command } => {
            execute_proposal(command, data_dir)
        },
        GovernanceSubcommand::Template(template_cmd) => {
            // Templates are stored in a subdirectory of the data directory
            let templates_dir = data_dir.join("templates");
            execute_template(template_cmd, templates_dir)
        },
        GovernanceSubcommand::Vote { id, choice, identity, comment, weight } => {
            execute_vote(id, choice, identity, comment, weight, data_dir)
        },
    }
}

/// Execute proposal subcommands
fn execute_proposal(cmd: ProposalSubcommand, data_dir: PathBuf) -> Output {
    match cmd {
        ProposalSubcommand::Create { 
            title, 
            description, 
            program, 
            template, 
            identity,
            quorum,
            threshold,
            weighting,
            expires_in,
            auto_execute 
        } => {
            // TODO: Implement enhanced proposal creation
            Output::info(format!(
                "Creating proposal '{}' with quorum '{}', threshold '{}%', weighting '{}'", 
                title, quorum, threshold, weighting
            ))
        },
        ProposalSubcommand::Submit { file, identity, title, description } => {
            // TODO: Implement proposal submission from file
            Output::info(format!(
                "Submitting proposal from file: {}", 
                file.display()
            ))
        },
        ProposalSubcommand::List { status, verbose, sort, limit } => {
            // TODO: Implement enhanced proposal listing
            let limit_str = limit.map_or("all".to_string(), |l| l.to_string());
            Output::info(format!(
                "Listing {} proposals with status '{}', sorted by '{}'{}", 
                limit_str, status, sort,
                if verbose { " (verbose)" } else { "" }
            ))
        },
        ProposalSubcommand::View { id, verbose, votes } => {
            // TODO: Implement enhanced proposal viewing
            let detail_level = match (verbose, votes) {
                (true, true) => "detailed with vote distribution",
                (true, false) => "detailed",
                (false, true) => "vote distribution only",
                (false, false) => "basic",
            };
            Output::info(format!("Viewing proposal ID: {} ({})", id, detail_level))
        },
        ProposalSubcommand::Execute { id, identity, force } => {
            // TODO: Implement enhanced proposal execution
            let force_str = if force { " (forced)" } else { "" };
            Output::info(format!("Executing proposal ID: {}{}", id, force_str))
        },
    }
}

/// Execute vote command
fn execute_vote(
    id: String, 
    choice: String, 
    identity: PathBuf, 
    comment: Option<String>, 
    weight: Option<f64>,
    data_dir: PathBuf
) -> Output {
    // Parse the vote choice
    let vote_result = match choice.to_lowercase().as_str() {
        "yes" | "y" => "YES",
        "no" | "n" => "NO", 
        "abstain" | "a" => "ABSTAIN",
        other => return Output::error(format!("Invalid vote choice: '{}'. Must be yes, no, or abstain", other)),
    };
    
    // Handle vote weight if provided
    let weight_str = if let Some(w) = weight {
        format!(" with weight {}", w)
    } else {
        String::new()
    };
    
    // Handle comment if provided
    let comment_str = if let Some(c) = comment {
        format!(" and comment: \"{}\"", c)
    } else {
        String::new()
    };
    
    // TODO: Implement actual voting logic
    Output::info(format!(
        "Casting vote '{}'{}{} on proposal ID: {}", 
        vote_result, weight_str, comment_str, id
    ))
} 