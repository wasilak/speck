pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: EngineEvent);
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum EngineEvent {
    VmStateChanged { state: VmState },
    Log { level: LogLevel, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

pub struct NoopSink;

impl EventSink for NoopSink {
    fn emit(&self, _event: EngineEvent) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_sink_accepts_vm_state_changed() {
        let sink = NoopSink;
        sink.emit(EngineEvent::VmStateChanged {
            state: VmState::Running,
        });
    }

    #[test]
    fn noop_sink_used_as_trait_object() {
        fn accepts_sink(_sink: &dyn EventSink) {}
        let sink = NoopSink;
        accepts_sink(&sink);
    }

    #[test]
    fn vm_state_partial_eq() {
        assert_eq!(VmState::Running, VmState::Running);
        assert_ne!(VmState::Error("x".into()), VmState::Running);
    }

    #[test]
    fn log_variant_constructs() {
        let _event = EngineEvent::Log {
            level: LogLevel::Info,
            message: "hi".into(),
        };
    }
}
