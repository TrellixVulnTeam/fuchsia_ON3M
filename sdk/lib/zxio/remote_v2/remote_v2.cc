// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "remote_v2.h"

#include <fuchsia/io2/llcpp/fidl.h>
#include <lib/zx/channel.h>
#include <lib/zxio/cpp/inception.h>
#include <lib/zxio/cpp/vector.h>
#include <lib/zxio/null.h>
#include <lib/zxio/ops.h>
#include <zircon/syscalls.h>

#include <type_traits>

#include "common_utils.h"
#include "dirent_iterator.h"

namespace fio2 = fuchsia_io2;

namespace {

zxio_node_attributes_t ToZxioNodeAttributes(const fio2::wire::NodeAttributes& attr) {
  zxio_node_attributes_t zxio_attr = {};
  if (attr.has_protocols()) {
    ZXIO_NODE_ATTR_SET(zxio_attr, protocols, ToZxioNodeProtocols(attr.protocols()));
  }
  if (attr.has_abilities()) {
    ZXIO_NODE_ATTR_SET(zxio_attr, protocols, ToZxioAbilities(attr.abilities()));
  }
  if (attr.has_id()) {
    ZXIO_NODE_ATTR_SET(zxio_attr, id, attr.id());
  }
  if (attr.has_content_size()) {
    ZXIO_NODE_ATTR_SET(zxio_attr, content_size, attr.content_size());
  }
  if (attr.has_storage_size()) {
    ZXIO_NODE_ATTR_SET(zxio_attr, storage_size, attr.storage_size());
  }
  if (attr.has_link_count()) {
    ZXIO_NODE_ATTR_SET(zxio_attr, link_count, attr.link_count());
  }
  if (attr.has_creation_time()) {
    ZXIO_NODE_ATTR_SET(zxio_attr, creation_time, attr.creation_time());
  }
  if (attr.has_modification_time()) {
    ZXIO_NODE_ATTR_SET(zxio_attr, modification_time, attr.modification_time());
  }
  return zxio_attr;
}

fio2::wire::NodeAttributes ToIo2NodeAttributes(fidl::AnyArena& allocator,
                                               const zxio_node_attributes_t& attr) {
  fio2::wire::NodeAttributes node_attributes(allocator);
  if (attr.has.protocols) {
    node_attributes.set_protocols(allocator, ToIo2NodeProtocols(attr.protocols));
  }
  if (attr.has.abilities) {
    node_attributes.set_abilities(allocator, ToIo2Abilities(attr.abilities));
  }
  if (attr.has.id) {
    node_attributes.set_id(allocator, attr.id);
  }
  if (attr.has.content_size) {
    node_attributes.set_content_size(allocator, attr.content_size);
  }
  if (attr.has.storage_size) {
    node_attributes.set_storage_size(allocator, attr.storage_size);
  }
  if (attr.has.link_count) {
    node_attributes.set_link_count(allocator, attr.link_count);
  }
  if (attr.has.creation_time) {
    node_attributes.set_creation_time(allocator, attr.creation_time);
  }
  if (attr.has.modification_time) {
    node_attributes.set_modification_time(allocator, attr.modification_time);
  }
  return node_attributes;
}

// These functions are named with "v2" to avoid mixing up with fuchsia.io v1
// backend during grepping.

zx_status_t zxio_remote_v2_close(zxio_t* io) {
  RemoteV2 rio(io);
  zx_status_t status = [&]() {
    auto result = fidl::WireCall(fidl::UnownedClientEnd<fio2::Node>(rio.control())).Close();
    // TODO(yifeit): The |Node.Close| method is one-way. In order to catch
    // any server-side error during close, we should wait for an epitaph.
    if (result.status() != ZX_OK) {
      return result.status();
    }
    return rio.control()->wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), nullptr);
  }();
  rio.Close();
  return status;
}

zx_status_t zxio_remote_v2_release(zxio_t* io, zx_handle_t* out_handle) {
  RemoteV2 rio(io);
  *out_handle = rio.Release().release();
  return ZX_OK;
}

zx_status_t zxio_remote_v2_clone(zxio_t* io, zx_handle_t* out_handle) {
  RemoteV2 rio(io);
  auto ends = fidl::CreateEndpoints<fio2::Node>();
  if (!ends.is_ok()) {
    return ends.status_value();
  }
  auto result = fidl::WireCall(fidl::UnownedClientEnd<fio2::Node>(rio.control()))
                    .Reopen(fio2::wire::ConnectionOptions(), ends->server.TakeChannel());
  if (result.status() != ZX_OK) {
    return result.status();
  }
  *out_handle = ends->client.TakeChannel().release();
  return ZX_OK;
}

