use crate::indices::{DataIndex, ElemIndex, EntityIndex, FieldIndex, FuncIndex, FuncRefIndex, GlobalIndex, LabelIndex, LocalIndex, MemoryIndex, SharedOrModuleTypeIndex, TableIndex, TagIndex, TypeIndex};
use crate::translate::module_strings::{ModuleStrings};
use crate::translate::module_types::{ModuleTypes, ModuleTypesBuilder, WasmparserTypeConverter};
use crate::translate::{ConstExpr, EntityType, FunctionBodyData, FunctionType, GlobalType, Import, MemoryInitializer, MemoryPlan, ProducersLanguage, ProducersLanguageField, ProducersSdk, ProducersSdkField, ProducersTool, ProducersToolField, TableInitialValue, TablePlan, TableSegment, TableSegmentElements, Translation};
use crate::wasm_unsupported;
use alloc::sync::Arc;
use alloc::vec::Vec;
use cranelift_entity::packed_option::ReservedValue;
use hashbrown::HashMap;
use wasmparser::{
    BinaryReader, CustomSectionReader, DataKind, DataSectionReader, ElementItems, ElementKind,
    ElementSectionReader, ExportSectionReader, ExternalKind, FunctionSectionReader,
    GlobalSectionReader, ImportSectionReader, IndirectNameMap, MemorySectionReader, Name, NameMap,
    NameSectionReader, Parser, Payload, ProducersFieldValue, ProducersSectionReader, TableInit,
    TableSectionReader, TagSectionReader, TypeRef, TypeSectionReader, Validator, WasmFeatures,
};

/// A WebAssembly module translator.
pub struct ModuleTranslator<'a, 'data> {
    result: Translation<'data>,
    validator: &'a mut Validator,
    types: ModuleTypesBuilder,
    strings: ModuleStrings,
}

impl<'a, 'data> ModuleTranslator<'a, 'data> {
    pub fn new(validator: &'a mut Validator) -> Self {
        Self {
            strings: ModuleStrings::with_capacity(16),
            types: ModuleTypesBuilder::new(validator),
            validator,
            result: Translation::default(),
        }
    }

    /// Translate raw WASM bytes into a `Translation`.
    ///
    /// Returns the translation along with it's interned types and strings.
    pub fn translate(
        mut self,
        data: &'data [u8],
    ) -> crate::Result<(Translation<'data>, ModuleTypes, ModuleStrings)> {
        let mut parser = Parser::default();
        parser.set_features(*self.validator.features());

        for payload in parser.parse_all(data) {
            self.translate_payload(payload?)?;
        }

        self.validator.reset();
        Ok((self.result, self.types.finish(), self.strings))
    }

