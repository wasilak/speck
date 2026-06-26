use objc2::rc::{Allocated, Retained};
use objc2::{AnyThread, ClassType, DefinedClass, define_class, msg_send};
use objc2_foundation::{NSError, NSObject, NSObjectProtocol};
use objc2_virtualization::{VZVirtualMachine, VZVirtualMachineDelegate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VmStateEvent {
    Stopped,
    Error,
}

pub(crate) struct VmDelegateIvars {
    callback_ptr: *mut std::ffi::c_void,
}

// SAFETY: callback_ptr is only accessed from the serial dispatch queue.
unsafe impl Send for VmDelegateIvars {}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = AnyThread]
    #[ivars = VmDelegateIvars]
    pub(crate) struct VmDelegate;

    unsafe impl NSObjectProtocol for VmDelegate {}

    unsafe impl VZVirtualMachineDelegate for VmDelegate {
        #[unsafe(method(guestDidStopVirtualMachine:))]
        #[allow(non_snake_case)]
        unsafe fn guestDidStopVirtualMachine(&self, _virtual_machine: &VZVirtualMachine) {
            // SAFETY: emit_event reads the callback pointer which was set during init
            // and never modified thereafter.
            unsafe { Self::emit_event(self, VmStateEvent::Stopped) };
        }

        #[unsafe(method(virtualMachine:didStopWithError:))]
        #[allow(non_snake_case)]
        unsafe fn virtualMachine_didStopWithError(
            &self,
            _virtual_machine: &VZVirtualMachine,
            _error: &NSError,
        ) {
            // SAFETY: emit_event reads the callback pointer which was set during init
            // and never modified thereafter.
            unsafe { Self::emit_event(self, VmStateEvent::Error) };
        }
    }
);

impl VmDelegate {
    unsafe fn emit_event(&self, event: VmStateEvent) {
        let ptr = self.ivars().callback_ptr;
        if ptr.is_null() {
            return;
        }
        let callback = unsafe { &*(ptr as *const Box<dyn Fn(VmStateEvent) + Send>) };
        callback(event);
    }

    pub(crate) fn create(sender: std::sync::mpsc::Sender<VmStateEvent>) -> Retained<Self> {
        let callback: Box<dyn Fn(VmStateEvent) + Send> = Box::new(move |event| {
            let _ = sender.send(event);
        });
        let callback_ptr = Box::into_raw(Box::new(callback));

        unsafe {
            let alloc: Allocated<Self> = msg_send![VmDelegate::class(), alloc];
            let alloc = alloc.set_ivars(VmDelegateIvars {
                callback_ptr: callback_ptr.cast(),
            });
            msg_send![super(alloc), init]
        }
    }
}
