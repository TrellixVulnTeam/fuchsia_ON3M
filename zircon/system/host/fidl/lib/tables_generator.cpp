// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "fidl/tables_generator.h"

#include "fidl/names.h"

namespace fidl {

namespace {

constexpr auto kIndent = "    ";

std::ostream& operator<<(std::ostream& stream, StringView view) {
    stream.rdbuf()->sputn(view.data(), view.size());
    return stream;
}

void Emit(std::ostream* file, StringView data) {
    *file << data;
}

void EmitNewlineAndIndent(std::ostream* file, size_t indent_level) {
    *file << "\n";
    while (indent_level--)
        *file << kIndent;
}

void EmitArrayBegin(std::ostream* file) {
    *file << "{";
}

void EmitArraySeparator(std::ostream* file, size_t indent_level) {
    *file << ",";
    EmitNewlineAndIndent(file, indent_level);
}

void EmitArrayEnd(std::ostream* file) {
    *file << "}";
}

void Emit(std::ostream* file, uint32_t value) {
    *file << value;
}

void Emit(std::ostream* file, types::HandleSubtype handle_subtype) {
    Emit(file, NameHandleZXObjType(handle_subtype));
}

void Emit(std::ostream* file, types::Nullability nullability) {
    switch (nullability) {
    case types::Nullability::kNullable:
        Emit(file, "::fidl::kNullable");
        break;
    case types::Nullability::kNonnullable:
        Emit(file, "::fidl::kNonnullable");
        break;
    }
}

} // namespace

void TablesGenerator::GenerateInclude(StringView filename) {
    Emit(&tables_file_, "#include ");
    Emit(&tables_file_, filename);
    Emit(&tables_file_, "\n");
}

void TablesGenerator::GenerateFilePreamble() {
    Emit(&tables_file_, "// WARNING: This file is machine generated by fidlc.\n\n");
    GenerateInclude("<lib/fidl/internal.h>");
    Emit(&tables_file_, "\nextern \"C\" {\n");
    Emit(&tables_file_, "\n");
}

void TablesGenerator::GenerateFilePostamble() {
    Emit(&tables_file_, "} // extern \"C\"\n");
}

template <typename Collection>
void TablesGenerator::GenerateArray(const Collection& collection) {
    EmitArrayBegin(&tables_file_);

    if (!collection.empty())
        EmitNewlineAndIndent(&tables_file_, ++indent_level_);

    for (size_t i = 0; i < collection.size(); ++i) {
        if (i)
            EmitArraySeparator(&tables_file_, indent_level_);
        Generate(collection[i]);
    }

    if (!collection.empty())
        EmitNewlineAndIndent(&tables_file_, --indent_level_);

    EmitArrayEnd(&tables_file_);
}

void TablesGenerator::Generate(const coded::StructType& struct_type) {
    Emit(&tables_file_, "static const ::fidl::FidlStructField ");
    Emit(&tables_file_, NameFields(struct_type.coded_name));
    Emit(&tables_file_, "[] = ");
    GenerateArray(struct_type.fields);
    Emit(&tables_file_, ";\n");

    Emit(&tables_file_, "const fidl_type_t ");
    Emit(&tables_file_, NameTable(struct_type.coded_name));
    Emit(&tables_file_, " = fidl_type_t(::fidl::FidlCodedStruct(");
    Emit(&tables_file_, NameFields(struct_type.coded_name));
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, static_cast<uint32_t>(struct_type.fields.size()));
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, struct_type.size);
    Emit(&tables_file_, ", \"");
    Emit(&tables_file_, struct_type.qname);
    Emit(&tables_file_, "\"));\n\n");
}

void TablesGenerator::Generate(const coded::TableType& table_type) {
    Emit(&tables_file_, "static const ::fidl::FidlTableField ");
    Emit(&tables_file_, NameFields(table_type.coded_name));
    Emit(&tables_file_, "[] = ");
    GenerateArray(table_type.fields);
    Emit(&tables_file_, ";\n");

    Emit(&tables_file_, "const fidl_type_t ");
    Emit(&tables_file_, NameTable(table_type.coded_name));
    Emit(&tables_file_, " = fidl_type_t(::fidl::FidlCodedTable(");
    Emit(&tables_file_, NameFields(table_type.coded_name));
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, static_cast<uint32_t>(table_type.fields.size()));
    Emit(&tables_file_, ", \"");
    Emit(&tables_file_, table_type.qname);
    Emit(&tables_file_, "\"));\n\n");
}

