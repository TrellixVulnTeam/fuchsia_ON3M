// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/device/c/fidl.h>
#include <fuchsia/io/c/fidl.h>
#include <lib/fdio/io.h>
#include <lib/fdio/namespace.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <lib/fdio/directory.h>
#include <string.h>
#include <zircon/device/vfs.h>
#include <zircon/syscalls.h>

#include "private.h"

#define ZXDEBUG 0

// POLL_MASK and POLL_SHIFT intend to convert the lower five POLL events into
// ZX_USER_SIGNALs and vice-versa. Other events need to be manually converted to
// a zx_signals_t, if they are desired.
#define POLL_SHIFT  24
#define POLL_MASK   0x1F

static_assert(FDIO_CHUNK_SIZE >= PATH_MAX,
              "FDIO_CHUNK_SIZE must be large enough to contain paths");

static_assert(fuchsia_io_VMO_FLAG_READ == ZX_VM_PERM_READ,
              "Vmar / Vmo flags should be aligned");
static_assert(fuchsia_io_VMO_FLAG_WRITE == ZX_VM_PERM_WRITE,
              "Vmar / Vmo flags should be aligned");
static_assert(fuchsia_io_VMO_FLAG_EXEC == ZX_VM_PERM_EXECUTE,
              "Vmar / Vmo flags should be aligned");

static_assert(ZX_USER_SIGNAL_0 == (1 << POLL_SHIFT), "");
static_assert((POLLIN << POLL_SHIFT) == fuchsia_device_DEVICE_SIGNAL_READABLE, "");
static_assert((POLLPRI << POLL_SHIFT) == fuchsia_device_DEVICE_SIGNAL_OOB, "");
static_assert((POLLOUT << POLL_SHIFT) == fuchsia_device_DEVICE_SIGNAL_WRITABLE, "");
static_assert((POLLERR << POLL_SHIFT) == fuchsia_device_DEVICE_SIGNAL_ERROR, "");
static_assert((POLLHUP << POLL_SHIFT) == fuchsia_device_DEVICE_SIGNAL_HANGUP, "");

// The |mode| argument used for |fuchsia.io.Directory/Open| calls.
#define FDIO_CONNECT_MODE ((uint32_t)0755)

// Closes the |zx_handle_t| in |info|, if one exists.
static void zxrio_object_close_handle_if_present(const fuchsia_io_NodeInfo* info) {
    switch (info->tag) {
    case fuchsia_io_NodeInfoTag_file:
        if (info->file.event != ZX_HANDLE_INVALID) {
            zx_handle_close(info->file.event);
        }
        break;
    case fuchsia_io_NodeInfoTag_pipe:
        if (info->pipe.socket != ZX_HANDLE_INVALID) {
            zx_handle_close(info->pipe.socket);
        }
        break;
    case fuchsia_io_NodeInfoTag_vmofile:
        if (info->vmofile.vmo != ZX_HANDLE_INVALID) {
            zx_handle_close(info->vmofile.vmo);
        }
        break;
    case fuchsia_io_NodeInfoTag_device:
        if (info->device.event != ZX_HANDLE_INVALID) {
            zx_handle_close(info->device.event);
        }
        break;
    case fuchsia_io_NodeInfoTag_tty:
        if (info->tty.event != ZX_HANDLE_INVALID) {
            zx_handle_close(info->tty.event);
        }
        break;
    }
}

// Validates a |path| argument.
//
// Returns ZX_OK if |path| is non-null and less than |PATH_MAX| in length
// (excluding the null terminator). Upon success, the length of the path is
// returned via |out_length|.
//
// Otherwise, returns |ZX_ERR_INVALID_ARGS|.
static zx_status_t fdio_validate_path(const char* path, size_t* out_length) {
    if (path == NULL) {
        return ZX_ERR_INVALID_ARGS;
    }
    size_t length = strlen(path);
    if (length >= PATH_MAX) {
        return ZX_ERR_INVALID_ARGS;
    }
    *out_length = length;
    return ZX_OK;
}

// A |fuchsia.io.Node/OnOpen| event and accompanying secondary object.
//
// Used to manually read and decode |OnOpen| events.
//
// In principle, the code for dealing with the |OnOpen| event should be
// generated by the FIDL compiler as part of the C bindings.
typedef struct {
    fuchsia_io_NodeOnOpenEvent primary;
    fuchsia_io_NodeInfo extra;
} fdio_on_open_msg_t;

