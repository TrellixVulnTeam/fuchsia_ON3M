// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_INCLUDE_FIDL_JSON_GENERATOR_H_
#define TOOLS_FIDL_FIDLC_INCLUDE_FIDL_JSON_GENERATOR_H_

#include <memory>
#include <sstream>
#include <string>
#include <string_view>
#include <vector>

#include "flat_ast.h"
#include "json_writer.h"

namespace fidl {

struct NameSpan {
  explicit NameSpan(const SourceSpan& span)
      : filename(span.source_file().filename()), length(span.data().length()) {
    span.SourceLine(&position);
  }

  // TODO(fxbug.dev/7920): We are incorrectly assuming that the provided name is not
  // anonymous, and relying on callers to avoid derefencing a nullptr
  // location.
  explicit NameSpan(const flat::Name& name) : NameSpan(name.span().value()) {
    assert(name.span().has_value() && "NameSpan was passed an anonymous name");
  }

  const std::string filename;
  SourceFile::Position position;
  const size_t length;
};

// Methods or functions named "Emit..." are the actual interface to
// the JSON output.

// Methods named "Generate..." directly generate JSON output via the
// "Emit" routines.

// Methods named "Produce..." indirectly generate JSON output by calling
// the Generate methods, and should not call the "Emit" functions
// directly.

// |JsonWriter| requires the derived type as a template parameter so it can
// match methods declared with parameter overrides in the derived class.
class JSONGenerator : public utils::JsonWriter<JSONGenerator> {
 public:
  // "using" is required for overridden methods, so the implementations in
  // both the base class and in this derived class are visible when matching
  // parameter types
  using utils::JsonWriter<JSONGenerator>::Generate;
  using utils::JsonWriter<JSONGenerator>::GenerateArray;

  explicit JSONGenerator(const flat::Library* library)
      : JsonWriter(json_file_), library_(library) {}

  ~JSONGenerator() = default;

  std::ostringstream Produce();

  void Generate(const flat::Decl* decl);

  void Generate(SourceSpan value);
  void Generate(NameSpan value);

  void Generate(types::HandleSubtype value);
  void Generate(types::Nullability value);

  void Generate(const raw::Identifier& value);
  void Generate(const flat::AttributeArg& value);
  void Generate(const flat::Attribute& value);
  void Generate(const flat::AttributeList& value);
  void Generate(const raw::Ordinal64& value);

  void Generate(const TypeShape& type_shape);
  void Generate(const FieldShape& type_shape);

  void GenerateDeclName(const flat::Name& name);
  void Generate(const flat::Name& value);
  void Generate(const flat::Type* value);
  void Generate(const flat::Constant& value);
  void Generate(const flat::ConstantValue& value);
  void Generate(const flat::Bits& value);
  void Generate(const flat::Bits::Member& value);
  void Generate(const flat::Const& value);
  void Generate(const flat::Enum& value);
  void Generate(const flat::Enum::Member& value);
  void Generate(const flat::Protocol& value);
  void Generate(const flat::Protocol::ComposedProtocol& composed_protocol);
  void Generate(const flat::Protocol::MethodWithInfo& value);
  void Generate(const flat::LiteralConstant& value);
  void Generate(const flat::Resource& value);
  void Generate(const flat::Resource::Property& value);
  void Generate(const flat::Service& value);
  void Generate(const flat::Service::Member& value);
  void Generate(const flat::Struct& value);
  void Generate(const flat::Struct* value);
  void Generate(const flat::Struct::Member& value, bool is_request_or_response = false);
  void Generate(const flat::Table& value);
  void Generate(const flat::Table::Member& value);
  void Generate(const flat::Union& value);
  void Generate(const flat::Union::Member& value);
  void Generate(const flat::LayoutInvocation& value);
  void Generate(const flat::TypeConstructor& value);
  void Generate(const flat::TypeAlias& value);
  void Generate(const flat::Library* library);

 private:
  enum TypeKind {
    kConcrete,
    kParameterized,
    kRequestPayload,
    kResponsePayload,
  };
  void GenerateTypeAndFromTypeAlias(TypeKind parent_type_kind, const flat::TypeConstructor* value,
                                    Position position = Position::kSubsequent);
  void GenerateTypeAndFromTypeAlias(const flat::TypeConstructor* value,
                                    Position position = Position::kSubsequent);

  // This is a generator for the builtin generics: array, vector, and request.
  // The "type" argument is the resolved type of the parameterized type to be
  // generated, and the "type_ctor" argument is the de-aliased constructor for
  // that type.  For example, consider the following FIDL
  //
  //   alias Foo = vector<bool>:5;
  //
  //   struct Example {
  //     bar Foo?;
  //   };
  //
  // When GenerateParameterizedType is called for Example.bar, the "type" will
  // be a nullable vector of size 5, but the de-aliased constructor passed in
  // will be the underlying type for just Foo, in this is case "vector<bool:5>."
  void GenerateParameterizedType(TypeKind parent_type_kind, const flat::Type* type,
                                 const flat::TypeConstructor* type_ctor,
                                 Position position = Position::kSubsequent);
  void GenerateExperimentalMaybeFromTypeAlias(const flat::LayoutInvocation& invocation);
  void GenerateDeclarationsEntry(int count, const flat::Name& name, std::string_view decl_kind);
  void GenerateDeclarationsMember(const flat::Library* library,
                                  Position position = Position::kSubsequent);
  void GenerateExternalDeclarationsEntry(int count, const flat::Name& name,
                                         std::string_view decl_kind,
                                         std::optional<types::Resourceness> maybe_resourceness);
  void GenerateExternalDeclarationsMember(const flat::Library* library,
                                          Position position = Position::kSubsequent);
  void GenerateTypeShapes(const flat::Object& object);
  void GenerateTypeShapes(std::string prefix, const flat::Struct* value);
  void GenerateFieldShapes(const flat::Struct::Member& struct_member,
                           bool is_request_or_response = false);

  const flat::Library* library_;
  std::ostringstream json_file_;
};

}  // namespace fidl

#endif  // TOOLS_FIDL_FIDLC_INCLUDE_FIDL_JSON_GENERATOR_H_