    /// Translates a single payload (essentially a section) of a WASM module.
    fn translate_payload(&mut self, payload: Payload<'data>) -> crate::Result<()> {
        match payload {
            Payload::Version {
                num,
                encoding,
                range,
            } => {
                self.validator.version(num, encoding, &range)?;
            }
            Payload::TypeSection(types) => {
                self.validator.type_section(&types)?;
                self.translate_type_section(types)?;
            }
            Payload::ImportSection(imports) => {
                self.validator.import_section(&imports)?;
                self.translate_import_section(imports)?;
            }
            Payload::FunctionSection(functions) => {
                self.validator.function_section(&functions)?;
                self.translate_function_section(functions)?;
            }
            Payload::TableSection(tables) => {
                self.validator.table_section(&tables)?;
                self.translate_table_section(tables)?;
            }
            Payload::MemorySection(memories) => {
                self.validator.memory_section(&memories)?;
                self.translate_memory_section(memories)?;
            }
            Payload::TagSection(tags) => {
                self.validator.tag_section(&tags)?;
                return Err(wasm_unsupported!("exception handling is unsupported"));
            }
            Payload::GlobalSection(globals) => {
                self.validator.global_section(&globals)?;
                self.translate_global_section(globals)?;
            }
            Payload::ExportSection(exports) => {
                self.validator.export_section(&exports)?;
                self.translate_export_section(exports)?;
            }
            Payload::StartSection { func, range } => {
                self.validator.start_section(func, &range)?;
                self.result.module.start = Some(FuncIndex::from_u32(func));
            }
            Payload::ElementSection(elements) => {
                self.validator.element_section(&elements)?;
                self.translate_element_section(elements)?;
            }
            Payload::DataCountSection { count, range } => {
                self.validator.data_count_section(count, &range)?;
            }
            Payload::DataSection(data) => {
                self.validator.data_section(&data)?;
                self.translate_data_section(data)?;
            }
            Payload::CodeSectionStart { count, range, .. } => {
                self.validator.code_section_start(count, &range)?;
                self.result.function_bodies.reserve_exact(count as usize);
                self.result.debug_info.code_section_offset = range.start as u64;
            }
            Payload::CodeSectionEntry(body) => {
                let validator = self.validator.code_section_entry(&body)?;
                self.result
                    .function_bodies
                    .push(FunctionBodyData { body, validator });
            }
            Payload::CustomSection(section) => match section.name() {
                "target_features" => self.parse_target_feature_section(&section),
                "name" => {
                    self.translate_name_section(NameSectionReader::new(BinaryReader::new(
                        section.data(),
                        section.data_offset(),
                    )))?;
                }
                "producers" => {
                    self.translate_producers_section(ProducersSectionReader::new(
                        BinaryReader::new_features(
                            section.data(),
                            section.data_offset(),
                            *self.validator.features(),
                        ),
                    )?)?;
                }
                name => {
                    tracing::trace!("custom section {name}");
                    if name.trim_end_matches(".dwo").starts_with(".debug_") {
                        self.translate_dwarf_section(name, &section);
                    } else {
                        tracing::warn!("unhandled custom section {section:?}");
                    }
                }
            },
            Payload::End(offset) => {
                self.validator.end(offset)?;
            }
            Payload::ModuleSection { .. }
            | Payload::InstanceSection(_)
            | Payload::CoreTypeSection(_)
            | Payload::ComponentSection { .. }
            | Payload::ComponentInstanceSection(_)
            | Payload::ComponentAliasSection(_)
            | Payload::ComponentTypeSection(_)
            | Payload::ComponentCanonicalSection(_)
            | Payload::ComponentStartSection { .. }
            | Payload::ComponentImportSection(_)
            | Payload::ComponentExportSection(_) => {
                return Err(wasm_unsupported!("component model is unsupported"));
            }
            p => tracing::warn!("unknown section {p:?}"),
        }
        Ok(())
    }

    fn flag_func_as_escaped(&mut self, func_index: FuncIndex) {
        let ty = &mut self.result.module.functions[func_index];
        if ty.is_escaping() {
            return;
        }
        let index = self.result.module.num_escaped_functions;
        ty.func_ref = FuncRefIndex::from_u32(index);
        self.result.module.num_escaped_functions += 1;
    }

    fn translate_type_section(&mut self, types: TypeSectionReader) -> crate::Result<()> {
        let count = types.count();
        self.result
            .module
            .types
            .reserve(usize::try_from(count).unwrap());

        let mut type_index = 0;
        for _ in 0..count {
            let validator_types = self.validator.types(0).unwrap();

            let core_type_id = validator_types.core_type_at_in_module(type_index);
            tracing::trace!(
                "about to intern rec group for {core_type_id:?} = {:?}",
                validator_types[core_type_id]
            );
            let rec_group_id = validator_types.rec_group_id_of(core_type_id);
            debug_assert_eq!(
                validator_types
                    .rec_group_elements(rec_group_id)
                    .position(|id| id == core_type_id),
                Some(0)
            );

            let interned =
                self.types
                    .intern_rec_group(&self.result.module, validator_types, rec_group_id)?;

            let elems = self.types.types.rec_group_elements(interned);
            let len = elems.len();
            self.result.module.types.reserve(len);
            for ty in elems {
                self.result.module.types.push(ty);
            }

            // Advance `type_index` to the start of the next rec group.
            type_index += u32::try_from(len).unwrap();
        }
        Ok(())
    }

