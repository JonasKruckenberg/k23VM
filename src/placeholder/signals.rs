#![allow(static_mut_refs)]

use crate::trap::signals;
use std::cell::Cell;
use std::mem::MaybeUninit;
use std::{io, mem, ptr};

#[derive(Debug)]
pub(crate) struct TrapRegisters {
    pub pc: usize,
    pub fp: usize,
}

/// Return value from `test_if_trap`.
pub(crate) enum TrapTest {
    /// Not a wasm trap, need to delegate to whatever process handler is next.
    NotWasm,
    /// This trap was handled by the embedder via custom embedding APIs.
    HandledByEmbedder,
}

pub unsafe fn catch_traps<F>(
    signal_handler: Option<*const SignalHandler<'static>>,
    mut closure: F,
) -> Result<(), ()>
where
    F: FnMut(),
{
    return CallThreadState::new(signal_handler).with(|cx| {
        let _handler = TrapHandler::new(false);
        closure();
        -1
    });

    extern "C" fn call_closure<F>(payload: *mut u8)
    where
        F: FnMut(),
    {
        unsafe { (*(payload as *mut F))() }
    }
}

/// Function which may handle custom signals while processing traps.
pub type SignalHandler<'a> =
    dyn Fn(libc::c_int, *const libc::siginfo_t, *const libc::c_void) -> bool + Send + 'a;

static mut PREV_SIGSEGV: MaybeUninit<libc::sigaction> = MaybeUninit::uninit();
static mut PREV_SIGBUS: MaybeUninit<libc::sigaction> = MaybeUninit::uninit();
static mut PREV_SIGILL: MaybeUninit<libc::sigaction> = MaybeUninit::uninit();
static mut PREV_SIGFPE: MaybeUninit<libc::sigaction> = MaybeUninit::uninit();

pub struct TrapHandler;

impl TrapHandler {
    /// Installs all trap handlers.
    ///
    /// # Unsafety
    ///
    /// This function is unsafe because it's not safe to call concurrently and
    /// it's not safe to call if the trap handlers have already been initialized
    /// for this process.
    pub unsafe fn new(macos_use_mach_ports: bool) -> TrapHandler {
        // Either mach ports shouldn't be in use or we shouldn't be on macOS,
        // otherwise the `machports.rs` module should be used instead.
        assert!(!macos_use_mach_ports || !cfg!(target_os = "macos"));

        foreach_handler(|slot, signal| {
            let mut handler: libc::sigaction = mem::zeroed();
            // The flags here are relatively careful, and they are...
            //
            // SA_SIGINFO gives us access to information like the program
            // counter from where the fault happened.
            //
            // SA_ONSTACK allows us to handle signals on an alternate stack,
            // so that the handler can run in response to running out of
            // stack space on the main stack. Rust installs an alternate
            // stack with sigaltstack, so we rely on that.
            //
            // SA_NODEFER allows us to reenter the signal handler if we
            // crash while handling the signal, and fall through to the
            // Breakpad handler by testing handlingSegFault.
            handler.sa_flags = libc::SA_SIGINFO | libc::SA_NODEFER | libc::SA_ONSTACK;
            handler.sa_sigaction = trap_handler as usize;
            libc::sigemptyset(&mut handler.sa_mask);
            if libc::sigaction(signal, &handler, slot) != 0 {
                panic!(
                    "unable to install signal handler: {}",
                    io::Error::last_os_error(),
                );
            }
        });

        TrapHandler
    }

    pub fn validate_config(&self, macos_use_mach_ports: bool) {
        assert!(!macos_use_mach_ports || !cfg!(target_os = "macos"));
    }
}

