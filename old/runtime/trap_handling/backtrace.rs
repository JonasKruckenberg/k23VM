use alloc::vec;
use alloc::vec::Vec;
use crate::runtime::arch;
use crate::runtime::trap_handling::CallThreadState;
use core::ops::ControlFlow;

#[derive(Debug)]
pub struct Backtrace(Vec<Frame>);

impl Backtrace {
    pub(crate) unsafe fn new_with_trap_state(
        state: &CallThreadState,
        trap_pc_and_fp: Option<(usize, usize)>,
    ) -> Self {
        let mut frames = vec![];
        Self::trace_with_trap_state(state, trap_pc_and_fp, |frame| {
            frames.push(frame);
            ControlFlow::Continue(())
        });
        Backtrace(frames)
    }

    /// Walk the current Wasm stack, calling `f` for each frame we walk.
    pub(crate) unsafe fn trace_with_trap_state(
        state: &CallThreadState,
        trap_pc_and_fp: Option<(usize, usize)>,
        mut f: impl FnMut(Frame) -> ControlFlow<()>,
    ) {
        tracing::trace!("====== Capturing Backtrace ======");

        match trap_pc_and_fp {
            Some((pc, fp)) => {}
            None => {}
        }

        let (last_wasm_exit_pc, last_wasm_exit_fp) = match trap_pc_and_fp {
            // If we exited Wasm by catching a trap, then the Wasm-to-host
            // trampoline did not get a chance to save the last Wasm PC and FP,
            // and we need to use the plumbed-through values instead.
            Some((pc, fp)) => (pc, fp),
            // Either there is no Wasm currently on the stack, or we exited Wasm
            // through the Wasm-to-host trampoline.
            None => {
                // TODO this is horrible can we improve this?
                let pc = *state
                    .vmctx
                    .byte_add(state.vmoffsets.last_wasm_exit_pc as usize)
                    .cast::<usize>();
                let fp = *state
                    .vmctx
                    .byte_add(state.vmoffsets.last_wasm_exit_fp as usize)
                    .cast::<usize>();

                (pc, fp)
            }
        };

        let last_wasm_entry_fp = *state
            .vmctx
            .byte_add(state.vmoffsets.last_wasm_entry_fp as usize)
            .cast::<usize>();

        let activations =
            core::iter::once((last_wasm_exit_pc, last_wasm_exit_fp, last_wasm_entry_fp))
                .chain(state.iter().map(|state| {
                    (
                        state.old_last_wasm_exit_pc.get(),
                        state.old_last_wasm_exit_fp.get(),
                        state.old_last_wasm_entry_fp.get(),
                    )
                }))
                .take_while(|&(pc, fp, sp)| {
                    if pc == 0 {
                        debug_assert_eq!(fp, 0);
                        debug_assert_eq!(sp, 0);
                    }
                    pc != 0
                });

        for (pc, fp, sp) in activations {
            if let ControlFlow::Break(()) = Self::trace_through_wasm(pc, fp, sp, &mut f) {
                tracing::trace!("====== Done Capturing Backtrace (closure break) ======");
                return;
            }
        }

        tracing::trace!("====== Done Capturing Backtrace (reached end of activations) ======");
    }

