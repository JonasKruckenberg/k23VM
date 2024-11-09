# k23VM

## Design

The VM design currently follows wasmtime's implementation pretty closely (it is the de-facto standard implementation for
WASM outside the browser and an awesome piece of software) with a few minor differences. It is expected that both will
diverge quite significantly though, as we tailor k23VM to the needs of the operating system.

TODO explain

## Parsing & Translation

WASM binaries are parsed using the `wasmparser` crate, and translated into data structures useful to the VM. This
includes
translating memory types present in the binary into `MemoryDesc` instances that encode k23-specific additional data.
This also means type canonicalization for wasm GC types.

This step produces 2 kinds of output: Module information that is kept around for the whole lifetime of instances created
from the WASM, these are `TranslatedModule` and `ModuleTypes` and information that is only used during compilation and
discarded afterward such as function bodies and debugging information.

## Compilation

## Allocation