unsafe fn foreach_handler(mut f: impl FnMut(*mut libc::sigaction, i32)) {
    // Allow handling OOB with signals on all architectures
    f(PREV_SIGSEGV.as_mut_ptr(), libc::SIGSEGV);

    // Handle `unreachable` instructions which execute `ud2` right now
    f(PREV_SIGILL.as_mut_ptr(), libc::SIGILL);

    // x86 and s390x use SIGFPE to report division by zero
    if cfg!(target_arch = "x86_64") || cfg!(target_arch = "s390x") {
        f(PREV_SIGFPE.as_mut_ptr(), libc::SIGFPE);
    }

    // Sometimes we need to handle SIGBUS too:
    // - On Darwin, guard page accesses are raised as SIGBUS.
    if cfg!(target_os = "macos") || cfg!(target_os = "freebsd") {
        f(PREV_SIGBUS.as_mut_ptr(), libc::SIGBUS);
    }

    // TODO(#1980): x86-32, if we support it, will also need a SIGFPE handler.
    // TODO(#1173): ARM32, if we support it, will also need a SIGBUS handler.
}

impl Drop for TrapHandler {
    fn drop(&mut self) {
        unsafe {
            foreach_handler(|slot, signal| {
                let mut prev: libc::sigaction = mem::zeroed();

                // Restore the previous handler that this signal had.
                if libc::sigaction(signal, slot, &mut prev) != 0 {
                    eprintln!(
                        "unable to reinstall signal handler: {}",
                        io::Error::last_os_error(),
                    );
                    libc::abort();
                }

                // If our trap handler wasn't currently listed for this process
                // then that's a problem because we have just corrupted the
                // signal handler state and don't know how to remove ourselves
                // from the signal handling state. Inform the user of this and
                // abort the process.
                if prev.sa_sigaction != trap_handler as usize {
                    eprintln!(
                        "
Wasmtime's signal handler was not the last signal handler to be installed
in the process so it's not certain how to unload signal handlers. In this
situation the Engine::unload_process_handlers API is not applicable and requires
perhaps initializing libraries in a different order. The process will be aborted
now.
"
                    );
                    libc::abort();
                }
            });
        }
    }
}

unsafe extern "C" fn trap_handler(
    signum: libc::c_int,
    siginfo: *mut libc::siginfo_t,
    context: *mut libc::c_void,
) {
    let previous = match signum {
        libc::SIGSEGV => PREV_SIGSEGV.as_ptr(),
        libc::SIGBUS => PREV_SIGBUS.as_ptr(),
        libc::SIGFPE => PREV_SIGFPE.as_ptr(),
        libc::SIGILL => PREV_SIGILL.as_ptr(),
        _ => panic!("unknown signal: {signum}"),
    };
    let handled = tls::with(|info| {
        // If no wasm code is executing, we don't handle this as a wasm
        // trap.
        let info = match info {
            Some(info) => info,
            None => return false,
        };

        // If we hit an exception while handling a previous trap, that's
        // quite bad, so bail out and let the system handle this
        // recursive segfault.
        //
        // Otherwise flag ourselves as handling a trap, do the trap
        // handling, and reset our trap handling flag. Then we figure
        // out what to do based on the result of the trap handling.
        let faulting_addr = match signum {
            libc::SIGSEGV | libc::SIGBUS => Some((*siginfo).si_addr() as usize),
            _ => None,
        };
        let regs = get_trap_registers(context, signum);
        let test = info.test_if_trap(regs, faulting_addr, |handler| {
            handler(signum, siginfo, context)
        });

        // Figure out what to do based on the result of this handling of
        // the trap. Note that our sentinel value of 1 means that the
        // exception was handled by a custom exception handler, so we
        // keep executing.
        match test {
            TrapTest::NotWasm => false,
            TrapTest::HandledByEmbedder => true,
        }
    });

    if handled {
        return;
    }

    // This signal is not for any compiled wasm code we expect, so we
    // need to forward the signal to the next handler. If there is no
    // next handler (SIG_IGN or SIG_DFL), then it's time to crash. To do
    // this, we set the signal back to its original disposition and
    // return. This will cause the faulting op to be re-executed which
    // will crash in the normal way. If there is a next handler, call
    // it. It will either crash synchronously, fix up the instruction
    // so that execution can continue and return, or trigger a crash by
    // returning the signal to it's original disposition and returning.
    let previous = *previous;
    if previous.sa_flags & libc::SA_SIGINFO != 0 {
        mem::transmute::<usize, extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void)>(
            previous.sa_sigaction,
        )(signum, siginfo, context)
    } else if previous.sa_sigaction == libc::SIG_DFL || previous.sa_sigaction == libc::SIG_IGN {
        libc::sigaction(signum, &previous as *const _, ptr::null_mut());
    } else {
        mem::transmute::<usize, extern "C" fn(libc::c_int)>(previous.sa_sigaction)(signum)
    }
}

