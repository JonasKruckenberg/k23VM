use criterion::{black_box, criterion_group, criterion_main, Criterion};
use k23_vm::{Engine, Module};
use wasmparser::Validator;

fn compile(bytes: &[u8]) {
    let engine = Engine::default();
    let mut validator = Validator::new();

    let module = Module::from_bytes(&engine, &mut validator, bytes).unwrap();
    black_box(module);
}

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Translate");
    group.bench_function("compile fib_cpp", |b| {
        b.iter(|| compile(black_box(include_bytes!("../tests/fib_cpp.wasm"))))
    });
    // group.bench_function("translate kiwi-editor", |b| {
    //     b.iter(|| compile(black_box(include_bytes!("../tests/kiwi-editor.wasm"))))
    // });
    group.bench_function("compile embenchen_fannkuch", |b| {
        b.iter(|| {
            compile(black_box(include_bytes!(
                "../tests/embenchen_fannkuch.wasm"
            )))
        })
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