void zxio_remote_v2_wait_begin(zxio_t* io, zxio_signals_t zxio_signals, zx_handle_t* out_handle,
                               zx_signals_t* out_zx_signals) {
  RemoteV2 rio(io);
  *out_handle = rio.observer()->get();
  using fio2::wire::DeviceSignal;
  auto device_signal_part = DeviceSignal();
  if (zxio_signals & ZXIO_SIGNAL_READABLE) {
    device_signal_part |= DeviceSignal::kReadable;
  }
  if (zxio_signals & ZXIO_SIGNAL_OUT_OF_BAND) {
    device_signal_part |= DeviceSignal::kOob;
  }
  if (zxio_signals & ZXIO_SIGNAL_WRITABLE) {
    device_signal_part |= DeviceSignal::kWritable;
  }
  if (zxio_signals & ZXIO_SIGNAL_ERROR) {
    device_signal_part |= DeviceSignal::kError;
  }
  if (zxio_signals & ZXIO_SIGNAL_PEER_CLOSED) {
    device_signal_part |= DeviceSignal::kHangup;
  }
  // static_cast is a-okay, because |DeviceSignal| values are defined
  // using Zircon ZX_USER_* signals.
  auto zx_signals = static_cast<zx_signals_t>(device_signal_part);
  if (zxio_signals & ZXIO_SIGNAL_READ_DISABLED) {
    zx_signals |= ZX_CHANNEL_PEER_CLOSED;
  }
  *out_zx_signals = zx_signals;
}

void zxio_remote_v2_wait_end(zxio_t* io, zx_signals_t zx_signals,
                             zxio_signals_t* out_zxio_signals) {
  zxio_signals_t zxio_signals = ZXIO_SIGNAL_NONE;
  using fio2::wire::DeviceSignal;
  // static_cast is a-okay, because |DeviceSignal| values are defined
  // using Zircon ZX_USER_* signals.
  auto device_signal_part = DeviceSignal::TruncatingUnknown(zx_signals);
  if (device_signal_part & DeviceSignal::kReadable) {
    zxio_signals |= ZXIO_SIGNAL_READABLE;
  }
  if (device_signal_part & DeviceSignal::kOob) {
    zxio_signals |= ZXIO_SIGNAL_OUT_OF_BAND;
  }
  if (device_signal_part & DeviceSignal::kWritable) {
    zxio_signals |= ZXIO_SIGNAL_WRITABLE;
  }
  if (device_signal_part & DeviceSignal::kError) {
    zxio_signals |= ZXIO_SIGNAL_ERROR;
  }
  if (device_signal_part & DeviceSignal::kHangup) {
    zxio_signals |= ZXIO_SIGNAL_PEER_CLOSED;
  }
  if (zx_signals & ZX_CHANNEL_PEER_CLOSED) {
    zxio_signals |= ZXIO_SIGNAL_READ_DISABLED;
  }
  *out_zxio_signals = zxio_signals;
}

zx_status_t zxio_remote_sync(zxio_t* io) {
  RemoteV2 rio(io);
  auto result = fidl::WireCall(fidl::UnownedClientEnd<fio2::Node>(rio.control())).Sync();
  if (result.status() != ZX_OK) {
    return result.status();
  }
  if (result->result.is_err()) {
    return result->result.err();
  }
  return ZX_OK;
}

zx_status_t zxio_remote_v2_attr_get(zxio_t* io, zxio_node_attributes_t* out_attr) {
  RemoteV2 rio(io);
  auto result = fidl::WireCall(fidl::UnownedClientEnd<fio2::Node>(rio.control()))
                    .GetAttributes(fio2::wire::NodeAttributesQuery::kMask);
  if (result.status() != ZX_OK) {
    return result.status();
  }
  if (result->result.is_err()) {
    return result->result.err();
  }
  const fio2::wire::NodeAttributes& attributes = result->result.response().attributes;
  *out_attr = ToZxioNodeAttributes(attributes);
  return ZX_OK;
}

