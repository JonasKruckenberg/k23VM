#![allow(static_mut_refs)]

use crate::placeholder::code_registry;
use core::ffi::c_void;
use core::mem::MaybeUninit;
use core::{mem, ptr};
use spin::once::Once;

static mut PREV_SIGSEGV: MaybeUninit<libc::sigaction> = MaybeUninit::uninit();
static mut PREV_SIGBUS: MaybeUninit<libc::sigaction> = MaybeUninit::uninit();
static mut PREV_SIGILL: MaybeUninit<libc::sigaction> = MaybeUninit::uninit();
static mut PREV_SIGFPE: MaybeUninit<libc::sigaction> = MaybeUninit::uninit();

pub unsafe fn ensure_signal_handlers_are_registered() {
    static SIGNAL_HANDLER: Once = Once::new();

    SIGNAL_HANDLER.call_once(|| {
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
                panic!("unable to install signal handler",);
            }
        });
    });
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

unsafe extern "C" fn trap_handler(
    signum: libc::c_int,
    siginfo: *mut libc::siginfo_t,
    context: *mut c_void,
) {
    let previous = match signum {
        libc::SIGSEGV => PREV_SIGSEGV.as_ptr(),
        libc::SIGBUS => PREV_SIGBUS.as_ptr(),
        libc::SIGFPE => PREV_SIGFPE.as_ptr(),
        libc::SIGILL => PREV_SIGILL.as_ptr(),
        _ => panic!("unknown signal: {signum}"),
    };

    let handled = (|| unsafe {
        let p = &crate::placeholder::trap_handling::TLS;

        // If no wasm code is executing, we don't handle this as a wasm
        // trap.
        let info = match p.get() {
            Some(info) => &*info,
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

        let cx = &*(context as *const libc::ucontext_t);
        let pc = (*cx.uc_mcontext).__ss.__pc as usize;
        let fp = (*cx.uc_mcontext).__ss.__fp as usize;

        // If this fault wasn't in wasm code, then it's not our problem
        let Some((code, text_offset)) = code_registry::lookup_code(pc) else {
            return false;
        };

        let Some(trap) = code.lookup_trap_code(text_offset) else {
            return false;
        };

        info.set_jit_trap(pc, fp, faulting_addr, trap);

        if cfg!(target_os = "macos") {
            unsafe extern "C" fn wasmtime_longjmp_shim(jmp_buf: *const u8) {
                crate::placeholder::setjmp::longjmp(jmp_buf.cast_mut().cast(), 1)
            }
            set_pc(
                context,
                wasmtime_longjmp_shim as usize,
                info.jmp_buf.as_ptr() as usize,
            );
            return true;
        }
        crate::placeholder::setjmp::longjmp(info.jmp_buf.as_ptr().cast(), 1)
    })();

    if handled {
        return;
    }

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

unsafe fn set_pc(cx: *mut c_void, pc: usize, arg1: usize) {
    let cx = &mut *(cx as *mut libc::ucontext_t);
    (*cx.uc_mcontext).__ss.__pc = pc as u64;
    (*cx.uc_mcontext).__ss.__x[0] = arg1 as u64;
}