    fn translate_import_section(
        &mut self,
        imports: ImportSectionReader<'data>,
    ) -> crate::Result<()> {
        self.result
            .module
            .imports
            .reserve_exact(imports.count() as usize);

        for import in imports {
            let import = import?;

            let index = match import.ty {
                TypeRef::Func(index) => {
                    let signature = TypeIndex::from_u32(index);
                    let interned_index = self.result.module.types[signature];
                    self.result.module.num_imported_functions += 1;
                    EntityType::Function(SharedOrModuleTypeIndex::Module(
                        interned_index,
                    ))
                }
                TypeRef::Table(ty) => {
                    self.result.module.num_imported_tables += 1;

                    let ty_convert =
                        WasmparserTypeConverter::new(&self.types.types, &self.result.module);

                    EntityType::Table(TablePlan::for_table(ty, &ty_convert))
                }
                TypeRef::Memory(ty) => {
                    self.result.module.num_imported_memories += 1;
                    EntityType::Memory(MemoryPlan::for_memory(ty))
                }
                TypeRef::Global(ty) => {
                    self.result.module.num_imported_globals += 1;

                    let ty_convert =
                        WasmparserTypeConverter::new(&self.types.types, &self.result.module);

                    EntityType::Global(GlobalType {
                        content_type: ty_convert.convert_val_type(&ty.content_type),
                        mutable: ty.mutable,
                        shared: ty.shared,
                    })
                }

                // doesn't get past validation
                TypeRef::Tag(_) => unreachable!(),
            };

            self.result.module.imports.push(Import {
                module: self.strings.intern(import.module),
                name: self.strings.intern(import.module),
                ty: index,
            });
        }

        Ok(())
    }

    fn translate_function_section(
        &mut self,
        functions: FunctionSectionReader<'data>,
    ) -> crate::Result<()> {
        self.result
            .module
            .functions
            .reserve_exact(functions.count() as usize);

        for index in functions {
            let signature = TypeIndex::from_u32(index?);
            self.result.module.functions.push(FunctionType {
                signature,
                func_ref: FuncRefIndex::reserved_value(),
            });
        }

        Ok(())
    }

    fn translate_table_section(&mut self, tables: TableSectionReader<'data>) -> crate::Result<()> {
        self.result
            .module
            .table_plans
            .reserve_exact(tables.count() as usize);
        self.result
            .module
            .table_initializers
            .initial_values
            .reserve_exact(tables.count() as usize);

        for table in tables {
            let table = table?;

            let ty_convert = WasmparserTypeConverter::new(&self.types.types, &self.result.module);

            let plan = TablePlan::for_table(table.ty, &ty_convert);
            self.result.module.table_plans.push(plan);

            let init = match table.init {
                TableInit::RefNull => TableInitialValue::RefNull,
                TableInit::Expr(expr) => {
                    let (expr, escaped) = ConstExpr::from_wasmparser(expr)?;
                    for func in escaped {
                        self.flag_func_as_escaped(func);
                    }
                    TableInitialValue::ConstExpr(expr)
                }
            };
            self.result
                .module
                .table_initializers
                .initial_values
                .push(init);
        }

        Ok(())
    }

    fn translate_memory_section(
        &mut self,
        memories: MemorySectionReader<'data>,
    ) -> crate::Result<()> {
        self.result
            .module
            .memory_plans
            .reserve_exact(memories.count() as usize);

        for ty in memories {
            self.result
                .module
                .memory_plans
                .push(MemoryPlan::for_memory(ty?));
        }

        Ok(())
    }

    fn parse_tag_section(&self, _tags: TagSectionReader<'data>) -> crate::Result<()> {
        Err(wasm_unsupported!("exception handling"))
    }

    fn translate_global_section(
        &mut self,
        globals: GlobalSectionReader<'data>,
    ) -> crate::Result<()> {
        self.result
            .module
            .globals
            .reserve_exact(globals.count() as usize);
        self.result
            .module
            .global_initializers
            .reserve_exact(globals.count() as usize);

        for global in globals {
            let global = global?;

            let ty_convert = WasmparserTypeConverter::new(&self.types.types, &self.result.module);

            self.result.module.globals.push(GlobalType {
                content_type: ty_convert.convert_val_type(&global.ty.content_type),
                mutable: global.ty.mutable,
                shared: global.ty.shared,
            });

            let (init_expr, escaped) = ConstExpr::from_wasmparser(global.init_expr)?;
            for func in escaped {
                self.flag_func_as_escaped(func);
            }
            self.result.module.global_initializers.push(init_expr);
        }

        Ok(())
    }