zx_status_t zxio_remote_v2_attr_set(zxio_t* io, const zxio_node_attributes_t* attr) {
  fidl::Arena<1024> allocator;
  auto attributes = ToIo2NodeAttributes(allocator, *attr);
  RemoteV2 rio(io);
  auto result = fidl::WireCall(fidl::UnownedClientEnd<fio2::Node>(rio.control()))
                    .UpdateAttributes(std::move(attributes));
  if (result.status() != ZX_OK) {
    return result.status();
  }
  if (result->result.is_err()) {
    return result->result.err();
  }
  return ZX_OK;
}

}  // namespace

void RemoteV2::Close() {
  Release().reset();
  if (rio_->observer != ZX_HANDLE_INVALID) {
    zx_handle_close(rio_->observer);
    rio_->observer = ZX_HANDLE_INVALID;
  }
  if (rio_->stream != ZX_HANDLE_INVALID) {
    zx_handle_close(rio_->stream);
    rio_->stream = ZX_HANDLE_INVALID;
  }
}

zx::channel RemoteV2::Release() {
  zx::channel control(rio_->control);
  rio_->control = ZX_HANDLE_INVALID;
  return control;
}

static constexpr zxio_ops_t zxio_remote_v2_ops = []() {
  zxio_ops_t ops = zxio_default_ops;
  ops.close = zxio_remote_v2_close;
  ops.release = zxio_remote_v2_release;
  ops.clone = zxio_remote_v2_clone;
  ops.wait_begin = zxio_remote_v2_wait_begin;
  ops.wait_end = zxio_remote_v2_wait_end;
  ops.sync = zxio_remote_sync;
  ops.attr_get = zxio_remote_v2_attr_get;
  ops.attr_set = zxio_remote_v2_attr_set;
  return ops;
}();

zx_status_t zxio_remote_v2_init(zxio_storage_t* storage, zx_handle_t control,
                                zx_handle_t observer) {
  auto remote = reinterpret_cast<zxio_remote_v2_t*>(storage);
  zxio_init(&remote->io, &zxio_remote_v2_ops);
  remote->control = control;
  remote->observer = observer;
  remote->stream = ZX_HANDLE_INVALID;
  return ZX_OK;
}

static constexpr zxio_ops_t zxio_dir_v2_ops = []() {
  zxio_ops_t ops = zxio_default_ops;
  ops.close = zxio_remote_v2_close;
  ops.release = zxio_remote_v2_release;
  ops.clone = zxio_remote_v2_clone;
  ops.sync = zxio_remote_sync;
  ops.attr_get = zxio_remote_v2_attr_get;
  ops.attr_set = zxio_remote_v2_attr_set;
  ops.dirent_iterator_init = zxio_remote_v2_dirent_iterator_init;
  ops.dirent_iterator_next = zxio_remote_v2_dirent_iterator_next;
  ops.dirent_iterator_destroy = zxio_remote_v2_dirent_iterator_destroy;
  return ops;
}();

zx_status_t zxio_dir_v2_init(zxio_storage_t* storage, zx_handle_t control) {
  auto remote = reinterpret_cast<zxio_remote_v2_t*>(storage);
  zxio_init(&remote->io, &zxio_dir_v2_ops);
  remote->control = control;
  remote->observer = ZX_HANDLE_INVALID;
  remote->stream = ZX_HANDLE_INVALID;
  return ZX_OK;
}