void TablesGenerator::Generate(const coded::UnionType& union_type) {
    Emit(&tables_file_, "static const fidl_type_t* ");
    Emit(&tables_file_, NameMembers(union_type.coded_name));
    Emit(&tables_file_, "[] = ");
    GenerateArray(union_type.types);
    Emit(&tables_file_, ";\n");

    Emit(&tables_file_, "const fidl_type_t ");
    Emit(&tables_file_, NameTable(union_type.coded_name));
    Emit(&tables_file_, " = fidl_type_t(::fidl::FidlCodedUnion(");
    Emit(&tables_file_, NameMembers(union_type.coded_name));
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, static_cast<uint32_t>(union_type.types.size()));
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, union_type.data_offset);
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, union_type.size);
    Emit(&tables_file_, ", \"");
    Emit(&tables_file_, union_type.qname);
    Emit(&tables_file_, "\"));\n\n");
}

void TablesGenerator::Generate(const coded::XUnionType& xunion_type) {
    Emit(&tables_file_, "static const ::fidl::FidlXUnionField ");
    Emit(&tables_file_, NameFields(xunion_type.coded_name));
    Emit(&tables_file_, "[] = ");
    GenerateArray(xunion_type.fields);
    Emit(&tables_file_, ";\n");

    Emit(&tables_file_, "const fidl_type_t ");
    Emit(&tables_file_, NameTable(xunion_type.coded_name));
    Emit(&tables_file_, " = fidl_type_t(::fidl::FidlCodedXUnion(");
    Emit(&tables_file_, static_cast<uint32_t>(xunion_type.fields.size()));
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, NameFields(xunion_type.coded_name));
    Emit(&tables_file_, ", \"");
    Emit(&tables_file_, xunion_type.qname);
    Emit(&tables_file_, "\"));\n\n");
}

void TablesGenerator::Generate(const coded::MessageType& message_type) {
    Emit(&tables_file_, "extern const fidl_type_t ");
    Emit(&tables_file_, NameTable(message_type.coded_name));
    Emit(&tables_file_, ";\n");

    Emit(&tables_file_, "static const ::fidl::FidlStructField ");
    Emit(&tables_file_, NameFields(message_type.coded_name));
    Emit(&tables_file_, "[] = ");
    GenerateArray(message_type.fields);
    Emit(&tables_file_, ";\n");

    Emit(&tables_file_, "const fidl_type_t ");
    Emit(&tables_file_, NameTable(message_type.coded_name));
    Emit(&tables_file_, " = fidl_type_t(::fidl::FidlCodedStruct(");
    Emit(&tables_file_, NameFields(message_type.coded_name));
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, static_cast<uint32_t>(message_type.fields.size()));
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, message_type.size);
    Emit(&tables_file_, ", \"");
    Emit(&tables_file_, message_type.qname);
    Emit(&tables_file_, "\"));\n\n");
}

void TablesGenerator::Generate(const coded::HandleType& handle_type) {
    Emit(&tables_file_, "static const fidl_type_t ");
    Emit(&tables_file_, NameTable(handle_type.coded_name));
    Emit(&tables_file_, " = fidl_type_t(::fidl::FidlCodedHandle(");
    Emit(&tables_file_, handle_type.subtype);
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, handle_type.nullability);
    Emit(&tables_file_, "));\n\n");
}

void TablesGenerator::Generate(const coded::RequestHandleType& request_type) {
    Emit(&tables_file_, "static const fidl_type_t ");
    Emit(&tables_file_, NameTable(request_type.coded_name));
    Emit(&tables_file_, " = fidl_type_t(::fidl::FidlCodedHandle(");
    Emit(&tables_file_, types::HandleSubtype::kChannel);
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, request_type.nullability);
    Emit(&tables_file_, "));\n\n");
}

void TablesGenerator::Generate(const coded::InterfaceHandleType& interface_type) {
    Emit(&tables_file_, "static const fidl_type_t ");
    Emit(&tables_file_, NameTable(interface_type.coded_name));
    Emit(&tables_file_, " = fidl_type_t(::fidl::FidlCodedHandle(");
    Emit(&tables_file_, types::HandleSubtype::kChannel);
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, interface_type.nullability);
    Emit(&tables_file_, "));\n\n");
}

void TablesGenerator::Generate(const coded::ArrayType& array_type) {
    Emit(&tables_file_, "static const fidl_type_t ");
    Emit(&tables_file_, NameTable(array_type.coded_name));
    Emit(&tables_file_, " = fidl_type_t(::fidl::FidlCodedArray(&");
    Emit(&tables_file_, NameTable(array_type.element_type->coded_name));
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, array_type.size);
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, array_type.element_size);
    Emit(&tables_file_, "));\n\n");
}

void TablesGenerator::Generate(const coded::StringType& string_type) {
    Emit(&tables_file_, "static const fidl_type_t ");
    Emit(&tables_file_, NameTable(string_type.coded_name));
    Emit(&tables_file_, " = fidl_type_t(::fidl::FidlCodedString(");
    Emit(&tables_file_, string_type.max_size);
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, string_type.nullability);
    Emit(&tables_file_, "));\n\n");
}

