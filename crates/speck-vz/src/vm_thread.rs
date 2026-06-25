//! Dedicated VM worker thread with a serial GCD dispatch queue.
//!
//! Apple's `Virtualization.framework` requires that all methods on a
//! `VZVirtualMachine` are called from a **single serial dispatch queue**
//! (`dispatch_queue_t`).  This module enforces that invariant at the
//! type level by spawning a dedicated OS thread that owns a serial GCD
//! queue and processing every command on it.
//!
//! Commands are submitted via a bounded `mpsc` channel; replies travel
//! back over `tokio::sync::oneshot` channels so callers can `await` the
//! result from their own async context.

use std::thread::{self, JoinHandle};
use tokio::sync::{mpsc, oneshot};

use dispatch2::{DispatchQueue, DispatchQueueAttr};

use crate::config::GuestConfig;
use crate::error::Error;

// ---------------------------------------------------------------------------
// Command & state types
// ---------------------------------------------------------------------------

/// Commands that can be sent to the VM worker thread.
///
/// Every variant that carries a `reply` channel **must** be answered
/// exactly once — the caller is blocked on the corresponding oneshot
/// receiver.
pub(crate) enum VmCommand {
    /// Boot the micro-VM with the given guest configuration.
    Start {
        config: GuestConfig,
        reply: oneshot::Sender<std::result::Result<InternalState, Error>>,
    },

    /// Gracefully stop the running VM.
    Stop {
        reply: oneshot::Sender<std::result::Result<InternalState, Error>>,
    },

    /// Snapshot the current internal state without side effects.
    State {
        reply: oneshot::Sender<InternalState>,
    },

    /// Drain signal — the thread exits after processing this.
    Shutdown,
}

/// Lifecycle state of the micro-VM.
///
/// This mirrors [`speck_core::VmState`] but is kept separate so the
/// VM thread can transition through states without depending on the
/// presentation layer.  Conversion helpers are provided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalState {
    Stopped,
    Starting,
    Running,
    Stopping,
}

impl From<InternalState> for speck_core::VmState {
    fn from(s: InternalState) -> Self {
        match s {
            InternalState::Stopped => speck_core::VmState::Stopped,
            InternalState::Starting => speck_core::VmState::Starting,
            InternalState::Running => speck_core::VmState::Running,
            InternalState::Stopping => speck_core::VmState::Stopping,
        }
    }
}

// ---------------------------------------------------------------------------
// VmThread
// ---------------------------------------------------------------------------

/// A thread-safe handle to the dedicated VM worker thread.
///
/// Dropping the handle without calling [`join`](VmThread::join) will
/// close the command channel, causing the worker thread to exit
/// gracefully on its own.
pub struct VmThread {
    /// Sender end of the command channel.
    sender: mpsc::Sender<VmCommand>,

    /// Optional join handle — taken by [`join`](VmThread::join).
    thread: Option<JoinHandle<()>>,
}

// All fields are automatically `Send + Sync`; the unsafe impls are
// provided explicitly to document that the type is designed to be
// shared across threads.
unsafe impl Send for VmThread {}
unsafe impl Sync for VmThread {}

impl VmThread {
    /// Spawn the VM worker thread and return a handle.
    ///
    /// The thread creates a serial GCD dispatch queue named
    /// `com.speck.vm` and blocks on the command channel.  All
    /// commands are dispatched synchronously onto the serial queue,
    /// satisfying the Virtualization.framework threading requirement.
    pub fn spawn() -> Self {
        let (tx, mut rx) = mpsc::channel::<VmCommand>(16);

        // Serial GCD queue — every VZVirtualMachine call must happen
        // on this queue.
        let queue = DispatchQueue::new("com.speck.vm", DispatchQueueAttr::SERIAL);

        let thread = thread::Builder::new()
            .name("speck-vm".into())
            .spawn(move || {
                // Keep the queue alive for the thread's entire lifetime.
                let _queue = queue;

                while let Some(cmd) = rx.blocking_recv() {
                    match cmd {
                        VmCommand::Start { config, reply } => {
                            // Future: construct VZVirtualMachine, boot it.
                            // For now the state machine is a placeholder
                            // that transitions straight to Running.
                            let _ = config; // consumed in 02-02
                            _queue.exec_sync(move || {
                                let _ = reply.send(Ok(InternalState::Running));
                            });
                        }
                        VmCommand::Stop { reply } => {
                            _queue.exec_sync(move || {
                                let _ = reply.send(Ok(InternalState::Stopped));
                            });
                        }
                        VmCommand::State { reply } => {
                            _queue.exec_sync(move || {
                                let _ = reply.send(InternalState::Stopped);
                            });
                        }
                        VmCommand::Shutdown => break,
                    }
                }
            })
            .expect("spawning speck-vm worker thread");

        Self {
            sender: tx,
            thread: Some(thread),
        }
    }

    // -- internal helpers ------------------------------------------------

    /// Send a command via the mpsc channel and await the oneshot reply.
    fn send_blocking<T>(
        &self,
        cmd: VmCommand,
        rx: oneshot::Receiver<T>,
    ) -> std::result::Result<T, Error> {
        self.sender
            .blocking_send(cmd)
            .map_err(|_| Error::ChannelError("vm thread channel closed".into()))?;
        rx.blocking_recv()
            .map_err(|_| Error::ChannelError("vm thread reply channel closed".into()))
    }

    // -- public API -------------------------------------------------------

    /// Start the VM with the given configuration.
    ///
    /// Returns the new state on success, or an error if the VM was
    /// already running or the thread died.
    #[allow(dead_code)]
    pub fn start(&self, config: GuestConfig) -> std::result::Result<InternalState, Error> {
        let (tx, rx) = oneshot::channel();
        self.send_blocking(VmCommand::Start { config, reply: tx }, rx)?
    }

    /// Issue a graceful stop.
    #[allow(dead_code)]
    pub fn stop(&self) -> std::result::Result<InternalState, Error> {
        let (tx, rx) = oneshot::channel();
        self.send_blocking(VmCommand::Stop { reply: tx }, rx)?
    }

    /// Query the current lifecycle state.
    #[allow(dead_code)]
    pub fn state(&self) -> std::result::Result<InternalState, Error> {
        let (tx, rx) = oneshot::channel();
        self.send_blocking(VmCommand::State { reply: tx }, rx)
    }

    /// Send the shutdown signal and block until the worker thread exits.
    ///
    /// After this returns the handle is consumed and the VM thread is
    /// guaranteed to have stopped.
    pub fn join(mut self) -> std::result::Result<(), Error> {
        self.sender
            .blocking_send(VmCommand::Shutdown)
            .map_err(|_| Error::ChannelError("vm thread channel closed".into()))?;

        if let Some(handle) = self.thread.take() {
            handle.join().map_err(|_| Error::ThreadJoin)?;
        }
        Ok(())
    }
}
