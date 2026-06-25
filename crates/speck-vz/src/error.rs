use thiserror::Error;

/// Errors originating from the Virtualization.framework binding layer.
#[derive(Error, Debug)]
pub enum Error {
    /// The underlying framework returned an error.
    #[error("Virtualization framework error: {0}")]
    VmFramework(String),

    /// An operation was attempted that requires the VM to be stopped.
    #[error("VM is already running")]
    AlreadyRunning,

    /// An operation was attempted that requires the VM to be running.
    #[error("VM is not running")]
    NotRunning,

    /// A start operation did not complete within the expected window.
    #[error("VM start timed out")]
    StartTimeout,

    /// A stop operation did not complete within the expected window.
    #[error("VM stop timed out")]
    StopTimeout,

    /// Internal communication channel broke (thread exited unexpectedly).
    #[error("Internal channel error: {0}")]
    ChannelError(String),

    /// Failed to join the VM worker thread.
    #[error("Internal thread error")]
    ThreadJoin,
}

/// Convenience alias for `std::result::Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;
