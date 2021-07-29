// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use crate::types::*;

/// A node that represents a symlink to another node.
pub struct SymlinkNode {
    /// The target of the symlink (the path to use to find the actual node).
    target: FsString,
}

impl SymlinkNode {
    pub fn new(target: &FsStr) -> Self {
        SymlinkNode { target: target.to_owned() }
    }
}

impl FsNodeOps for SymlinkNode {
    fn open(&self, _node: &FsNode, _flags: OpenFlags) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(NullFile))
    }

    fn readlink<'a>(&'a self, node: &FsNode) -> Result<FsString, Errno> {
        let now = fuchsia_runtime::utc_time();
        node.info_write().time_access = now;
        Ok(self.target.clone())
    }
}
