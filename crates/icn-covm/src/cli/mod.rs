pub mod federation;
pub mod federation_test;
pub mod governance;
pub mod governance_template;
pub mod proposal;
pub mod proposal_demo;
pub mod utils;
pub mod dag_sync;

pub mod helpers {
    /// Helper for CLI command output
    #[derive(Debug)]
    pub enum Output {
        /// Info message
        Info(String),
        /// Warning message
        Warning(String),
        /// Error message
        Error(String),
    }

    impl Output {
        /// Create an info output
        pub fn info<S: Into<String>>(message: S) -> Self {
            Self::Info(message.into())
        }

        /// Create a warning output
        pub fn warning<S: Into<String>>(message: S) -> Self {
            Self::Warning(message.into())
        }

        /// Create an error output
        pub fn error<S: Into<String>>(message: S) -> Self {
            Self::Error(message.into())
        }
    }
}

// Re-export key components
pub use federation::federation_command;
pub use proposal::proposal_command;