    fn translate_export_section(
        &mut self,
        exports: ExportSectionReader<'data>,
    ) -> crate::Result<()> {
        for export in exports {
            let export = export?;
            let index = match export.kind {
                ExternalKind::Func => {
                    let index = FuncIndex::from_u32(export.index);
                    self.flag_func_as_escaped(index);
                    self.result
                        .debug_info
                        .names
                        .func_names
                        .insert(index, export.name);
                    EntityIndex::Function(index)
                }
                ExternalKind::Table => {
                    let index = TableIndex::from_u32(export.index);
                    self.result
                        .debug_info
                        .names
                        .table_names
                        .insert(index, export.name);
                    EntityIndex::Table(index)
                }
                ExternalKind::Memory => {
                    let index = MemoryIndex::from_u32(export.index);
                    self.result
                        .debug_info
                        .names
                        .memory_names
                        .insert(index, export.name);
                    EntityIndex::Memory(index)
                }
                ExternalKind::Tag => {
                    let index = TagIndex::from_u32(export.index);
                    self.result
                        .debug_info
                        .names
                        .tag_names
                        .insert(index, export.name);
                    EntityIndex::Tag(index)
                }
                ExternalKind::Global => {
                    let index = GlobalIndex::from_u32(export.index);
                    self.result
                        .debug_info
                        .names
                        .global_names
                        .insert(index, export.name);
                    EntityIndex::Global(index)
                }
            };

            self.result
                .module
                .exports
                .insert(self.strings.intern(export.name), index);
        }

        Ok(())
    }

    fn translate_element_section(
        &mut self,
        elements: ElementSectionReader<'data>,
    ) -> crate::Result<()> {
        for (elem_index, element) in elements.into_iter().enumerate() {
            let element = element?;
            let elem_index = ElemIndex::from_u32(elem_index as u32);

            let elements = match element.items {
                ElementItems::Functions(funcs) => {
                    let mut out = Vec::with_capacity(funcs.count() as usize);
                    for func_idx in funcs {
                        out.push(FuncIndex::from_u32(func_idx?));
                    }
                    TableSegmentElements::Functions(out.into_boxed_slice())
                }
                ElementItems::Expressions(_, exprs) => {
                    let mut out = Vec::with_capacity(exprs.count() as usize);

                    for expr in exprs {
                        let (expr, escaped) = ConstExpr::from_wasmparser(expr?)?;
                        for func in escaped {
                            self.flag_func_as_escaped(func);
                        }
                        out.push(expr);
                    }
                    TableSegmentElements::Expressions(out.into_boxed_slice())
                }
            };

            match element.kind {
                ElementKind::Active {
                    table_index,
                    offset_expr,
                } => {
                    let table_index = TableIndex::from_u32(table_index.unwrap_or(0));
                    let (offset, escaped) = ConstExpr::from_wasmparser(offset_expr)?;
                    debug_assert!(escaped.is_empty());

                    self.result
                        .module
                        .table_initializers
                        .segments
                        .push(TableSegment {
                            table_index,
                            offset,
                            elements,
                        });
                    self.result
                        .module
                        .active_table_initializers
                        .insert(elem_index);
                }
                ElementKind::Passive => {
                    self.result
                        .module
                        .passive_table_initializers
                        .insert(elem_index, elements);
                }
                ElementKind::Declared => {}
            }
        }

        Ok(())
    }

    fn translate_data_section(&mut self, section: DataSectionReader<'data>) -> crate::Result<()> {
        for (data_index, entry) in section.into_iter().enumerate() {
            let entry = entry?;
            let data_index = DataIndex::from_u32(data_index as u32);

            let mk_range = |total: &mut u32| -> crate::Result<_> {
                let range = u32::try_from(entry.data.len())
                    .ok()
                    .and_then(|size| {
                        let start = *total;
                        let end = start.checked_add(size)?;
                        Some(start..end)
                    })
                    .ok_or_else(|| {
                        wasm_unsupported!("more than 4 gigabytes of data in wasm module")
                    })?;
                *total += range.end - range.start;
                Ok(range)
            };

            match entry.kind {
                DataKind::Active {
                    memory_index,
                    offset_expr,
                } => {
                    let memory_index = MemoryIndex::from_u32(memory_index);
                    let (offset, escaped) = ConstExpr::from_wasmparser(offset_expr)?;
                    debug_assert!(escaped.is_empty());

                    let range = mk_range(&mut self.result.total_data)?;

                    self.result
                        .module
                        .memory_initializers
                        .push(MemoryInitializer {
                            memory_index,
                            offset,
                            data: range,
                        });
                    self.result.data.push(entry.data.into());
                    self.result
                        .module
                        .active_memory_initializers
                        .insert(data_index);
                }
                DataKind::Passive => {
                    let range = mk_range(&mut self.result.total_passive_data)?;
                    self.result.passive_data.push(entry.data.into());
                    self.result
                        .module
                        .passive_memory_initializers
                        .insert(data_index, range);
                }
            }
        }

        Ok(())
    }