// Decodes an |fuchsia.io.Node/OnOpen| event.
//
// Decodes the handle into |info|, if it exists and should be decoded.
//
// Takes ownership of |extra_handle|, if provided.
static zx_status_t zxrio_decode_on_open_event(fdio_on_open_msg_t* info,
                                              zx_handle_t extra_handle) {
    bool have_handle = (extra_handle != ZX_HANDLE_INVALID);
    bool want_handle = false;
    zx_handle_t* handle_target = NULL;

    switch (info->extra.tag) {
    // Case: No extra handles expected
    case fuchsia_io_NodeInfoTag_service:
    case fuchsia_io_NodeInfoTag_directory:
        break;
    // Case: Extra handles optional
    case fuchsia_io_NodeInfoTag_file:
        handle_target = &info->extra.file.event;
        goto handle_optional;
    case fuchsia_io_NodeInfoTag_device:
        handle_target = &info->extra.device.event;
        goto handle_optional;
    case fuchsia_io_NodeInfoTag_tty:
        handle_target = &info->extra.tty.event;
        goto handle_optional;
handle_optional:
        want_handle = *handle_target == FIDL_HANDLE_PRESENT;
        break;
    // Case: Extra handles required
    case fuchsia_io_NodeInfoTag_pipe:
        handle_target = &info->extra.pipe.socket;
        goto handle_required;
    case fuchsia_io_NodeInfoTag_vmofile:
        handle_target = &info->extra.vmofile.vmo;
        goto handle_required;
handle_required:
        want_handle = *handle_target == FIDL_HANDLE_PRESENT;
        if (!want_handle) {
            goto fail;
        }
        break;
    default:
        printf("Unexpected protocol type opening connection\n");
        goto fail;
    }

    if (have_handle != want_handle) {
        goto fail;
    }
    if (have_handle) {
        *handle_target = extra_handle;
    }
    return ZX_OK;

fail:
    if (have_handle) {
        zx_handle_close(extra_handle);
    }
    return ZX_ERR_IO;
}

// Waits for, and then decodes, an |fuchsia.io.Node/OnOpen| event.
//
// The content of the event are written into |info|.
static zx_status_t zxrio_process_on_open_event(zx_handle_t h,
                                               fdio_on_open_msg_t* info) {
    zx_object_wait_one(h, ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED,
                       ZX_TIME_INFINITE, NULL);

    // Attempt to read the description from open
    uint32_t dsize = sizeof(*info);
    zx_handle_t extra_handle = ZX_HANDLE_INVALID;
    uint32_t actual_handles;
    zx_status_t r = zx_channel_read(h, 0, info, &extra_handle, dsize, 1, &dsize,
                                    &actual_handles);
    if (r != ZX_OK) {
        return r;
    }
    if (dsize < sizeof(fuchsia_io_NodeOnOpenEvent) ||
        info->primary.hdr.ordinal != fuchsia_io_NodeOnOpenOrdinal) {
        r = ZX_ERR_IO;
    } else {
        r = info->primary.s;
    }

    if (dsize != sizeof(fdio_on_open_msg_t)) {
        r = (r != ZX_OK) ? r : ZX_ERR_IO;
    }

    if (r != ZX_OK) {
        if (extra_handle != ZX_HANDLE_INVALID) {
            zx_handle_close(extra_handle);
        }
        return r;
    }

    // Confirm that the objects "fdio_on_open_msg_t" and "fuchsia_io_NodeOnOpenEvent"
    // are aligned enough to be compatible.
    //
    // This is somewhat complicated by the fact that the "fuchsia_io_NodeOnOpenEvent"
    // object has an optional "fuchsia_io_NodeInfo" secondary which exists immediately
    // following the struct.
    static_assert(__builtin_offsetof(fdio_on_open_msg_t, extra) ==
                  FIDL_ALIGN(sizeof(fuchsia_io_NodeOnOpenEvent)),
                  "RIO Description message doesn't align with FIDL response secondary");
    // Connection::NodeDescribe also relies on these static_asserts.
    // fidl_describe also relies on these static_asserts.

    return zxrio_decode_on_open_event(info, extra_handle);
}

__EXPORT
zx_status_t fdio_service_connect(const char* path, zx_handle_t h) {
    // TODO: fdio_validate_path?
    if (path == NULL) {
        zx_handle_close(h);
        return ZX_ERR_INVALID_ARGS;
    }
    // Otherwise attempt to connect through the root namespace
    if (fdio_root_ns != NULL) {
        return fdio_ns_connect(fdio_root_ns, path, ZX_FS_RIGHT_READABLE | ZX_FS_RIGHT_WRITABLE, h);
    }
    // Otherwise we fail
    zx_handle_close(h);
    return ZX_ERR_NOT_FOUND;
}

