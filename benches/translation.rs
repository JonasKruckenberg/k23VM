use criterion::{black_box, criterion_group, criterion_main, Criterion};
use k23_vm::{Engine, ModuleTranslator};
use wasmparser::Validator;

fn translate(bytes: &[u8]) {
    let engine = Engine::default();
    let mut validator = Validator::new();

    let (translation, types) = ModuleTranslator::new(&mut validator)
        .translate(bytes)
        .unwrap();

    let type_collection = engine.type_registry().register_module_types(&engine, types);

    black_box((translation, type_collection));
}

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Translate");
    group.bench_function("translate fib_cpp", |b| {
        b.iter(|| translate(black_box(include_bytes!("../tests/fib_cpp.wasm"))))
    });
    group.bench_function("translate kiwi-editor", |b| {
        b.iter(|| translate(black_box(include_bytes!("../tests/kiwi-editor.wasm"))))
    });
    group.bench_function("translate embenchen_fannkuch", |b| {
        b.iter(|| {
            translate(black_box(include_bytes!(
                "../tests/embenchen_fannkuch.wasm"
            )))
        })
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