    fn parse_target_feature_section(&mut self, section: &CustomSectionReader<'data>) {
        let mut r = BinaryReader::new_features(
            section.data(),
            section.data_offset(),
            *self.validator.features(),
        );

        let _count = r.read_u8().unwrap();

        let mut required_features = WasmFeatures::empty();

        while !r.eof() {
            let prefix = r.read_u8().unwrap();
            assert_eq!(prefix, 0x2b, "only the `+` prefix is supported");

            let len = r.read_var_u64().unwrap();
            let feature = r.read_bytes(usize::try_from(len).unwrap()).unwrap();
            let feature = core::str::from_utf8(feature).unwrap();

            match feature {
                "atomics" => required_features.insert(WasmFeatures::THREADS),
                "bulk-memory" => required_features.insert(WasmFeatures::BULK_MEMORY),
                "exception-handling" => required_features.insert(WasmFeatures::EXCEPTIONS),
                "multivalue" => required_features.insert(WasmFeatures::MULTI_VALUE),
                "mutable-globals" => required_features.insert(WasmFeatures::MUTABLE_GLOBAL),
                "nontrapping-fptoint" => {
                    required_features.insert(WasmFeatures::SATURATING_FLOAT_TO_INT);
                }
                "sign-ext" => required_features.insert(WasmFeatures::SIGN_EXTENSION),
                "simd128" => required_features.insert(WasmFeatures::SIMD),
                "tail-call" => required_features.insert(WasmFeatures::TAIL_CALL),
                "reference-types" => required_features.insert(WasmFeatures::REFERENCE_TYPES),
                "gc" => required_features.insert(WasmFeatures::GC),
                "memory64" => required_features.insert(WasmFeatures::MEMORY64),
                "relaxed-simd" => required_features.insert(WasmFeatures::RELAXED_SIMD),
                "extended-const" => required_features.insert(WasmFeatures::EXTENDED_CONST),
                "multimemory" => required_features.insert(WasmFeatures::MULTI_MEMORY),
                "shared-everything" => {
                    required_features.insert(WasmFeatures::SHARED_EVERYTHING_THREADS)
                }
                _ => tracing::warn!("unknown required WASM feature `{feature}`"),
            }
        }

        self.result.required_features = required_features;
    }

