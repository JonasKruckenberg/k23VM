# k23VM

The VM design currently follows wasmtime's implementation pretty closely (it is the de-facto standard implementation for
WASM outside the browser and an awesome piece of software) with a few minor differences. It is expected that both will
diverge quite significantly though, as we tailor k23VM to the needs of the operating system.

## Running a WASM module

A WASM module arrives as bytes or in form of a WAT (WebAssembly Text Format) string. In either case it goes through a
number of phases before it is executed. These phases are documented below.

### 1. Parsing & Translation

The entrypoint of WASM into the runtime is through `Module::from_bytes` and `Module::from_str` for the binary and text
representation respectively. As the first step a `wasmparser::Parser` instance and a `ModuleTranslator` are created.
The `ModuleTranslator` drives the `wasmparser::Parser` and reads WASM data structures into a preallocated
`ModuleTranslation`.

This step also includes special handling of the WASM `types` section, which are ingested, deduplicated and translated
into
types for use with this crate. This step is known as type canonicalization (or more accurate module-level type
canonicalization)
and is required for the recursive types of the GC proposal. But even in absence of the GC proposal this helps with
producing nicer
types for the next steps.

NOTE: This phase does **not** parse the function bodies (the actual code) at all, they are lazily parsed in the
compilation phase.

> TODO
> - Support for streaming WASM to reduce time-to-first-instruction (in combination with tiering JIT?)

### 2. Compilation

In the compilation phase we turn the WASM bytecode into native machine code. This is done by closing over each function
body
in the WASM module, and constructing a `CompileInput` from it (essentially a boxed closure with access to the compiler).
This also includes necessary trampoline functions to convert between the WASM ABI and native ABIs.

The collected vector of `CompileInputs` is then fed through the `&dyn Compiler` which will produce a `CompiledFunction`
artifact
for each. These `CompiledFunction`s might have static relocations to builtins or other WASM functions in the module,
which will
be resolved now, before emitting all compiled code into an `Mmap` buffer.

This mmap'ed buffer is the marked as read-execute which concludes the compilation phase.

Note that during this phase we also collect metadata such as trap offsets & codes, source mappings and maybe DWARF debug
info.

> TODO
> - caching/serialization of artifacts & support for memory mapping a compiled module (produce an ELF object like
    wasmtime?)
> - tiering JIT: cheap tier0 for fast compilation & more expensive tier1 using cranelift for optimized code. Currently,
    > all modules are AOT compiled (ahead-of-time) using the optimizing cranelift compiler. This is fast enough for
    small programs
    > and serverless workloads, but not fast enough for the instantly-available-application, browser inspired UX of k23.
    > Adding a cheap tier0 compiler and only tiering up hot functions should dramatically increase
    time-to-first-instruction.
    > If necessary we might also introduce an interpreter as the tier0 (and bump the non-optimizing compiler to tier1
    and
    > cranelift to tier2). This would help with large and sparse modules that have many functions most of which are
    called very
    > infrequently.

### 3. Allocation

A WASM instance consists of a number of resources that take up space in virtual memory: Linear memories, tables, but
also
compiled functions and the `VMContext` runtime metadata.
During the allocation phase the compiled code and `ModuleTranslation` metadata are used to reserve memory for all
declared resources. This allocation might use one of several strategies, but by default it will use one similar to the
`InstanceAllocationStrategy::Pooling` in wasmtime where the full available address space is subdivided into *Slots* for
instances that can then cheaply be allocated from through simple bump allocation.

Note that k23 has quite different requirements for this allocation scheme than hosted runtimes such as wasmtime:
Reserving large amounts of virtual or even physical memory isn't a big deal as long as we can provide programs with the
resources they request. In fact, an OS should use as much physical memory as possible for things like caches that help
speed up execution. You paid for the RAM after all, so your OS might as well make use of it. Importantly though, program
usage of memory takes priority, so the OS should evict caches of required.

> TODO
> - Should implement a "Pooling" and "On Demand" (or "regular") strategy where "On Demand"
> - Maybe: Use the kernel address space half too (gives us almost double the virtual memory).
> - Implement lazy-committing of physical memory where we reserve *virtual* memory but never allocate *physical* memory
    to
    > back it up until the program accesses the page.
> - Implement swap where we can swap pages of memory to disk to "increase the amount of physical memory".
> - Implement CoW (copy on write). If a module gets allocated and a snapshot of the instances data already exists on
    disk
    > (through swap, explicit snapshotting or eager initialization) use map that snapshot into memory instead of
    allocating
    > and initializing it from scratch. The pages will be marked as *read-only* and remapped using a copy if written to
    > (the copy on write part). This reduces the amount of physical memory consumed especially when the same module
    > gets instantiated many times.

### 4. Instantiation

During instantiation, the previously allocated regions of memory get initialized into a ready state. This means filling
the `VMContext` fields with the correct pointers to entities, filling linear memories using the WASM `data` segments or
filling tables using the `elem` segments if present.