__EXPORT
zx_status_t fdio_service_connect_at(zx_handle_t dir, const char* path, zx_handle_t request) {
    size_t length = 0u;
    zx_status_t status = fdio_validate_path(path, &length);
    if (status != ZX_OK) {
        zx_handle_close(request);
        return status;
    }

    if (dir == ZX_HANDLE_INVALID) {
        zx_handle_close(request);
        return ZX_ERR_UNAVAILABLE;
    }
    uint32_t flags = ZX_FS_RIGHT_READABLE | ZX_FS_RIGHT_WRITABLE;
    return fuchsia_io_DirectoryOpen(dir, flags, FDIO_CONNECT_MODE, path, length, request);
}

__EXPORT
zx_status_t fdio_open(const char* path, uint32_t flags, zx_handle_t request) {
    // TODO: fdio_validate_path?
    if (path == NULL) {
        zx_handle_close(request);
        return ZX_ERR_INVALID_ARGS;
    }
    // Otherwise attempt to connect through the root namespace
    if (fdio_root_ns != NULL) {
        return fdio_ns_connect(fdio_root_ns, path, flags, request);
    }
    // Otherwise we fail
    zx_handle_close(request);
    return ZX_ERR_NOT_FOUND;
}

__EXPORT
zx_status_t fdio_open_at(zx_handle_t dir, const char* path, uint32_t flags, zx_handle_t request) {
    size_t length = 0u;
    zx_status_t status = fdio_validate_path(path, &length);
    if (status != ZX_OK) {
        zx_handle_close(request);
        return status;
    }

    if (flags & ZX_FS_FLAG_DESCRIBE) {
        zx_handle_close(request);
        return ZX_ERR_INVALID_ARGS;
    }

    return fuchsia_io_DirectoryOpen(dir, flags, FDIO_CONNECT_MODE, path, length, request);
}

__EXPORT
zx_handle_t fdio_service_clone(zx_handle_t handle) {
    if (handle == ZX_HANDLE_INVALID) {
        return ZX_HANDLE_INVALID;
    }
    zx_handle_t clone, request;
    zx_status_t status = zx_channel_create(0, &clone, &request);
    if (status != ZX_OK) {
        return ZX_HANDLE_INVALID;
    }
    uint32_t flags = ZX_FS_FLAG_CLONE_SAME_RIGHTS;
    status = fuchsia_io_NodeClone(handle, flags, request);
    if (status != ZX_OK) {
        zx_handle_close(clone);
        return ZX_HANDLE_INVALID;
    }
    return clone;
}

__EXPORT
zx_status_t fdio_service_clone_to(zx_handle_t handle, zx_handle_t request) {
    if (handle == ZX_HANDLE_INVALID) {
        zx_handle_close(request);
        return ZX_ERR_INVALID_ARGS;
    }
    uint32_t flags = ZX_FS_FLAG_CLONE_SAME_RIGHTS;
    return fuchsia_io_NodeClone(handle, flags, request);
}

// Creates an |fdio_t| from a Zircon socket object.
//
// Examines |socket| and determines whether to create a pipe, stream socket, or
// datagram socket.
//
// Always consumes |socket|.
static zx_status_t fdio_from_socket(zx_handle_t socket, fdio_t** out_io) {
    zx_info_socket_t info;
    memset(&info, 0, sizeof(info));
    zx_status_t status = zx_object_get_info(socket, ZX_INFO_SOCKET, &info, sizeof(info), NULL, NULL);
    if (status != ZX_OK) {
        zx_handle_close(socket);
        return status;
    }
    fdio_t* io = NULL;
    if ((info.options & ZX_SOCKET_HAS_CONTROL) != 0) {
        // If the socket has a control plane, then the socket is either
        // a stream or a datagram socket.
        if ((info.options & ZX_SOCKET_DATAGRAM) != 0) {
            io = fdio_socket_create_datagram(socket, IOFLAG_SOCKET_CONNECTED);
        } else {
            io = fdio_socket_create_stream(socket, IOFLAG_SOCKET_CONNECTED);
        }
    } else {
        // Without a control plane, the socket is a pipe.
        io = fdio_pipe_create(socket);
    }
    if (!io) {
        return ZX_ERR_NO_RESOURCES;
    }
    *out_io = io;
    return ZX_OK;
}