    fn translate_name_section(&mut self, reader: NameSectionReader<'data>) -> crate::Result<()> {
        for subsection in reader {
            fn for_each_direct_name<'data>(
                names: NameMap<'data>,
                mut f: impl FnMut(u32, &'data str),
            ) -> crate::Result<()> {
                for name in names {
                    let name = name?;

                    f(name.index, name.name)
                }

                Ok(())
            }

            fn for_each_indirect_name<'data, I>(
                names: IndirectNameMap<'data>,
                mut f1: impl FnMut(&mut HashMap<I, &'data str>, u32, &'data str),
                mut f2: impl FnMut(HashMap<I, &'data str>, u32),
            ) -> crate::Result<()> {
                for naming in names {
                    let name = naming?;
                    let mut result = HashMap::default();

                    for name in name.names {
                        let name = name?;

                        f1(&mut result, name.index, name.name)
                    }

                    f2(result, name.index);
                }

                Ok(())
            }

            match subsection? {
                Name::Module { name, .. } => {
                    self.result.module.name = Some(self.strings.intern(name));
                }
                Name::Function(names) => {
                    for_each_direct_name(names, |idx, name| {
                        // Skip this naming if it's naming a function that
                        // doesn't actually exist.
                        if (idx as usize) < self.result.module.functions.len() {
                            self.result
                                .debug_info
                                .names
                                .func_names
                                .insert(FuncIndex::from_u32(idx), name);
                        }
                    })?;
                }
                Name::Local(names) => {
                    for_each_indirect_name(
                        names,
                        |result, idx, name| {
                            result.insert(LocalIndex::from_u32(idx), name);
                        },
                        |result, idx| {
                            // Skip this naming if it's naming a function that
                            // doesn't actually exist.
                            if (idx as usize) < self.result.module.functions.len() {
                                self.result
                                    .debug_info
                                    .names
                                    .locals_names
                                    .insert(FuncIndex::from_u32(idx), result);
                            }
                        },
                    )?;
                }
                Name::Global(names) => {
                    for_each_direct_name(names, |idx, name| {
                        self.result
                            .debug_info
                            .names
                            .global_names
                            .insert(GlobalIndex::from_u32(idx), name);
                    })?;
                }
                Name::Data(names) => {
                    for_each_direct_name(names, |idx, name| {
                        self.result
                            .debug_info
                            .names
                            .data_names
                            .insert(DataIndex::from_u32(idx), name);
                    })?;
                }
                Name::Type(names) => {
                    for_each_direct_name(names, |idx, name| {
                        self.result
                            .debug_info
                            .names
                            .type_names
                            .insert(TypeIndex::from_u32(idx), name);
                    })?;
                }
                Name::Label(names) => {
                    for_each_indirect_name(
                        names,
                        |result, idx, name| {
                            result.insert(LabelIndex::from_u32(idx), name);
                        },
                        |result, idx| {
                            // Skip this naming if it's naming a function that
                            // doesn't actually exist.
                            if (idx as usize) < self.result.module.functions.len() {
                                self.result
                                    .debug_info
                                    .names
                                    .labels_names
                                    .insert(FuncIndex::from_u32(idx), result);
                            }
                        },
                    )?;
                }
                Name::Table(names) => {
                    for_each_direct_name(names, |idx, name| {
                        self.result
                            .debug_info
                            .names
                            .table_names
                            .insert(TableIndex::from_u32(idx), name);
                    })?;
                }
                Name::Memory(names) => {
                    for_each_direct_name(names, |idx, name| {
                        self.result
                            .debug_info
                            .names
                            .memory_names
                            .insert(MemoryIndex::from_u32(idx), name);
                    })?;
                }
                Name::Element(names) => {
                    for_each_direct_name(names, |idx, name| {
                        self.result
                            .debug_info
                            .names
                            .element_names
                            .insert(ElemIndex::from_u32(idx), name);
                    })?;
                }
                Name::Field(names) => {
                    for_each_indirect_name(
                        names,
                        |result, idx, name| {
                            // Skip this naming if it's naming a function that
                            // doesn't actually exist.
                            if (idx as usize) < self.result.module.functions.len() {
                                result.insert(FieldIndex::from_u32(idx), name);
                            }
                        },
                        |result, idx| {
                            self.result
                                .debug_info
                                .names
                                .fields_names
                                .insert(FuncIndex::from_u32(idx), result);
                        },
                    )?;
                }
                Name::Tag(names) => {
                    for_each_direct_name(names, |idx, name| {
                        self.result
                            .debug_info
                            .names
                            .tag_names
                            .insert(TagIndex::from_u32(idx), name);
                    })?;
                }
                Name::Unknown { .. } => {}
            }
        }

        Ok(())
    }

