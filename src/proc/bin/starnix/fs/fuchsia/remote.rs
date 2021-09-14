// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_zircon::{self as zx, HandleBased};
use log::warn;
use parking_lot::{RwLockReadGuard, RwLockWriteGuard};
use std::sync::Arc;
use syncio::{zxio, zxio::zxio_get_posix_mode, zxio_node_attributes_t, Zxio};

use crate::errno;
use crate::error;
use crate::fd_impl_nonseekable;
use crate::fd_impl_seekable;
use crate::from_status_like_fdio;
use crate::fs::*;
use crate::logging::impossible_error;
use crate::task::*;
use crate::types::*;
use crate::vmex_resource::VMEX_RESOURCE;
pub struct RemoteFs;
impl FileSystemOps for RemoteFs {}

impl RemoteFs {
    pub fn new(root: zx::Channel, rights: u32) -> FileSystemHandle {
        // TODO: Should we use the inode_num from root rather than inventing a new number?
        FileSystem::new_with_root(RemoteFs, RemoteNode::new(root.into_handle(), rights).unwrap())
    }
}

struct RemoteNode {
    /// The underlying Zircon I/O object for this remote node.
    ///
    /// We delegate to the zxio library for actually doing I/O with remote
    /// objects, including fuchsia.io.Directory and fuchsia.io.File objects.
    /// This structure lets us share code with FDIO and other Fuchsia clients.
    zxio: Arc<syncio::Zxio>,

    /// The fuchsia.io rights for the dir handle. Subdirs will be opened with
    /// the same rights.
    rights: u32,
}

impl RemoteNode {
    fn new(handle: zx::Handle, rights: u32) -> Result<RemoteNode, Errno> {
        let zxio = Arc::new(Zxio::create(handle).map_err(|status| from_status_like_fdio!(status))?);
        Ok(RemoteNode { zxio, rights })
    }
}

pub fn create_fuchsia_pipe(kern: &Kernel, socket: zx::Socket) -> Result<FileHandle, Errno> {
    let ops = Box::new(RemotePipeObject::new(socket.into_handle())?);
    Ok(Anon::new_file(anon_fs(kern), ops, OpenFlags::RDWR))
}

fn update_into_from_attrs(info: &mut FsNodeInfo, attrs: zxio_node_attributes_t) {
    /// st_blksize is measured in units of 512 bytes.
    const BYTES_PER_BLOCK: usize = 512;

    // TODO - store these in FsNodeState and convert on fstat
    info.size = attrs.content_size as usize;
    info.storage_size = attrs.storage_size as usize;
    info.blksize = BYTES_PER_BLOCK;
    info.link_count = attrs.link_count;
}

fn get_zxio_signals_from_events(events: FdEvents) -> zxio::zxio_signals_t {
    let mut signals = zxio::ZXIO_SIGNAL_NONE;
    if events & FdEvents::POLLIN {
        signals |= zxio::ZXIO_SIGNAL_READABLE
            | zxio::ZXIO_SIGNAL_READ_DISABLED
            | zxio::ZXIO_SIGNAL_PEER_CLOSED;
    }
    if events & FdEvents::POLLOUT {
        signals |= zxio::ZXIO_SIGNAL_WRITABLE | zxio::ZXIO_SIGNAL_PEER_CLOSED;
    }
    if events & FdEvents::POLLRDHUP {
        signals |= zxio::ZXIO_SIGNAL_WRITE_DISABLED | zxio::ZXIO_SIGNAL_PEER_CLOSED;
    }
    return signals;
}