This step will also attempt to resolve imports. All imports consisting of a `module` and `field` name-pair
and their associated type (function, table, memory, or global) of a module are gathered and looked up in the
`Linker`. A `Linker` is just a mapping from these name pairs to pointers into registered instances' exports.
With all imports successfully resolved, their pointers get copied into the `VMContext` structure to be accessed by JIT
code
as needed.

At the end the `start` function of a module should be called if present to perform module-specific initialization.

During this phase the `VMcontext`s `func_refs` array gets filled with pointers to WASM functions (more accurately their
associated trampolines) which will be used during execution to find function addresses.

> TODO
> - like allocation this should be optimized using memory-mapping or memcpy-ing in many situations
> - Allow different protocols for resolving names in the linker. Currently, the `Linker` only knows about instances and
    > their exports if they have been registered ahead of time. For the final vision we need dynamic resolution of names
    though.
    > To this end the linker should support the following subset of *WASM Component Model Import Specifiers*:
    >
- **interface names** that request semantic contract without specifying an exact implementation (e.g. used for drivers).
>   - **locked dependency name** that requests a particular WASM module from the builtin registry using a hierarchical
      name and version.
>   - **unlocked dependency name** that requests one of a set of possible WASM modules from the builtin registry using a
      hierarchical name and version *range*.
      > In addition to 3rd party resolver drivers might define resolutions for:
>   - **URL names** that request a WASM module be fetched from the given URL
>   - **hash names** that request the content-hash of a particular WASM module without a specific location, useful maybe
      for
      > distributing modules through content-addressing systems such as bittorrent or IPFS.
>
> Open questions
> - We probably want 3rd part resolvers to act as "fallback resolvers" for imports unknown to the main one.
    > But should 3rd party resolvers be allowed to override builtin resolvers? In what order should they be tried?
> - What should the resolution order look like in general (builtin, local cache, registry?)
> - How should the linker handle conflicting resolutions e.g. multiple implementations of the same interface?

### 5. Execution

With all the previous steps completed we have a fully ready `Instance` on our hands. This instance can be queried for
its
exports. As the host k23 has additional control over instances exports, it can read & write data and manipulate
resources
such as growing linear memories etc.

The most import export is the `Func` representing an exported WASM function that can be called using the `call` method.
Calling an exported function requires providing arguments and a slice to put results into. It will transfer control to
the JIT compiled code, run that to completion and return control to Rust code after. Special handling is required for
traps and exceptions (see the next section below).

> TODO
> - Implement a `TypedFunc` system similar to wasmtime to reduce cost of function calls (allowing us to sidestep the
    array-call
    > trampoline and frontload the signature check)

#### Handling Traps

For performance reasons all traps use hardware traps. Explicit traps or checks translate to jumps to invalid
instructions,
out-of-bounds accesses are caught by unmapped guard pages. At the end all traps end up in the hosts trap handler
function,
which will determine cause and faulting instance, then long jump back to the `call` method invocation that caused the
WASM
execution. Here the trap info will be translated into a nice Rust error, and it is up to the calling host code to
determine
what to do. Most likely the instance will be killed and deallocated.

#### Builtins

Some WASM operations are too complex to be implemented in JIT code, or are so common that sharing them globally would
be beneficial. Examples of this are `memory.init` or `memcpy`. Additionally, some WASM operations require "privileged"
access to the instance e.g. `memory.grow`. The third and least common category of builtins are polyfills e.g. for
floating
point operations (allowing k23 to operate on machines without a native floating point unit).

All these are translated into calls to so-called builtins. These are `extern "C"` Rust functions with a particular ABI
that can be called directly from JIT code.

> TODO
> - The builtin functions should be placed into a custom `.wasm-builtins` elf section so we can map them into userspace.
> - For programs that require privileged access to the instance we might need a syscall-like jump to kernel-space (if we
    > decide to use different privilege levels). Figure out how to do that in cranelift (resumable traps?).

#### Preemptive vs Cooperative multitasking

Currently, we can only execute one program at a time which is obviously bad. But a major open question is whether
to use the traditional preemptive multitasking approach, where a hardware timer periodically claws control from the
userspace program and returns it to the OS, or if we should use a cooperative multitasking approach where programs
voluntarily give up control periodically.

##### Preemptive Scheduling

- **Advantages**
    - Tight control over the time slice allocated
    - Programs are guaranteed to yield in a fixed amount of time
- **Disadvantages**
    - Requires a full context switch and saving of *all* registers which is slow

##### Cooperative Scheduling

- **Advantages**
    - More efficient as programs are responsible for saving their state before yielding
    - Integrates well will Rust async
- **Disadvantages**
    - Requires maintaining an epoch counter (and updating that periodically which requires incrementing the atomic
      counter in the trap handler. Ideally using only assembly in a special trap-vector snippet)
      AND check this counter in each function prologue & loop header
    - OR keep a precise fuel counter
    - Both options *might* be slower in the long run that the occasional context switch
    
In conclusion, there are benefits and issues with both approaches, so we might need to implement both and measure 
to make a proper decision. On paper, I quite like the elegance of the Rust async-only design cooperative scheduling allows.
But performance needs to be prioritized.