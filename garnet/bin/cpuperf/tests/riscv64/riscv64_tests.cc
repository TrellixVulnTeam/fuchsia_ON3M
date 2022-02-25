// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "riscv64_tests.h"

#include <iterator>

// List of tests.
// A test automatically fails if it's not listed here.
extern const TestSpec* const kTestSpecs[] = {
    &kTallySpec,
};

extern const size_t kTestSpecCount = std::size(kTestSpecs);