impl FsNodeOps for RemoteNode {
    fn open(&self, _node: &FsNode, _flags: OpenFlags) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(RemoteFileObject { zxio: Arc::clone(&self.zxio) }))
    }

    fn lookup(&self, node: &FsNode, name: &FsStr) -> Result<FsNodeHandle, Errno> {
        let name = std::str::from_utf8(name).map_err(|_| {
            warn!("bad utf8 in pathname! remote filesystems can't handle this");
            EINVAL
        })?;
        let zxio = Arc::new(
            self.zxio
                .open(self.rights, 0, name)
                .map_err(|status| from_status_like_fdio!(status))?,
        );

        // TODO: It's unfortunate to have another round-trip. We should be able
        // to set the mode based on the information we get during open.
        let attrs = zxio.attr_get().map_err(|status| from_status_like_fdio!(status))?;

        let ops = Box::new(RemoteNode { zxio, rights: self.rights });
        let mode =
            FileMode::from_bits(unsafe { zxio_get_posix_mode(attrs.protocols, attrs.abilities) });

        // TODO: Consider using attrs.id for inode_num. We need to decide
        // whether to trust the remote to use uniquie inode numbers or to continue
        // to generate our own unique numbers.
        let child = node.fs().create_node(ops, mode);

        update_into_from_attrs(&mut child.info_write(), attrs);
        Ok(child)
    }

    fn truncate(&self, _node: &FsNode, length: u64) -> Result<(), Errno> {
        self.zxio.truncate(length).map_err(|status| from_status_like_fdio!(status))
    }

    fn update_info<'a>(&self, node: &'a FsNode) -> Result<RwLockReadGuard<'a, FsNodeInfo>, Errno> {
        let attrs = self.zxio.attr_get().map_err(|status| from_status_like_fdio!(status))?;
        let mut info = node.info_write();
        update_into_from_attrs(&mut info, attrs);
        Ok(RwLockWriteGuard::downgrade(info))
    }
}

fn zxio_read(zxio: &Zxio, task: &Task, data: &[UserBuffer]) -> Result<usize, Errno> {
    let total = UserBuffer::get_total_length(data);
    let mut bytes = vec![0u8; total];
    let actual = zxio.read(&mut bytes).map_err(|status| from_status_like_fdio!(status))?;
    task.mm.write_all(data, &bytes[0..actual])?;
    Ok(actual)
}

fn zxio_read_at(
    zxio: &Zxio,
    task: &Task,
    offset: usize,
    data: &[UserBuffer],
) -> Result<usize, Errno> {
    let total = UserBuffer::get_total_length(data);
    let mut bytes = vec![0u8; total];
    let actual =
        zxio.read_at(offset as u64, &mut bytes).map_err(|status| from_status_like_fdio!(status))?;
    task.mm.write_all(data, &bytes[0..actual])?;
    Ok(actual)
}

fn zxio_write(zxio: &Zxio, task: &Task, data: &[UserBuffer]) -> Result<usize, Errno> {
    let total = UserBuffer::get_total_length(data);
    let mut bytes = vec![0u8; total];
    task.mm.read_all(data, &mut bytes)?;
    let actual = zxio.write(&bytes).map_err(|status| from_status_like_fdio!(status))?;
    Ok(actual)
}

fn zxio_write_at(
    zxio: &Zxio,
    task: &Task,
    offset: usize,
    data: &[UserBuffer],
) -> Result<usize, Errno> {
    let total = UserBuffer::get_total_length(data);
    let mut bytes = vec![0u8; total];
    task.mm.read_all(data, &mut bytes)?;
    let actual =
        zxio.write_at(offset as u64, &bytes).map_err(|status| from_status_like_fdio!(status))?;
    Ok(actual)
}

fn zxio_wait_async(
    zxio: &Zxio,
    waiter: &Arc<Waiter>,
    events: FdEvents,
    handler: Option<WaitHandler>,
) {
    let (handle, signals) = zxio.wait_begin(get_zxio_signals_from_events(events));
    waiter.wake_and_call_on(&handle, signals, handler).unwrap();
}

struct RemoteFileObject {
    /// The underlying Zircon I/O object.
    ///
    /// Shared with RemoteNode.
    zxio: Arc<syncio::Zxio>,
}

impl FileOps for RemoteFileObject {
    fd_impl_seekable!();

    fn read_at(
        &self,
        _file: &FileObject,
        task: &Task,
        offset: usize,
        data: &[UserBuffer],
    ) -> Result<usize, Errno> {
        zxio_read_at(&self.zxio, task, offset, data)
    }

    fn write_at(
        &self,
        _file: &FileObject,
        task: &Task,
        offset: usize,
        data: &[UserBuffer],
    ) -> Result<usize, Errno> {
        zxio_write_at(&self.zxio, task, offset, data)
    }