void TablesGenerator::Generate(const coded::VectorType& vector_type) {
    Emit(&tables_file_, "static const fidl_type_t ");
    Emit(&tables_file_, NameTable(vector_type.coded_name));
    Emit(&tables_file_, " = fidl_type_t(::fidl::FidlCodedVector(");
    if (vector_type.element_type->coding_needed == coded::CodingNeeded::kNeeded) {
        Emit(&tables_file_, "&");
        Emit(&tables_file_, NameTable(vector_type.element_type->coded_name));
    } else {
        Emit(&tables_file_, "nullptr");
    }
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, vector_type.max_count);
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, vector_type.element_size);
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, vector_type.nullability);
    Emit(&tables_file_, "));\n\n");
}

void TablesGenerator::Generate(const coded::Type* type) {
    if (type) {
        Emit(&tables_file_, "&");
        Emit(&tables_file_, NameTable(type->coded_name));
    } else {
        Emit(&tables_file_, "nullptr");
    }
}

void TablesGenerator::Generate(const coded::StructField& field) {
    Emit(&tables_file_, "::fidl::FidlStructField(&");
    Emit(&tables_file_, NameTable(field.type->coded_name));
    Emit(&tables_file_, ", ");
    Emit(&tables_file_, field.offset);
    Emit(&tables_file_, ")");
}

void TablesGenerator::Generate(const coded::TableField& field) {
    Emit(&tables_file_, "::fidl::FidlTableField(&");
    Emit(&tables_file_, NameTable(field.type->coded_name));
    Emit(&tables_file_, ",");
    Emit(&tables_file_, field.ordinal);
    Emit(&tables_file_, ")");
}

void TablesGenerator::Generate(const coded::XUnionField& field) {
    Emit(&tables_file_, "::fidl::FidlXUnionField(&");
    Emit(&tables_file_, NameTable(field.type->coded_name));
    Emit(&tables_file_, ",");
    Emit(&tables_file_, field.ordinal);
    Emit(&tables_file_, ")");
}

void TablesGenerator::GeneratePointerIfNeeded(const coded::StructType& struct_type) {
    if (struct_type.referenced_by_pointer) {
        Emit(&tables_file_, "static const fidl_type_t ");
        Emit(&tables_file_, NameTable(struct_type.pointer_name));
        Emit(&tables_file_, " = fidl_type_t(::fidl::FidlCodedStructPointer(&");
        Emit(&tables_file_, NameTable(struct_type.coded_name));
        Emit(&tables_file_, ".coded_struct));\n");
    }
}

void TablesGenerator::GeneratePointerIfNeeded(const coded::TableType& table_type) {
    if (table_type.referenced_by_pointer) {
        Emit(&tables_file_, "static const fidl_type_t ");
        Emit(&tables_file_, NameTable(table_type.pointer_name));
        Emit(&tables_file_, " = fidl_type_t(::fidl::FidlCodedTablePointer(&");
        Emit(&tables_file_, NameTable(table_type.coded_name));
        Emit(&tables_file_, ".coded_table));\n");
    }
}

void TablesGenerator::GeneratePointerIfNeeded(const coded::UnionType& union_type) {
    if (union_type.referenced_by_pointer) {
        Emit(&tables_file_, "static const fidl_type_t ");
        Emit(&tables_file_, NameTable(union_type.pointer_name));
        Emit(&tables_file_, " = fidl_type_t(::fidl::FidlCodedUnionPointer(&");
        Emit(&tables_file_, NameTable(union_type.coded_name));
        Emit(&tables_file_, ".coded_union));\n");
    }
}

void TablesGenerator::GenerateForward(const coded::StructType& struct_type) {
    Emit(&tables_file_, "extern const fidl_type_t ");
    Emit(&tables_file_, NameTable(struct_type.coded_name));
    Emit(&tables_file_, ";\n");
}

void TablesGenerator::GenerateForward(const coded::TableType& table_type) {
    Emit(&tables_file_, "extern const fidl_type_t ");
    Emit(&tables_file_, NameTable(table_type.coded_name));
    Emit(&tables_file_, ";\n");
}

void TablesGenerator::GenerateForward(const coded::UnionType& union_type) {
    Emit(&tables_file_, "extern const fidl_type_t ");
    Emit(&tables_file_, NameTable(union_type.coded_name));
    Emit(&tables_file_, ";\n");
}

void TablesGenerator::GenerateForward(const coded::XUnionType& xunion_type) {
    Emit(&tables_file_, "extern const fidl_type_t ");
    Emit(&tables_file_, NameTable(xunion_type.coded_name));
    Emit(&tables_file_, ";\n");
}

