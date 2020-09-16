// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/display/hardware_display_controller_provider_impl.h"

#include <fcntl.h>
#include <fuchsia/hardware/display/c/fidl.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include "src/lib/fsl/io/device_watcher.h"

namespace ui_display {

static const std::string kDisplayDir = "/dev/class/display-controller";

HardwareDisplayControllerProviderImpl::HardwareDisplayControllerProviderImpl(
    sys::ComponentContext* app_context) {
  app_context->outgoing()->AddPublicService(bindings_.GetHandler(this));
}

// |fuchsia::hardware::display::Provider|.
void HardwareDisplayControllerProviderImpl::OpenController(
    zx::channel device, ::fidl::InterfaceRequest<fuchsia::hardware::display::Controller> request,
    OpenControllerCallback callback) {
  // We want the lifetime of |watcher| below to be the same as the closure passed into its
  // constructor.
  auto holder = std::make_shared<std::unique_ptr<fsl::DeviceWatcher>>();

  auto watcher = fsl::DeviceWatcher::Create(
      kDisplayDir, [holder, device = std::move(device), request = std::move(request),
                    callback = std::move(callback)](int dir_fd, std::string filename) mutable {
        // Get display info.
        std::string path = kDisplayDir + "/" + filename;

        FX_LOGS(INFO) << "Found display controller at path: " << path << ".";
        fxl::UniqueFD fd(open(path.c_str(), O_RDWR));
        if (!fd.is_valid()) {
          FX_LOGS(ERROR) << "Failed to open display_controller at path: " << path
                         << " (errno: " << errno << " " << strerror(errno) << ")";

          // We could try to match the value of the C "errno" macro to the closest ZX error, but
          // this would give rise to many corner cases.  We never expect this to fail anyway, since
          // |filename| is given to us by the device watcher.
          callback(ZX_ERR_INTERNAL);
          return;
        }

        // TODO(57269): it would be nice to simply pass |callback| asynchronously into
        // OpenController(), rather than blocking on a synchronous call.  However, it is non-trivial
        // to do so, so for now we use a blocking call to proxy the request.
        fdio_cpp::FdioCaller caller(std::move(fd));
        zx_status_t status = ZX_OK;
        zx_status_t fidl_status = fuchsia_hardware_display_ProviderOpenController(
            caller.borrow_channel(), device.release(), request.TakeChannel().release(), &status);
        if (fidl_status != ZX_OK) {
          FX_LOGS(ERROR) << "Failed to call service handle: " << zx_status_get_string(fidl_status);

          // There's not a clearly-better value to return here.  Returning the FIDL error would be
          // somewhat unexpected, since the caller wouldn't receive it as a FIDL status, rather as
          // the return value of a "successful" method invocation.
          callback(ZX_ERR_INTERNAL);
          return;
        }
        if (status != ZX_OK) {
          FX_LOGS(ERROR) << "Failed to open display controller : " << zx_status_get_string(status);
          callback(status);
          return;
        }

        callback(status);
      });

  // Keep the device-watcher alive until the callback fires.
  *holder = std::move(watcher);
}

}  // namespace ui_display
