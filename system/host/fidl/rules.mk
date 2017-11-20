# Copyright 2017 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

LOCAL_DIR := $(GET_LOCAL_DIR)

MODULE := $(LOCAL_DIR)

MODULE_TYPE := hostapp

MODULE_COMPILEFLAGS := -O0 -g

MODULE_SRCS := \
    $(LOCAL_DIR)/lib/identifier_table.cpp \
    $(LOCAL_DIR)/lib/lexer.cpp \
    $(LOCAL_DIR)/lib/parser.cpp \
    $(LOCAL_DIR)/lib/source_manager.cpp \
    $(LOCAL_DIR)/main.cpp \

include make/module.mk