    /// Walk through a contiguous sequence of Wasm frames starting with the
    /// frame at the given PC and FP and ending at `trampoline_sp`.
    unsafe fn trace_through_wasm(
        mut pc: usize,
        mut fp: usize,
        trampoline_fp: usize,
        mut f: impl FnMut(Frame) -> ControlFlow<()>,
    ) -> ControlFlow<()> {
        tracing::trace!("=== Tracing through contiguous sequence of Wasm frames ===");
        tracing::trace!("trampoline_fp = 0x{:016x}", trampoline_fp);
        tracing::trace!("   initial pc = 0x{:016x}", pc);
        tracing::trace!("   initial fp = 0x{:016x}", fp);

        // We already checked for this case in the `trace_with_trap_state`
        // caller.
        assert_ne!(pc, 0);
        assert_ne!(fp, 0);
        assert_ne!(trampoline_fp, 0);

        // This loop will walk the linked list of frame pointers starting at
        // `fp` and going up until `trampoline_fp`. We know that both `fp` and
        // `trampoline_fp` are "trusted values" aka generated and maintained by
        // Cranelift. This means that it should be safe to walk the linked list
        // of pointers and inspect wasm frames.
        //
        // Note, though, that any frames outside of this range are not
        // guaranteed to have valid frame pointers. For example native code
        // might be using the frame pointer as a general purpose register. Thus
        // we need to be careful to only walk frame pointers in this one
        // contiguous linked list.
        //
        // To know when to stop iteration all architectures' stacks currently
        // look something like this:
        //
        //     | ...               |
        //     | Native Frames     |
        //     | ...               |
        //     |-------------------|
        //     | ...               | <-- Trampoline FP            |
        //     | Trampoline Frame  |                              |
        //     | ...               | <-- Trampoline SP            |
        //     |-------------------|                            Stack
        //     | Return Address    |                            Grows
        //     | Previous FP       | <-- Wasm FP                Down
        //     | ...               |                              |
        //     | Wasm Frames       |                              |
        //     | ...               |                              V
        //
        // The trampoline records its own frame pointer (`trampoline_fp`),
        // which is guaranteed to be above all Wasm. To check when we've
        // reached the trampoline frame, it is therefore sufficient to
        // check when the next frame pointer is equal to `trampoline_fp`. Once
        // that's hit then we know that the entire linked list has been
        // traversed.
        //
        // Note that it might be possible that this loop doesn't execute at all.
        // For example if the entry trampoline called wasm which `return_call`'d
        // an imported function which is an exit trampoline, then
        // `fp == trampoline_fp` on the entry of this function, meaning the loop
        // won't actually execute anything.
        while fp != trampoline_fp {
            // At the start of each iteration of the loop, we know that `fp` is
            // a frame pointer from Wasm code. Therefore, we know it is not
            // being used as an extra general-purpose register, and it is safe
            // dereference to get the PC and the next older frame pointer.
            //
            // The stack also grows down, and therefore any frame pointer we are
            // dealing with should be less than the frame pointer on entry to
            // Wasm. Finally also assert that it's aligned correctly as an
            // additional sanity check.
            assert!(trampoline_fp > fp, "{trampoline_fp:#x} > {fp:#x}");
            arch::assert_fp_is_aligned(fp);

            tracing::trace!("--- Tracing through one Wasm frame ---");
            tracing::trace!("pc = {:p}", pc as *const ());
            tracing::trace!("fp = {:p}", fp as *const ());

            f(Frame { pc, fp })?;

            pc = arch::get_next_older_pc_from_fp(fp);

            // We rely on this offset being zero for all supported architectures
            // in `crates/cranelift/src/component/compiler.rs` when we set the
            // Wasm exit FP. If this ever changes, we will need to update that
            // code as well!
            assert_eq!(arch::NEXT_OLDER_FP_FROM_FP_OFFSET, 0);

            // Get the next older frame pointer from the current Wasm frame
            // pointer.
            let next_older_fp = *(fp as *mut usize).add(arch::NEXT_OLDER_FP_FROM_FP_OFFSET);

            // Because the stack always grows down, the older FP must be greater
            // than the current FP.
            assert!(next_older_fp > fp, "{next_older_fp:#x} > {fp:#x}");
            fp = next_older_fp;
        }

        tracing::trace!("=== Done tracing contiguous sequence of Wasm frames ===");
        ControlFlow::Continue(())
    }

    /// Iterate over the frames inside this backtrace.
    pub fn frames<'a>(
        &'a self,
    ) -> impl ExactSizeIterator<Item = &'a Frame> + DoubleEndedIterator + 'a {
        self.0.iter()
    }
}

/// A stack frame within a Wasm stack trace.
#[derive(Debug)]
pub struct Frame {
    pub pc: usize,
    pub fp: usize,
}