namespace {

void zxio_file_v2_wait_begin(zxio_t* io, zxio_signals_t zxio_signals, zx_handle_t* out_handle,
                             zx_signals_t* out_zx_signals) {
  using fio2::wire::FileSignal;
  RemoteV2 rio(io);
  *out_handle = rio.observer()->get();
  auto file_signal_part = FileSignal();
  if (zxio_signals & ZXIO_SIGNAL_READABLE) {
    file_signal_part |= FileSignal::kReadable;
  }
  if (zxio_signals & ZXIO_SIGNAL_WRITABLE) {
    file_signal_part |= FileSignal::kWritable;
  }
  auto zx_signals = static_cast<zx_signals_t>(file_signal_part);
  *out_zx_signals = zx_signals;
}

void zxio_file_v2_wait_end(zxio_t* io, zx_signals_t zx_signals, zxio_signals_t* out_zxio_signals) {
  using fio2::wire::FileSignal;
  zxio_signals_t zxio_signals = ZXIO_SIGNAL_NONE;
  auto file_signal_part = FileSignal::TruncatingUnknown(zx_signals);
  if (file_signal_part & FileSignal::kReadable) {
    zxio_signals |= ZXIO_SIGNAL_READABLE;
  }
  if (file_signal_part & FileSignal::kWritable) {
    zxio_signals |= ZXIO_SIGNAL_WRITABLE;
  }
  *out_zxio_signals = zxio_signals;
}

template <typename F>
static zx_status_t zxio_remote_do_vector(const RemoteV2& rio, const zx_iovec_t* vector,
                                         size_t vector_count, zxio_flags_t flags,
                                         size_t* out_actual, F fn) {
  return zxio_do_vector(vector, vector_count, out_actual,
                        [&](void* data, size_t capacity, size_t* out_actual) {
                          auto buffer = static_cast<uint8_t*>(data);
                          size_t total = 0;
                          while (capacity > 0) {
                            size_t chunk = std::min(capacity, fio2::wire::kMaxTransferSize);
                            size_t actual;
                            zx_status_t status = fn(rio.control(), buffer, chunk, &actual);
                            if (status != ZX_OK) {
                              return status;
                            }
                            total += actual;
                            if (actual != chunk) {
                              break;
                            }
                            buffer += actual;
                            capacity -= actual;
                          }
                          *out_actual = total;
                          return ZX_OK;
                        });
}

zx_status_t zxio_remote_v2_readv(zxio_t* io, const zx_iovec_t* vector, size_t vector_count,
                                 zxio_flags_t flags, size_t* out_actual) {
  if (flags) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  RemoteV2 rio(io);
  if (rio.stream()->is_valid()) {
    return rio.stream()->readv(0, vector, vector_count, out_actual);
  }

  return zxio_remote_do_vector(
      rio, vector, vector_count, flags, out_actual,
      [](zx::unowned_channel control, uint8_t* buffer, size_t capacity, size_t* out_actual) {
        // Explicitly allocating message buffers to avoid heap allocation.
        fidl::Buffer<fidl::WireRequest<fio2::File::Read>> request_buffer;
        fidl::Buffer<fidl::WireResponse<fio2::File::Read>> response_buffer;
        auto result = fidl::WireCall(fidl::UnownedClientEnd<fio2::File>(control))
                          .Read(request_buffer.view(), capacity, response_buffer.view());
        zx_status_t status;
        if ((status = result.status()) != ZX_OK) {
          return status;
        }
        if (result->result.is_err()) {
          return result->result.err();
        }
        const auto& data = result->result.response().data;
        size_t actual = data.count();
        if (actual > capacity) {
          return ZX_ERR_IO;
        }
        memcpy(buffer, data.begin(), actual);
        *out_actual = actual;
        return ZX_OK;
      });
}

zx_status_t zxio_remote_v2_readv_at(zxio_t* io, zx_off_t offset, const zx_iovec_t* vector,
                                    size_t vector_count, zxio_flags_t flags, size_t* out_actual) {
  if (flags) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  RemoteV2 rio(io);
  if (rio.stream()->is_valid()) {
    return rio.stream()->readv_at(0, offset, vector, vector_count, out_actual);
  }

  return zxio_remote_do_vector(
      rio, vector, vector_count, flags, out_actual,
      [&offset](zx::unowned_channel control, uint8_t* buffer, size_t capacity, size_t* out_actual) {
        fidl::Buffer<fidl::WireRequest<fio2::File::ReadAt>> request_buffer;
        fidl::Buffer<fidl::WireResponse<fio2::File::ReadAt>> response_buffer;
        auto result = fidl::WireCall(fidl::UnownedClientEnd<fio2::File>(control))
                          .ReadAt(request_buffer.view(), capacity, offset, response_buffer.view());
        zx_status_t status;
        if ((status = result.status()) != ZX_OK) {
          return status;
        }
        if (result->result.is_err()) {
          return result->result.err();
        }
        const auto& data = result->result.response().data;
        size_t actual = data.count();
        if (actual > capacity) {
          return ZX_ERR_IO;
        }
        offset += actual;
        memcpy(buffer, data.begin(), actual);
        *out_actual = actual;
        return ZX_OK;
      });
}

zx_status_t zxio_remote_v2_writev(zxio_t* io, const zx_iovec_t* vector, size_t vector_count,
                                  zxio_flags_t flags, size_t* out_actual) {
  if (flags) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  RemoteV2 rio(io);
  if (rio.stream()->is_valid()) {
    return rio.stream()->writev(0, vector, vector_count, out_actual);
  }

  return zxio_remote_do_vector(
      rio, vector, vector_count, flags, out_actual,
      [](zx::unowned_channel control, uint8_t* buffer, size_t capacity, size_t* out_actual) {
        // Explicitly allocating message buffers to avoid heap allocation.
        fidl::Buffer<fidl::WireRequest<fio2::File::Write>> request_buffer;
        fidl::Buffer<fidl::WireResponse<fio2::File::Write>> response_buffer;
        auto result = fidl::WireCall(fidl::UnownedClientEnd<fio2::File>(control))
                          .Write(request_buffer.view(),
                                 fidl::VectorView<uint8_t>::FromExternal(buffer, capacity),
                                 response_buffer.view());
        zx_status_t status;
        if ((status = result.status()) != ZX_OK) {
          return status;
        }
        if (result->result.is_err()) {
          return result->result.err();
        }
        size_t actual = result->result.response().actual_count;
        if (actual > capacity) {
          return ZX_ERR_IO;
        }
        *out_actual = actual;
        return ZX_OK;
      });
}

zx_status_t zxio_remote_v2_writev_at(zxio_t* io, zx_off_t offset, const zx_iovec_t* vector,
                                     size_t vector_count, zxio_flags_t flags, size_t* out_actual) {
  if (flags) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  RemoteV2 rio(io);
  if (rio.stream()->is_valid()) {
    return rio.stream()->writev_at(0, offset, vector, vector_count, out_actual);
  }

  return zxio_remote_do_vector(
      rio, vector, vector_count, flags, out_actual,
      [&offset](zx::unowned_channel control, uint8_t* buffer, size_t capacity, size_t* out_actual) {
        // Explicitly allocating message buffers to avoid heap allocation.
        fidl::Buffer<fidl::WireRequest<fio2::File::WriteAt>> request_buffer;
        fidl::Buffer<fidl::WireResponse<fio2::File::WriteAt>> response_buffer;
        auto result = fidl::WireCall(fidl::UnownedClientEnd<fio2::File>(control))
                          .WriteAt(request_buffer.view(),
                                   fidl::VectorView<uint8_t>::FromExternal(buffer, capacity),
                                   offset, response_buffer.view());
        zx_status_t status;
        if ((status = result.status()) != ZX_OK) {
          return status;
        }
        if (result->result.is_err()) {
          return result->result.err();
        }
        size_t actual = result->result.response().actual_count;
        if (actual > capacity) {
          return ZX_ERR_IO;
        }
        offset += actual;
        *out_actual = actual;
        return ZX_OK;
      });
}

zx_status_t zxio_remote_v2_seek(zxio_t* io, zxio_seek_origin_t start, int64_t offset,
                                size_t* out_offset) {
  RemoteV2 rio(io);
  if (rio.stream()->is_valid()) {
    return rio.stream()->seek(start, offset, out_offset);
  }

  auto result = fidl::WireCall(fidl::UnownedClientEnd<fio2::File>(rio.control()))
                    .Seek(static_cast<fio2::wire::SeekOrigin>(start), offset);
  if (result.status() != ZX_OK) {
    return result.status();
  }
  if (result->result.is_err()) {
    return result->result.err();
  }
  *out_offset = result->result.response().offset_from_start;
  return ZX_OK;
}

}  // namespace

