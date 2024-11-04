# k23-vm

Development repo of the k23 WASM VM

# Design

## Programs in k23

Programs in k23 are WASM [`Components`][component-model] which are essentially a collection of
dynamically linked WASM modules or components. You can think of this like one executable and potentially
many dynamic libraries packaged together.

WASM modules can be written in any language that supports WASM (like, C, Rust, Swift, Haskell and many more). These
modules interact through language-agnostic, high-level interfaces that describe available functions.
Components can import other components and can likewise be imported.

Consider the following simplified example (written in the low-level WASM text representation WAT), that just prints the
current time to STDOUT:

```wat
(component
    (import "wasi:clocks/wall-clock@0.2.2" (instance $time
        (export "now" (func ...))
    ))
    (import "wasi:cli/stdout@0.2.2" (component $stdout
        (export "get-stdout" (func ...))
    ))
    
    (func (export "print-time")
        ... transitively calls (func $time "now") to get the current time
        and (func $stdout "get-stdout") + "output-stream.write" to write to the STDOUT
    )
)
```

As you can see the program imports asks the OS to provide it with implementations
for the [`"wasi:clocks/wall-clock@0.2.2"`][wasi-clocks-wall-clock] and [`"wasi:cli/stdout@0.2.2"`][wasi-cli-stdout]
interfaces, from which it imports the `"now"` and `"get-stdout"` functions respectively.
It then exports a function called `"print_time"` that will use those imports to print the current
time to STDOUT.

Crucially programs don't need to care *how* these imports are actually fulfilled, the OS might have
builtin implementations, or fetch components from the network; As long as the API contract laid out by
the interface is upheld, the OS is free to choose the most optimal approach. You can think of this as
"typed dynamic linking" where you don't link against a specific library, but against an API contract.

This enables a number of cool things:

- **Dynamic Implementation Selection** - A program that depends on the `"wasi:filesystem` interface can work
  with **any** file system implementation, so the User or OS are free to most appropriate implementation (ext4, fat,
  etc.).
- **Language Independence** - Since the interface definitions are language-agnostic modules and components can be
  composed
  together regardless of the language they are written in. You can have C code calling Swift calling Haskell without any
  issues.

[//]: # (- **Bring only what you need** - Each Program comes with and explicit dependency tree)

## Microkernel with Dependency Management & Registry

k23 is naturally designed as a microkernel. This means the kernel itself is rather lightweight, it only contains
bootstrapping code, and a WASM Virtual Machine. Everything else is implemented as userspace programs, from drivers to
libraries and programs.

This microkernel architectures provides *much* greater security and stability since crashes or vulnerabilities in
one component remain contained to that component. Aside from that the kernel itself - being much smaller - becomes
easier to audit and test.

In addition to these benefits shared with all microkernels k23 has builtin *first class dependency management*.

Each program already explicitly declares its imports, and with these the OS will build a full dependency
tree that then gets fetched from a [*first party package registry*][wasm-warg]. Programs can import 

- a plain name that leaves it up to the developer to "read the docs" or otherwise figure out what to supply for the import;
- an interface name that is assumed to uniquely identify a higher-level semantic contract that the component is requesting an unspecified wasm or native implementation of;
- a URL name that the component is requesting be resolved to a particular wasm implementation by fetching the URL.
- a hash name containing a content-hash of the bytes of a particular wasm implemenentation but not specifying location of the bytes.
- a locked dependency name that the component is requesting be resolved via some contextually-supplied registry to a particular wasm implementation using the given hierarchical name and version; and
- an unlocked dependency name that the component is requesting be resolved via some contextually-supplied registry to one of a set of possible of wasm implementations using the given hierarchical name and version range.

## Microkernel Without the Performance Issues

## WASM VM Design

### Allocation

In WASM the instances are defined in the `Store` which owns
all related resources (memories, tables, globals etc.).
The WASM specification

### Execution

[component-model]: https://github.com/WebAssembly/component-model
[wasi-clocks-wall-clock]: https://github.com/WebAssembly/wasi-clocks/blob/110b161782f4900b188d326aeb303b211e4cd9e8/wit/wall-clock.wit#L17
[wasi-cli-stdout]: https://github.com/WebAssembly/wasi-cli/blob/0ed19accf7e9e677ad5911fc14cd1af7ceba1887/wit/stdio.wit#L11
[wasm-warg]: https://github.com/bytecodealliance/registry