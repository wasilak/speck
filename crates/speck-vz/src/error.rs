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

    /// Failed to connect to the guest vsock port.
    #[error("vsock connect failed: {0}")]
    VsockConnect(String),

    /// Connection timed out — no guest listening on the vsock port.
    #[error("vsock connection timed out")]
    VsockTimeout,

    /// I/O error on vsock socket read/write.
    #[error("vsock I/O error: {0}")]
    VsockIo(#[source] std::io::Error),

    /// Network device configuration or operation error.
    #[error("Network error: {0}")]
    Network(String),

    /// Network I/O error (socketpair, dup, etc.).
    #[error("Network I/O error: {0}")]
    NetworkIo(#[source] std::io::Error),

    /// Disk attachment error (virtio-blk attachment failure).
    #[error("Disk attachment error: {0}")]
    DiskAttachment(String),

    /// Guest ready signal not received within the expected window.
    #[error(
        "guest ready signal timed out on vsock port {_0} (last error: {_1}) — check console.log for guest boot messages"
    )]
    GuestReadyTimeout(u32, String),

    /// VirtioFS mount configuration error.
    #[error("VirtioFS mount error: {0}")]
    VirtioFsMount(String),
}

/// Convenience alias for `std::result::Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;