    fn translate_producers_section(
        &mut self,
        section: ProducersSectionReader<'data>,
    ) -> crate::Result<()> {
        for field in section {
            let field = field?;
            match field.name {
                "language" => {
                    for value in field.values {
                        let ProducersFieldValue { name, version } = value?;
                        let name = match name {
                            "wat" => ProducersLanguage::Wat,
                            "C" => ProducersLanguage::C,
                            "C++" => ProducersLanguage::Cpp,
                            "Rust" => ProducersLanguage::Rust,
                            "JavaScript" => ProducersLanguage::JavaScript,
                            _ => ProducersLanguage::Other(name),
                        };

                        self.result
                            .debug_info
                            .producers
                            .language
                            .push(ProducersLanguageField { name, version });
                    }
                }
                "processed-by" => {
                    for value in field.values {
                        let ProducersFieldValue { name, version } = value?;
                        let name = match name {
                            "wabt" => ProducersTool::Wabt,
                            "LLVM" => ProducersTool::Llvm,
                            "clang" => ProducersTool::Clang,
                            "lld" => ProducersTool::Lld,
                            "Binaryen" => ProducersTool::Binaryen,
                            "rustc" => ProducersTool::Rustc,
                            "wasm-bindgen" => ProducersTool::WasmBindgen,
                            "wasm-pack" => ProducersTool::WasmPack,
                            "webassemblyjs" => ProducersTool::Webassemblyjs,
                            "wasm-snip" => ProducersTool::WasmSnip,
                            "Javy" => ProducersTool::Javy,
                            _ => ProducersTool::Other(name),
                        };

                        self.result
                            .debug_info
                            .producers
                            .processed_by
                            .push(ProducersToolField { name, version });
                    }
                }
                "sdk" => {
                    for value in field.values {
                        let ProducersFieldValue { name, version } = value?;
                        let name = match name {
                            "Emscripten" => ProducersSdk::Emscripten,
                            "Webpack" => ProducersSdk::Webpack,
                            _ => ProducersSdk::Other(name),
                        };

                        self.result
                            .debug_info
                            .producers
                            .sdk
                            .push(ProducersSdkField { name, version });
                    }
                }
                _ => unreachable!(),
            }
        }

        Ok(())
    }

    fn translate_dwarf_section(&mut self, name: &'data str, section: &CustomSectionReader<'data>) {
        let endian = gimli::LittleEndian;
        let data = section.data();
        let slice = gimli::EndianSlice::new(data, endian);

        let mut dwarf = gimli::Dwarf::default();
        let info = &mut self.result.debug_info;

        match name {
            // `gimli::Dwarf` fields.
            ".debug_abbrev" => dwarf.debug_abbrev = gimli::DebugAbbrev::new(data, endian),
            ".debug_addr" => dwarf.debug_addr = gimli::DebugAddr::from(slice),
            ".debug_info" => {
                dwarf.debug_info = gimli::DebugInfo::new(data, endian);
            }
            ".debug_line" => dwarf.debug_line = gimli::DebugLine::new(data, endian),
            ".debug_line_str" => dwarf.debug_line_str = gimli::DebugLineStr::from(slice),
            ".debug_str" => dwarf.debug_str = gimli::DebugStr::new(data, endian),
            ".debug_str_offsets" => dwarf.debug_str_offsets = gimli::DebugStrOffsets::from(slice),
            ".debug_str_sup" => {
                let dwarf_sup = gimli::Dwarf {
                    debug_str: gimli::DebugStr::from(slice),
                    ..Default::default()
                };
                dwarf.sup = Some(Arc::new(dwarf_sup));
            }
            ".debug_types" => dwarf.debug_types = gimli::DebugTypes::from(slice),

            // Additional fields.
            ".debug_loc" => info.debug_loc = gimli::DebugLoc::from(slice),
            ".debug_loclists" => info.debug_loclists = gimli::DebugLocLists::from(slice),
            ".debug_ranges" => info.debug_ranges = gimli::DebugRanges::new(data, endian),
            ".debug_rnglists" => info.debug_rnglists = gimli::DebugRngLists::new(data, endian),

            // DWARF package fields
            ".debug_cu_index" => info.debug_cu_index = gimli::DebugCuIndex::new(data, endian),
            ".debug_tu_index" => info.debug_tu_index = gimli::DebugTuIndex::new(data, endian),

            // We don't use these at the moment.
            ".debug_aranges" | ".debug_pubnames" | ".debug_pubtypes" => return,
            other => {
                tracing::warn!("unknown debug section `{}`", other);
                return;
            }
        }

        dwarf.ranges = gimli::RangeLists::new(info.debug_ranges, info.debug_rnglists);
        dwarf.locations = gimli::LocationLists::new(info.debug_loc, info.debug_loclists);

        info.dwarf = dwarf;
    }
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn strings() {
        let long = "iufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisfiufgliufglisfliasflisaufisufufsdizfizaswfizfsaizfizsfilzfisf";
        let (strings, a, b, c) = {
            let mut builder = ModuleStrings::with_capacity(2);
            let a = builder.intern("hello");
            let c = builder.intern(long);
            let b = builder.intern("world");
            (builder, a, b, c)
        };

        assert_eq!(&strings[a], "hello");
        assert_eq!(&strings[c], long);
        assert_eq!(&strings[b], "world");
    }
}