std::ostringstream TablesGenerator::Produce() {
    coded_types_generator_.CompileCodedTypes();

    GenerateFilePreamble();

    for (const auto& decl : coded_types_generator_.library()->declaration_order_) {
        auto coded_type = coded_types_generator_.CodedTypeFor(&decl->name);
        if (!coded_type)
            continue;
        switch (coded_type->kind) {
        case coded::Type::Kind::kStruct:
            GenerateForward(*static_cast<const coded::StructType*>(coded_type));
            break;
        case coded::Type::Kind::kTable:
            GenerateForward(*static_cast<const coded::TableType*>(coded_type));
            break;
        case coded::Type::Kind::kUnion:
            GenerateForward(*static_cast<const coded::UnionType*>(coded_type));
            break;
        case coded::Type::Kind::kXUnion:
            GenerateForward(*static_cast<const coded::XUnionType*>(coded_type));
            break;
        default:
            break;
        }
    }

    Emit(&tables_file_, "\n");

    for (const auto& decl : coded_types_generator_.library()->declaration_order_) {
        auto coded_type = coded_types_generator_.CodedTypeFor(&decl->name);
        if (!coded_type)
            continue;
        switch (coded_type->kind) {
        case coded::Type::Kind::kStruct:
            GeneratePointerIfNeeded(*static_cast<const coded::StructType*>(coded_type));
            break;
        case coded::Type::Kind::kTable:
            GeneratePointerIfNeeded(*static_cast<const coded::TableType*>(coded_type));
            break;
        case coded::Type::Kind::kUnion:
            GeneratePointerIfNeeded(*static_cast<const coded::UnionType*>(coded_type));
            break;
        case coded::Type::Kind::kXUnion:
            // Do nothing here: there's no need to generate a XUnionPointer,
            // since nullable XUnions (for which we'd normally use a
            // XUnionPointer) have the same encoding representation as
            // non-optional XUnions.
            break;
        default:
            break;
        }
    }

    Emit(&tables_file_, "\n");

    for (const auto& coded_type : coded_types_generator_.coded_types()) {
        if (coded_type->coding_needed == coded::CodingNeeded::kNotNeeded)
            continue;

        switch (coded_type->kind) {
        case coded::Type::Kind::kStruct:
        case coded::Type::Kind::kStructPointer:
        case coded::Type::Kind::kTable:
        case coded::Type::Kind::kTablePointer:
        case coded::Type::Kind::kUnion:
        case coded::Type::Kind::kUnionPointer:
        case coded::Type::Kind::kXUnion:
            // These are generated in the next phase.
            break;
        case coded::Type::Kind::kInterface:
            // Nothing to generate for interfaces. We've already moved the
            // messages from the interface into coded_types_ directly.
            break;
        case coded::Type::Kind::kMessage:
            Generate(*static_cast<const coded::MessageType*>(coded_type.get()));
            break;
        case coded::Type::Kind::kHandle:
            Generate(*static_cast<const coded::HandleType*>(coded_type.get()));
            break;
        case coded::Type::Kind::kInterfaceHandle:
            Generate(*static_cast<const coded::InterfaceHandleType*>(coded_type.get()));
            break;
        case coded::Type::Kind::kRequestHandle:
            Generate(*static_cast<const coded::RequestHandleType*>(coded_type.get()));
            break;
        case coded::Type::Kind::kArray:
            Generate(*static_cast<const coded::ArrayType*>(coded_type.get()));
            break;
        case coded::Type::Kind::kString:
            Generate(*static_cast<const coded::StringType*>(coded_type.get()));
            break;
        case coded::Type::Kind::kVector:
            Generate(*static_cast<const coded::VectorType*>(coded_type.get()));
            break;
        case coded::Type::Kind::kPrimitive:
            // These are only around to provide size information to
            // vectors. There's never anything to generate, and this
            // should not be reached.
            assert(false && "Primitive types should never need coding tables");
            break;
        }
    }

    for (const auto& decl : coded_types_generator_.library()->declaration_order_) {
        if (decl->name.library() != coded_types_generator_.library())
            continue;

        auto coded_type = coded_types_generator_.CodedTypeFor(&decl->name);
        if (!coded_type)
            continue;
        switch (coded_type->kind) {
        case coded::Type::Kind::kStruct:
            Generate(*static_cast<const coded::StructType*>(coded_type));
            break;
        case coded::Type::Kind::kTable:
            Generate(*static_cast<const coded::TableType*>(coded_type));
            break;
        case coded::Type::Kind::kUnion:
            Generate(*static_cast<const coded::UnionType*>(coded_type));
            break;
        case coded::Type::Kind::kXUnion:
            Generate(*static_cast<const coded::XUnionType*>(coded_type));
            break;
        default:
            continue;
        }
    }

    GenerateFilePostamble();

    return std::move(tables_file_);
}

} // namespace fidl
