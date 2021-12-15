// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_ADMIN_SERVICE_H_
#define SRC_STORAGE_BLOBFS_ADMIN_SERVICE_H_

#include "src/lib/storage/vfs/cpp/service.h"
#include "src/storage/blobfs/runner.h"

namespace blobfs {

class AdminService : public fidl::WireServer<fuchsia_fs::Admin>, public fs::Service {
 public:
  AdminService(async_dispatcher_t* dispatcher, Runner& runner);

  void Shutdown(ShutdownRequestView request, ShutdownCompleter::Sync& completer) override;
  void GetRoot(GetRootRequestView request, GetRootCompleter::Sync& completer) override {
    // Not yet supported.
  }

 private:
  Runner& runner_;
};

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_ADMIN_SERVICE_H_