    fn get_vmo(
        &self,
        _file: &FileObject,
        _task: &Task,
        mut prot: zx::VmarFlags,
    ) -> Result<zx::Vmo, Errno> {
        let has_execute = prot.contains(zx::VmarFlags::PERM_EXECUTE);
        prot -= zx::VmarFlags::PERM_EXECUTE;
        let (mut vmo, _size) =
            self.zxio.vmo_get(prot).map_err(|status| from_status_like_fdio!(status))?;
        if has_execute {
            vmo = vmo.replace_as_executable(&VMEX_RESOURCE).map_err(impossible_error)?;
        }
        Ok(vmo)
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        waiter: &Arc<Waiter>,
        events: FdEvents,
        handler: Option<WaitHandler>,
    ) {
        zxio_wait_async(&self.zxio, waiter, events, handler)
    }
}

struct RemotePipeObject {
    /// The underlying Zircon I/O object.
    ///
    /// Shared with RemoteNode.
    zxio: Arc<syncio::Zxio>,
}

impl RemotePipeObject {
    fn new(handle: zx::Handle) -> Result<RemotePipeObject, Errno> {
        let zxio = Arc::new(Zxio::create(handle).map_err(|status| from_status_like_fdio!(status))?);
        Ok(RemotePipeObject { zxio })
    }
}

impl FileOps for RemotePipeObject {
    fd_impl_nonseekable!();

    fn read(&self, _file: &FileObject, task: &Task, data: &[UserBuffer]) -> Result<usize, Errno> {
        zxio_read(&self.zxio, task, data)
    }

    fn write(&self, _file: &FileObject, task: &Task, data: &[UserBuffer]) -> Result<usize, Errno> {
        zxio_write(&self.zxio, task, data)
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        waiter: &Arc<Waiter>,
        events: FdEvents,
        handler: Option<WaitHandler>,
    ) {
        zxio_wait_async(&self.zxio, waiter, events, handler)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl::endpoints::Proxy;
    use fidl_fuchsia_io as fio;
    use fuchsia_async as fasync;

    use crate::errno;
    use crate::mm::PAGE_SIZE;
    use crate::syscalls::*;
    use crate::testing::*;

    #[::fuchsia::test]
    async fn test_tree() -> Result<(), anyhow::Error> {
        let (_kernel, task_owner) = create_kernel_and_task();
        let rights = fio::OPEN_RIGHT_READABLE | fio::OPEN_RIGHT_EXECUTABLE;
        let root = io_util::directory::open_in_namespace("/pkg", rights)?;
        let fs = RemoteFs::new(root.into_channel().unwrap().into_zx_channel(), rights);
        let ns = Namespace::new(fs.clone());
        let root = ns.root();
        let mut context = LookupContext::default();
        assert_eq!(root.lookup(&mut context, &task_owner.task, b"nib").err(), Some(errno!(ENOENT)));
        let mut context = LookupContext::default();
        root.lookup(&mut context, &task_owner.task, b"lib").unwrap();

        let mut context = LookupContext::default();
        let _test_file = root
            .lookup(&mut context, &task_owner.task, b"bin/hello_starnix")?
            .open(OpenFlags::RDONLY)?;
        Ok(())
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_blocking_io() -> Result<(), anyhow::Error> {
        let (kernel, task_owner) = create_kernel_and_task();
        let ctx = SyscallContext::new(&task_owner.task);
        let task = Arc::clone(ctx.task);

        let address = map_memory(&ctx, UserAddress::default(), *PAGE_SIZE);
        let (client, server) = zx::Socket::create(zx::SocketOpts::empty())?;
        let pipe = create_fuchsia_pipe(&kernel, client)?;

        let thread = std::thread::spawn(move || {
            assert_eq!(64, pipe.read(&task, &[UserBuffer { address, length: 64 }]).unwrap());
        });

        // Wait for the thread to become blocked on the read.
        zx::Duration::from_seconds(2).sleep();

        let bytes = [0u8; 64];
        assert_eq!(64, server.write(&bytes)?);

        // The thread should unblock and join us here.
        let _ = thread.join();

        Ok(())
    }
}