#[allow(clippy::cast_possible_truncation)] // too fiddly to handle and wouldn't
                                           // help much anyway
pub unsafe fn get_trap_registers(cx: *mut libc::c_void, _signum: libc::c_int) -> TrapRegisters {
    cfg_if::cfg_if! {
        if #[cfg(all(any(target_os = "linux", target_os = "android"), target_arch = "x86_64"))] {
            let cx = &*(cx as *const libc::ucontext_t);
            TrapRegisters {
                pc: cx.uc_mcontext.gregs[libc::REG_RIP as usize] as usize,
                fp: cx.uc_mcontext.gregs[libc::REG_RBP as usize] as usize,
            }
        } else if #[cfg(all(any(target_os = "linux", target_os = "android"), target_arch = "aarch64"))] {
            let cx = &*(cx as *const libc::ucontext_t);
            TrapRegisters {
                pc: cx.uc_mcontext.pc as usize,
                fp: cx.uc_mcontext.regs[29] as usize,
            }
        } else if #[cfg(all(target_os = "linux", target_arch = "s390x"))] {
            // On s390x, SIGILL and SIGFPE are delivered with the PSW address
            // pointing *after* the faulting instruction, while SIGSEGV and
            // SIGBUS are delivered with the PSW address pointing *to* the
            // faulting instruction.  To handle this, the code generator registers
            // any trap that results in one of "late" signals on the last byte
            // of the instruction, and any trap that results in one of the "early"
            // signals on the first byte of the instruction (as usual).  This
            // means we simply need to decrement the reported PSW address by
            // one in the case of a "late" signal here to ensure we always
            // correctly find the associated trap handler.
            let trap_offset = match _signum {
                libc::SIGILL | libc::SIGFPE => 1,
                _ => 0,
            };
            let cx = &*(cx as *const libc::ucontext_t);
            TrapRegisters {
                pc: (cx.uc_mcontext.psw.addr - trap_offset) as usize,
                fp: *(cx.uc_mcontext.gregs[15] as *const usize),
            }
        } else if #[cfg(all(target_os = "macos", target_arch = "x86_64"))] {
            let cx = &*(cx as *const libc::ucontext_t);
            TrapRegisters {
                pc: (*cx.uc_mcontext).__ss.__rip as usize,
                fp: (*cx.uc_mcontext).__ss.__rbp as usize,
            }
        } else if #[cfg(all(target_os = "macos", target_arch = "aarch64"))] {
            let cx = &*(cx as *const libc::ucontext_t);
            TrapRegisters {
                pc: (*cx.uc_mcontext).__ss.__pc as usize,
                fp: (*cx.uc_mcontext).__ss.__fp as usize,
            }
        } else if #[cfg(all(target_os = "freebsd", target_arch = "x86_64"))] {
            let cx = &*(cx as *const libc::ucontext_t);
            TrapRegisters {
                pc: cx.uc_mcontext.mc_rip as usize,
                fp: cx.uc_mcontext.mc_rbp as usize,
            }
        } else if #[cfg(all(target_os = "linux", target_arch = "riscv64"))] {
            let cx = &*(cx as *const libc::ucontext_t);
            TrapRegisters {
                pc: cx.uc_mcontext.__gregs[libc::REG_PC] as usize,
                fp: cx.uc_mcontext.__gregs[libc::REG_S0] as usize,
            }
        } else if #[cfg(all(target_os = "freebsd", target_arch = "aarch64"))] {
            let cx = &*(cx as *const libc::mcontext_t);
            TrapRegisters {
                pc: cx.mc_gpregs.gp_elr as usize,
                fp: cx.mc_gpregs.gp_x[29] as usize,
            }
        } else if #[cfg(all(target_os = "openbsd", target_arch = "x86_64"))] {
            let cx = &*(cx as *const libc::ucontext_t);
            TrapRegisters {
                pc: cx.sc_rip as usize,
                fp: cx.sc_rbp as usize,
            }
        }
        else {
            compile_error!("unsupported platform");
            panic!();
        }
    }
}

