// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/zx/vmo.h>
#include <lib/zxio/inception.h>
#include <string.h>

#include <zxtest/zxtest.h>

#include "sdk/lib/zxio/tests/test_directory_server_base.h"
#include "sdk/lib/zxio/tests/test_file_server_base.h"

TEST(CreateWithAllocator, ErrorAllocator) {
  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    return ZX_ERR_INVALID_ARGS;
  };
  zx::channel channel0, channel1;
  ASSERT_OK(zx::channel::create(0u, &channel0, &channel1));
  void* context;
  ASSERT_EQ(zxio_create_with_allocator(std::move(channel0), allocator, &context), ZX_ERR_NO_MEMORY);

  // Make sure that the handle is closed.
  zx_signals_t pending = 0;
  ASSERT_EQ(channel1.wait_one(0u, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending & ZX_CHANNEL_PEER_CLOSED, ZX_CHANNEL_PEER_CLOSED, "pending is %u", pending);
}

TEST(CreateWithAllocator, BadAllocator) {
  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    *out_storage = nullptr;
    return ZX_OK;
  };
  zx::channel channel0, channel1;
  ASSERT_OK(zx::channel::create(0u, &channel0, &channel1));
  void* context;
  ASSERT_EQ(zxio_create_with_allocator(std::move(channel0), allocator, &context), ZX_ERR_NO_MEMORY);

  // Make sure that the handle is closed.
  zx_signals_t pending = 0;
  ASSERT_EQ(channel1.wait_one(0u, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending & ZX_CHANNEL_PEER_CLOSED, ZX_CHANNEL_PEER_CLOSED, "pending is %u", pending);
}

struct VmoWrapper {
  int32_t tag;
  zxio_storage_t storage;
};

TEST(CreateWithAllocator, Vmo) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(1024, 0u, &vmo));

  uint32_t data = 0x1a2a3a4a;
  ASSERT_OK(vmo.write(&data, 0u, sizeof(data)));

  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    if (type != ZXIO_OBJECT_TYPE_VMO) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    auto* wrapper = new VmoWrapper{
        .tag = 0x42,
        .storage = {},
    };
    *out_storage = &(wrapper->storage);
    *out_context = wrapper;
    return ZX_OK;
  };

  void* context = nullptr;
  ASSERT_OK(zxio_create_with_allocator(std::move(vmo), allocator, &context));
  ASSERT_NE(nullptr, context);
  std::unique_ptr<VmoWrapper> wrapper(static_cast<VmoWrapper*>(context));
  ASSERT_EQ(wrapper->tag, 0x42);

  zxio_t* io = &(wrapper->storage.io);

  uint32_t buffer = 0;
  size_t actual = 0;
  zxio_read(io, &buffer, sizeof(buffer), 0u, &actual);
  ASSERT_EQ(actual, sizeof(buffer));
  ASSERT_EQ(buffer, data);

  ASSERT_OK(zxio_close(io));
}

TEST(CreateWithInfo, Unsupported) {
  auto node_ends = fidl::CreateEndpoints<fuchsia_io::Node>();
  ASSERT_OK(node_ends.status_value());
  auto [node_client, node_server] = std::move(node_ends.value());

  zx::socket socket0, socket1;
  ASSERT_OK(zx::socket::create(0u, &socket0, &socket1));

  fuchsia_io::wire::StreamSocket stream_socket = {.socket = std::move(socket0)};
  fidl::FidlAllocator fidl_allocator;
  auto node_info =
      fuchsia_io::wire::NodeInfo::WithStreamSocket(fidl_allocator, std::move(stream_socket));

  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    *out_storage = new zxio_storage_t;
    *out_context = *out_storage;
    return ZX_OK;
  };

  void* context = nullptr;
  zx_status_t status =
      zxio_create_with_allocator(std::move(node_client), node_info, allocator, &context);
  EXPECT_EQ(status, ZX_ERR_NOT_SUPPORTED);
  ASSERT_NE(context, nullptr);

  // The socket in node_info should be preserved.
  EXPECT_EQ(node_info.which(), fuchsia_io::wire::NodeInfo::Tag::kStreamSocket);
  EXPECT_TRUE(node_info.stream_socket().socket.is_valid());

  std::unique_ptr<zxio_storage_t> storage(static_cast<zxio_storage_t*>(context));
  zxio_t* zxio = &(storage->io);

  zx::channel recaptured_handle;
  ASSERT_OK(zxio_release(zxio, recaptured_handle.reset_and_get_address()));
  EXPECT_TRUE(recaptured_handle.is_valid());
  ASSERT_OK(zxio_close(zxio));
}

class TestDirectoryServer final : public zxio_tests::TestDirectoryServerBase {
  void Sync(SyncRequestView request, SyncCompleter::Sync& completer) final {
    completer.Reply(ZX_OK);
  }
};