// Create an |fdio_t| from a |handle| and an |info|.
//
// Uses |info| to determine what kind of |fdio_t| to create.
//
// Upon successs, |out_io| receives ownership of all handles.
//
// Upon failure, consumes all handles.
static zx_status_t fdio_from_node_info(zx_handle_t handle,
                                       fuchsia_io_NodeInfo* info,
                                       fdio_t** out_io) {
    fdio_t* io = NULL;
    zx_status_t status = ZX_OK;
    if (handle == ZX_HANDLE_INVALID) {
        status = ZX_ERR_INVALID_ARGS;
        goto failure;
    }

    switch (info->tag) {
    case fuchsia_io_NodeInfoTag_directory:
        io = fdio_dir_create(handle);
        break;
    case fuchsia_io_NodeInfoTag_service:
        io = fdio_remote_create(handle, 0);
        break;
    case fuchsia_io_NodeInfoTag_file:
        io = fdio_file_create(handle, info->file.event);
        break;
    case fuchsia_io_NodeInfoTag_device:
        io = fdio_remote_create(handle, info->device.event);
        break;
    case fuchsia_io_NodeInfoTag_tty:
        io = fdio_remote_create(handle, info->tty.event);
        break;
    case fuchsia_io_NodeInfoTag_vmofile: {
        uint64_t seek = 0u;
        zx_status_t io_status = fuchsia_io_FileSeek(
            handle, 0, fuchsia_io_SeekOrigin_START, &status, &seek);
        if (io_status != ZX_OK) {
            status = io_status;
        }
        if (status != ZX_OK) {
            goto failure;
        }
        io = fdio_vmofile_create(handle, info->vmofile.vmo, info->vmofile.offset,
                                 info->vmofile.length, seek);
        break;
    }
    case fuchsia_io_NodeInfoTag_pipe: {
        zx_handle_close(handle);
        return fdio_from_socket(info->pipe.socket, out_io);
    }
    default:
        status = ZX_ERR_NOT_SUPPORTED;
        goto failure;
    }

    if (io == NULL) {
        return ZX_ERR_NO_RESOURCES;
    }

    *out_io = io;
    return ZX_OK;

failure:
    zxrio_object_close_handle_if_present(info);
    zx_handle_close(handle);
    return status;
}

// Creates an |fdio_t| from a Zircon channel object.
//
// The |channel| must implement the |fuchsia.io.Node| protocol. Uses the
// |Describe| method from the |fuchsia.io.Node| protocol to determine the type
// of |fdio_t| object to create.
//
// Always consumes |channel|.
static zx_status_t fdio_from_channel(zx_handle_t channel, fdio_t** out_io) {
    fuchsia_io_NodeInfo info;
    memset(&info, 0, sizeof(info));
    zx_status_t status = fuchsia_io_NodeDescribe(channel, &info);
    if (status != ZX_OK) {
        zx_handle_close(channel);
        return status;
    }
    return fdio_from_node_info(channel, &info, out_io);
}

__EXPORT
zx_status_t fdio_create(zx_handle_t handle, fdio_t** out_io) {
    zx_info_handle_basic_t info;
    zx_status_t status = zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info,
                                            sizeof(info), NULL, NULL);
    if (status != ZX_OK)
        return status;
    fdio_t* io = NULL;
    switch (info.type) {
        case ZX_OBJ_TYPE_CHANNEL:
            return fdio_from_channel(handle, out_io);
        case ZX_OBJ_TYPE_SOCKET:
            return fdio_from_socket(handle, out_io);
        case ZX_OBJ_TYPE_VMO:
            io = fdio_vmo_create(handle, 0u);
            break;
        case ZX_OBJ_TYPE_LOG:
            io = fdio_logger_create(handle);
            break;
        default: {
            zx_handle_close(handle);
            return ZX_ERR_INVALID_ARGS;
        }
    }
    if (io == NULL) {
        return ZX_ERR_NO_MEMORY;
    }
    *out_io = io;
    return ZX_OK;
}

zx_status_t fdio_remote_open_at(zx_handle_t dir, const char* path, uint32_t flags,
                                uint32_t mode, fdio_t** out_io) {
    size_t length = 0u;
    zx_status_t status = fdio_validate_path(path, &length);
    if (status != ZX_OK) {
        return status;
    }

    zx_handle_t handle, request;
    status = zx_channel_create(0, &handle, &request);
    if (status != ZX_OK) {
        return status;
    }

    status = fuchsia_io_DirectoryOpen(dir, flags, mode, path, length, request);
    if (status != ZX_OK) {
        zx_handle_close(handle);
        return status;
    }

    if (flags & ZX_FS_FLAG_DESCRIBE) {
        fdio_on_open_msg_t info;
        memset(&info, 0, sizeof(info));

        status = zxrio_process_on_open_event(handle, &info);
        if (status != ZX_OK) {
            zx_handle_close(handle);
            return status;
        }
        return fdio_from_node_info(handle, &info.extra, out_io);
    }

    fdio_t* io = fdio_remote_create(handle, 0);
    if (io == NULL) {
        return ZX_ERR_NO_RESOURCES;
    }
    *out_io = io;
    return ZX_OK;
}