pub struct CallThreadState {
    pub(crate) signal_handler: Option<*const SignalHandler<'static>>,
    pub(crate) prev: Cell<tls::Ptr>,
}

impl CallThreadState {
    #[inline]
    pub(crate) fn new(signal_handler: Option<*const SignalHandler<'static>>) -> CallThreadState {
        CallThreadState {
            signal_handler,
            prev: Cell::new(ptr::null()),
        }
    }

    #[inline]
    fn with(mut self, closure: impl FnOnce(&CallThreadState) -> i32) -> Result<(), ()> {
        let ret = tls::set(&mut self, |me| closure(me));
        if ret != 0 {
            Ok(())
        } else {
            Err(())
        }
    }

    pub(crate) fn test_if_trap(
        &self,
        regs: TrapRegisters,
        faulting_addr: Option<usize>,
        call_handler: impl Fn(&SignalHandler) -> bool,
    ) -> TrapTest {
        // First up see if any instance registered has a custom trap handler,
        // in which case run them all. If anything handles the trap then we
        // return that the trap was handled.
        if let Some(handler) = self.signal_handler {
            if unsafe { call_handler(&*handler) } {
                return TrapTest::HandledByEmbedder;
            }
        }

        // If this fault wasn't in wasm code, then it's not our problem
        let Some((code, text_offset)) = signals::lookup_code(regs.pc) else {
            return TrapTest::NotWasm;
        };

        let Some(trap) = code.lookup_trap_code(text_offset) else {
            return TrapTest::NotWasm;
        };

        todo!()
    }

    /// Get the previous `CallThreadState`.
    pub fn prev(&self) -> tls::Ptr {
        self.prev.get()
    }

    #[inline]
    pub(crate) unsafe fn push(&self) {
        assert!(self.prev.get().is_null());
        self.prev.set(tls::TLS.replace(self));
    }

    #[inline]
    pub(crate) unsafe fn pop(&self) {
        let prev = self.prev.replace(ptr::null());
        let head = tls::TLS.replace(prev);
        assert!(ptr::eq(head, self));
    }
}

mod tls {
    use crate::placeholder::signals::CallThreadState;
    use std::cell::Cell;
    use std::ptr;

    pub type Ptr = *const CallThreadState;

    const _: () = {
        assert!(align_of::<CallThreadState>() > 1);
    };

    std::thread_local!(pub static TLS: Cell<Ptr> = const { Cell::new(ptr::null_mut()) });

    #[inline]
    pub fn set<R>(state: &mut CallThreadState, closure: impl FnOnce(&CallThreadState) -> R) -> R {
        struct Reset<'a> {
            state: &'a CallThreadState,
        }

        impl Drop for Reset<'_> {
            #[inline]
            fn drop(&mut self) {
                unsafe {
                    self.state.pop();
                }
            }
        }

        unsafe {
            state.push();
            let reset = Reset { state };
            closure(reset.state)
        }
    }

    /// Returns the last pointer configured with `set` above, if any.
    pub fn with<R>(closure: impl FnOnce(Option<&CallThreadState>) -> R) -> R {
        let p: *mut CallThreadState = TLS.get().cast_mut();

        unsafe { closure(if p.is_null() { None } else { Some(&*p) }) }
    }
}
