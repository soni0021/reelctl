//! The two kinds of failure, which the command line turns into exit codes.

/// A failure of the compositor. `Setup` is something the user can fix before the
/// run (a path, a variable, a flag): exit code 2. `Run` happened while working:
/// exit code 1.
#[derive(Debug, thiserror::Error)]
pub enum CompositorError {
    #[error("{0}")]
    Setup(String),
    #[error("{0}")]
    Run(String),
}

impl CompositorError {
    pub fn setup(message: impl Into<String>) -> Self {
        Self::Setup(message.into())
    }

    pub fn run(message: impl Into<String>) -> Self {
        Self::Run(message.into())
    }

    /// Whether the user can fix this by changing the command or the environment.
    pub fn is_setup(&self) -> bool {
        matches!(self, Self::Setup(_))
    }
}

#[cfg(test)]
mod tests {
    use super::CompositorError;

    #[test]
    fn setup_and_run_are_told_apart() {
        assert!(CompositorError::setup("a").is_setup());
        assert!(!CompositorError::run("a").is_setup());
        assert_eq!(CompositorError::run("boom").to_string(), "boom");
    }
}