TEST(CreateWithInfo, Directory) {
  auto dir_ends = fidl::CreateEndpoints<fuchsia_io::Directory>();
  ASSERT_OK(dir_ends.status_value());
  auto [dir_client, dir_server] = std::move(dir_ends.value());

  fidl::FidlAllocator fidl_allocator;
  auto node_info = fuchsia_io::wire::NodeInfo::WithDirectory(fidl_allocator);

  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    if (type != ZXIO_OBJECT_TYPE_DIR) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    *out_storage = new zxio_storage_t;
    *out_context = *out_storage;
    return ZX_OK;
  };

  async::Loop dir_control_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  TestDirectoryServer server;
  fidl::BindServer(dir_control_loop.dispatcher(), std::move(dir_server), &server);
  dir_control_loop.StartThread("dir_control_thread");

  void* context = nullptr;
  fidl::ClientEnd<fuchsia_io::Node> node_client(dir_client.TakeChannel());
  ASSERT_OK(zxio_create_with_allocator(std::move(node_client), node_info, allocator, &context));
  ASSERT_NE(context, nullptr);

  std::unique_ptr<zxio_storage_t> storage(static_cast<zxio_storage_t*>(context));
  zxio_t* zxio = &(storage->io);

  // Sanity check the zxio by sending a sync operation to the server.
  EXPECT_OK(zxio_sync(zxio));

  ASSERT_OK(zxio_close(zxio));

  dir_control_loop.Shutdown();
}

class TestFileServer final : public zxio_tests::TestFileServerBase {
 public:
  void Read(ReadRequestView request, ReadCompleter::Sync& completer) final {
    fidl::FidlAllocator fidl_allocator;
    fidl::VectorView<uint8_t> read_data(fidl_allocator, sizeof(kTestData));
    memcpy(read_data.mutable_data(), kTestData, sizeof(kTestData));
    completer.Reply(ZX_OK, read_data);
  }

  static constexpr char kTestData[] = "abcdef";
};

TEST(CreateWithInfo, File) {
  auto file_ends = fidl::CreateEndpoints<fuchsia_io::File>();
  ASSERT_OK(file_ends.status_value());
  auto [file_client, file_server] = std::move(file_ends.value());

  zx::event file_event;
  ASSERT_OK(zx::event::create(0u, &file_event));
  fuchsia_io::wire::FileObject file = {.event = std::move(file_event)};

  fidl::FidlAllocator fidl_allocator;
  auto node_info = fuchsia_io::wire::NodeInfo::WithFile(fidl_allocator, std::move(file));

  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    if (type != ZXIO_OBJECT_TYPE_FILE) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    *out_storage = new zxio_storage_t;
    *out_context = *out_storage;
    return ZX_OK;
  };

  async::Loop file_control_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  TestFileServer server;
  fidl::BindServer(file_control_loop.dispatcher(), std::move(file_server), &server);
  file_control_loop.StartThread("file_control_thread");

  void* context = nullptr;
  fidl::ClientEnd<fuchsia_io::Node> node_client(file_client.TakeChannel());
  ASSERT_OK(zxio_create_with_allocator(std::move(node_client), node_info, allocator, &context));
  ASSERT_NE(context, nullptr);

  // The event in node_info should be consumed by zxio.
  EXPECT_FALSE(node_info.file().event.is_valid());

  std::unique_ptr<zxio_storage_t> storage(static_cast<zxio_storage_t*>(context));
  zxio_t* zxio = &(storage->io);

  // Sanity check the zxio by reading some test data from the server.
  char buffer[sizeof(TestFileServer::kTestData)];
  size_t actual = 0u;

  ASSERT_OK(zxio_read(zxio, buffer, sizeof(buffer), 0u, &actual));

  EXPECT_EQ(sizeof(buffer), actual);
  EXPECT_BYTES_EQ(buffer, TestFileServer::kTestData, sizeof(buffer));

  ASSERT_OK(zxio_close(zxio));

  file_control_loop.Shutdown();
}

TEST(CreateWithInfo, Pipe) {
  auto node_ends = fidl::CreateEndpoints<fuchsia_io::Node>();
  ASSERT_OK(node_ends.status_value());
  auto [node_client, node_server] = std::move(node_ends.value());

  zx::socket socket0, socket1;
  ASSERT_OK(zx::socket::create(0u, &socket0, &socket1));
  fuchsia_io::wire::Pipe pipe = {.socket = std::move(socket0)};
  fidl::FidlAllocator fidl_allocator;
  auto node_info = fuchsia_io::wire::NodeInfo::WithPipe(fidl_allocator, std::move(pipe));

  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    if (type != ZXIO_OBJECT_TYPE_PIPE) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    *out_storage = new zxio_storage_t;
    *out_context = *out_storage;
    return ZX_OK;
  };

  void* context = nullptr;
  ASSERT_OK(zxio_create_with_allocator(std::move(node_client), node_info, allocator, &context));
  ASSERT_NE(context, nullptr);

  // The socket in node_info should be consumed by zxio.
  EXPECT_FALSE(node_info.pipe().socket.is_valid());

  std::unique_ptr<zxio_storage_t> storage(static_cast<zxio_storage_t*>(context));
  zxio_t* zxio = &(storage->io);

  // Send some data through the kernel socket object and read it through zxio to
  // sanity check that the pipe is functional.
  int32_t data = 0x1a2a3a4a;
  size_t actual = 0u;
  ASSERT_OK(socket1.write(0u, &data, sizeof(data), &actual));
  EXPECT_EQ(actual, sizeof(data));

  int32_t buffer = 0;
  ASSERT_OK(zxio_read(zxio, &buffer, sizeof(buffer), 0u, &actual));
  EXPECT_EQ(actual, sizeof(buffer));
  EXPECT_EQ(buffer, data);

  ASSERT_OK(zxio_close(zxio));
}
