use crate::config::GuestConfig;
use crate::error::Error;
use crate::vm_thread::VmThread;
use crate::vsock::VzSocket;
use speck_core::EventSink;

pub struct Guest {
    thread: VmThread,
    config: GuestConfig,
    sink: Option<Box<dyn EventSink>>,
}

impl Guest {
    pub fn new(config: GuestConfig) -> Self {
        Self {
            thread: VmThread::spawn(),
            config,
            sink: None,
        }
    }

    pub fn with_sink(mut self, sink: Box<dyn EventSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    pub fn start(&self) -> Result<speck_core::VmState, Error> {
        let state = self.thread.start(self.config.clone())?;
        let core_state = speck_core::VmState::from(state);
        if let Some(ref sink) = self.sink {
            sink.emit(speck_core::EngineEvent::VmStateChanged {
                state: core_state.clone(),
            });
        }
        Ok(core_state)
    }

    pub fn stop(&self) -> Result<speck_core::VmState, Error> {
        let state = self.thread.stop(self.config.stop_timeout)?;
        let core_state = speck_core::VmState::from(state);
        if let Some(ref sink) = self.sink {
            sink.emit(speck_core::EngineEvent::VmStateChanged {
                state: core_state.clone(),
            });
        }
        Ok(core_state)
    }

    pub fn state(&self) -> speck_core::VmState {
        self.thread
            .state()
            .map(speck_core::VmState::from)
            .unwrap_or(speck_core::VmState::Stopped)
    }

    pub fn vsock_connect(&self, port: u32) -> Result<VzSocket, Error> {
        self.thread.vsock_connect(port)
    }

    pub fn join(&mut self) -> Result<(), Error> {
        self.thread.join()
    }

    pub fn netstack_fd(&self) -> Result<std::os::unix::io::RawFd, Error> {
        self.thread.netstack_fd()
    }

    pub fn dns_vsock_fd(&self) -> Result<std::os::unix::io::RawFd, Error> {
        self.thread.dns_vsock_fd()
    }
}

impl Drop for Guest {
    fn drop(&mut self) {
        let _ = self.thread.send_shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use speck_core::VmState;
    use std::time::Duration;

    #[test]
    fn test_guest_boots_to_running() {
        let config = GuestConfig {
            kernel_path: std::path::PathBuf::from(
                std::env::var("SPECK_HOME")
                    .map(|h| format!("{h}/kernel/vmlinux"))
                    .unwrap_or_else(|_| {
                        format!(
                            "{}/.local/share/speck/kernel/vmlinux",
                            std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
                        )
                    }),
            ),
            cmdline: "console=hvc0 quiet panic=-1".into(),
            ..Default::default()
        };
        let guest = Guest::new(config);
        let result = guest.start();
        assert!(result.is_ok(), "VM should boot: {:?}", result);
        assert_eq!(result.unwrap(), VmState::Running);
    }

    #[test]
    fn ten_start_stop_cycles() {
        if std::env::var("SPECK_STRESS_TEST").is_err() {
            eprintln!(
                "Skipping: set SPECK_STRESS_TEST=1 to run \
                 (requires virtualization entitlement)"
            );
            return;
        }

        let kernel_path = std::env::var("SPECK_HOME")
            .map(|h| format!("{h}/kernel/vmlinux"))
            .unwrap_or_else(|_| {
                format!(
                    "{}/.local/share/speck/kernel/vmlinux",
                    std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
                )
            });
        let config = GuestConfig {
            kernel_path: kernel_path.into(),
            cmdline: "console=hvc0 quiet panic=-1".into(),
            stop_timeout: Duration::from_secs(10),
            ..Default::default()
        };

        for i in 0..10 {
            let guest = Guest::new(config.clone());
            let start = std::time::Instant::now();

            let state = guest
                .start()
                .unwrap_or_else(|e| panic!("Cycle {i}: start should succeed: {e}"));

            assert_eq!(
                state,
                VmState::Running,
                "Cycle {i}: state should be Running"
            );
            let boot_time = start.elapsed();
            println!("Cycle {i}: booted in {boot_time:?}");

            std::thread::sleep(std::time::Duration::from_millis(100));

            guest
                .stop()
                .unwrap_or_else(|e| panic!("Cycle {i}: stop should succeed: {e}"));

            let stop_time = start.elapsed() - boot_time;
            println!("Cycle {i}: stopped in ~{stop_time:?}");
        }
    }
}