static constexpr zxio_ops_t zxio_file_v2_ops = []() {
  zxio_ops_t ops = zxio_default_ops;
  ops.close = zxio_remote_v2_close;
  ops.release = zxio_remote_v2_release;
  ops.clone = zxio_remote_v2_clone;
  ops.wait_begin = zxio_file_v2_wait_begin;
  ops.wait_end = zxio_file_v2_wait_end;
  ops.sync = zxio_remote_sync;
  ops.attr_get = zxio_remote_v2_attr_get;
  ops.attr_set = zxio_remote_v2_attr_set;
  ops.readv = zxio_remote_v2_readv;
  ops.readv_at = zxio_remote_v2_readv_at;
  ops.writev = zxio_remote_v2_writev;
  ops.writev_at = zxio_remote_v2_writev_at;
  ops.seek = zxio_remote_v2_seek;
  return ops;
}();

zx_status_t zxio_file_v2_init(zxio_storage_t* storage, zx_handle_t control, zx_handle_t observer,
                              zx_handle_t stream) {
  auto remote = reinterpret_cast<zxio_remote_v2_t*>(storage);
  zxio_init(&remote->io, &zxio_file_v2_ops);
  remote->control = control;
  remote->observer = observer;
  remote->stream = stream;
  return ZX_OK;
}
