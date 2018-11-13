// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ESCHER_PAPER_PAPER_TRANSFORM_STACK_H_
#define LIB_ESCHER_PAPER_PAPER_TRANSFORM_STACK_H_

#include <stack>
#include <vector>

#include "lib/escher/geometry/types.h"

namespace escher {

// PaperTransformStack is a helper class to be used along with PaperRenderer
// when rendering hierarchical scenes.  It maintains a stack where each item has
// two fields:
//   - a 4x4 model-to-world transform matrix
//   - a list of model-space clip planes
// When a new transform is pushed on the stack, existing clip planes are
// transformed into the new model-space coordinate system.
class PaperTransformStack final {
 public:
  PaperTransformStack();

  struct Item {
    mat4 matrix;
    std::vector<plane3> clip_planes;
  };

  // All of the Push*() methods generate a new Item and push it onto the stack.
  // The new Item is generated by applying the specified transform to the Item
  // that is currently on top of the stack.  The returned Item& is valid until
  // it is popped from the stack via Pop().
  // NOTE: Variants such as PushTranslation() and PushIdentity() are more
  // performant than PushTransform(), because they do something cheaper than
  // applying a 4x4 matrix to the matrix/clip-planes on top of the stack.
  // NOTE: as long as the returned item is on top of the stack, its clip-planes
  // will be affected by calls to AddClipPlanes(). Use PushIdentity() if you
  // want to add additional clip-planes in the current coordinate system without
  // affecting existing Item& references.  For example, this is handy for the
  // use-case where a parent has several children, and different sets of
  // clip-planes for each child in the parent's coordinate system.
  const Item& PushTransform(const mat4& transform);
  const Item& PushTranslation(const vec3& translation);
  const Item& PushTranslation(const vec2& translation) {
    return PushTranslation(vec3(translation, 0));
  }
  const Item& PushElevation(float elevation) {
    return PushTranslation(vec3(0, 0, elevation));
  }
  const Item& PushScale(float scale);
  const Item& PushIdentity();

  // Add the clip planes to the stack.  These planes are in the space defined by
  // the current transform.  In other words, they are not transformed by the
  // current transform, but will be transformed by each subsequent transform
  // that is pushed onto the stack.  NOTE: stack's size must be > 0 when this is
  // called.
  const Item& AddClipPlanes(const plane3* clip_planes, size_t num_clip_planes);
  const Item& AddClipPlanes(const std::vector<plane3>& clip_planes) {
    return AddClipPlanes(clip_planes.data(), clip_planes.size());
  }

  // There is always a valid Item at the top of the stack.  Popping the last
  // item clears the clip planes, leaving an identity matrix.
  const Item& Top() const {
    static const Item kTopItem{.matrix = mat4(1), .clip_planes = {}};
    return stack_.empty() ? kTopItem : stack_.top();
  }

  // Pop the top entry from the stack, unless only the default identity matrix
  // remains.
  PaperTransformStack& Pop();

  // Pop entries off the stack until only the specified number remain (zero by
  // default).  Then, trim trailing clip-planes until only the specified number
  // remain (zero by default).  In both cases, the specified number must not
  // exceed the number of stack-entries/clip-planes that currently exist.
  PaperTransformStack& Clear(
      std::pair<size_t, size_t> stack_size_and_num_clip_planes = {0, 0});

  size_t size() const { return stack_.size(); }
  size_t empty() const { return stack_.empty(); }
  size_t num_clip_planes() const { return Top().clip_planes.size(); }

 private:
  std::stack<Item> stack_;
};

}  // namespace escher

#endif  // LIB_ESCHER_PAPER_PAPER_TRANSFORM_STACK_H_
