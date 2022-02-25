#!/usr/bin/env python3.8
# Copyright 2017 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Build script for a Go app.

import argparse
import errno
import json
import os
import shutil
import subprocess
import sys

from gen_library_metadata import get_sources

FUCHSIA_MODULE = 'go.fuchsia.dev/fuchsia'


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        '--root-out-dir', help='Path to root of build output', required=True)
    parser.add_argument(
        '--cc', help='The C compiler to use', required=False, default='cc')
    parser.add_argument(
        '--cxx', help='The C++ compiler to use', required=False, default='c++')
    parser.add_argument(
        '--ar', help='The archive linker to use', required=False, default='ar')
    parser.add_argument(
        '--dump-syms', help='The dump_syms tool to use', required=False)
    parser.add_argument(
        '--objcopy',
        help='The objcopy tool to use',
        required=False,
        default='objcopy')
    parser.add_argument('--sysroot', help='The sysroot to use', required=False)
    parser.add_argument(
        '--target', help='The compiler target to use', required=False)
    parser.add_argument(
        '--depfile', help='The path to the depfile', required=False)
    parser.add_argument(
        '--current-cpu',
        help='Target architecture.',
        choices=['x64', 'arm64', 'riscv64'],
        required=True)
    parser.add_argument(
        '--current-os',
        help='Target operating system.',
        choices=['fuchsia', 'linux', 'mac', 'win'],
        required=True)
    parser.add_argument('--buildidtool', help='The path to the buildidtool.')
    parser.add_argument(
        '--build-id-dir', help='The path to the .build-id directory.')
    parser.add_argument(
        '--go-root', help='The go root to use for builds.', required=True)
    parser.add_argument(
        '--go-cache', help='Cache directory to use for builds.', required=False)
    parser.add_argument(
        '--golibs-dir',
        help='The directory containing third party libraries.',
        required=True)
    parser.add_argument(
        '--is-test', help='True if the target is a go test', default=False)
    parser.add_argument('--buildmode', help='Build mode to use')
    parser.add_argument(
        '--gcflag',
        help='Arguments to pass to Go compiler',
        action='append',
        default=[])
    parser.add_argument(
        '--ldflag',
        help='Arguments to pass to Go linker',
        action='append',
        default=[])
    parser.add_argument(
        '--go-dep-files',
        help='List of files describing library dependencies',
        nargs='*',
        default=[])
    parser.add_argument('--binname', help='Output file', required=True)
    parser.add_argument(
        '--output-path',
        help='Where to output the (unstripped) binary',
        required=True)
    parser.add_argument(
        '--stripped-output-path',
        help='Where to output a stripped binary, if supplied')
    parser.add_argument(
        '--verbose',
        help='Tell the go tool to be verbose about what it is doing',
        action='store_true')
    parser.add_argument('--package', help='The package name', required=True)
    parser.add_argument(
        '--include-dir',
        help='-isystem path to add',
        action='append',
        default=[])
    parser.add_argument(
        '--lib-dir', help='-L path to add', action='append', default=[])
    parser.add_argument('--vet', help='Run go vet', action='store_true')
    parser.add_argument(
        '--tag', help='Add a go build tag', default=[], action='append')
    parser.add_argument(
        '--cgo', help='Whether to enable CGo', action='store_true')
    args = parser.parse_args()

    try:
        os.makedirs(args.go_cache)
    except OSError as e:
        if e.errno == errno.EEXIST and os.path.isdir(args.go_cache):
            pass
        else:
            raise

    goarch = {
        'x64': 'amd64',
        'arm64': 'arm64',
        'riscv64': 'riscv64',
    }[args.current_cpu]
    goos = {
        'fuchsia': 'fuchsia',
        'linux': 'linux',
        'mac': 'darwin',
        'win': 'windows',
    }[args.current_os]

    dist = args.stripped_output_path or args.output_path

    # Project path is a package specific gopath, also known as a "project" in go parlance.
    project_path = os.path.join(
        args.root_out_dir, 'gen', 'gopaths', args.binname)

    # Clean up any old project path to avoid leaking old dependencies.
    gopath_src = os.path.join(project_path, 'src')
    # Manually removing all subdirectories and files instead of using
    # shutil.rmtree, to avoid registering spurious reads on stale
    # subdirectories. See https://fxbug.dev/74084.
    if os.path.exists(gopath_src):
        for root, dirs, files in os.walk(gopath_src, topdown=False):
            for file in files:
                os.unlink(os.path.join(root, file))
            for dir in dirs:
                full_path = os.path.join(root, dir)
                if os.path.islink(full_path):
                    os.unlink(full_path)
                else:
                    os.rmdir(full_path)

    dst_vendor = os.path.join(gopath_src, 'vendor')
    os.makedirs(dst_vendor)
    # Symlink interprets path against the current working directory, so use
    # absolute path for consistency.
    abs_golibs_dir = os.path.abspath(args.golibs_dir)
    for src in ['go.mod', 'go.sum']:
        os.symlink(
            os.path.join(abs_golibs_dir, src), os.path.join(gopath_src, src))
    os.symlink(
        os.path.join(os.path.join(abs_golibs_dir, 'vendor'), 'modules.txt'),
        os.path.join(dst_vendor, 'modules.txt'))

    link_to_source_list = []
    if args.go_dep_files:
        link_to_source = {}

        # Create a GOPATH for the packages dependency tree.
        for dst, src in sorted(get_sources(args.go_dep_files).items()):
            # This path is later used in go commands that run in cwd=gopath_src.
            src = os.path.abspath(src)

            # If the destination is part of the "main module", strip off the
            # module path. Otherwise, put it in the vendor directory.
            if dst.startswith(FUCHSIA_MODULE):
                dst = os.path.relpath(dst, FUCHSIA_MODULE)
            else:
                dst = os.path.join('vendor', dst)

            # Determine if the src path should
            # - be mapped as-is which, if src is a directory, includes all subdirectories
            # - have its contents enumerated and mapped directly
            map_directly = False

            if dst.endswith('/...'):
                # When a directory and all its subdirectories must be made available, map
                # the directory directly.
                map_directly = True

                # - src can have a '/...' suffix like with 'github.com/google/go-cmp/...'.
                #   - This means all subpackages are being symlinked to the GOPATH.
                # - dst have the suffix when defining a package.
                # - src can only have the suffix if dst has it too.
                assert dst.endswith('/...') >= src.endswith('/...'), (dst, src)
                dst = dst[:-4]
                if src.endswith('/...'):
                    src = src[:-4]
            elif os.path.isfile(src):
                # When sources are explicitly listed in the BUILD.gn file, each `src` will
                # be a path to a file that must be mapped directly.
                map_directly = True
                # Paths with /.../ in the middle designate go packages that include
                # subpackages, but also explicitly list all their source files.
                #
                # The construction of these paths is done in the go list invocation, so we
                # remove these sentinel values here.
                dst = dst.replace('/.../', '/')

            dstdir = os.path.join(gopath_src, dst)

            if map_directly:
                # Make a symlink to the src directory or file.
                parent = os.path.dirname(dstdir)
                if not os.path.exists(parent):
                    os.makedirs(parent)
                # hardlink regular files instead of symlinking to handle non-Go
                # files that we want to embed using //go:embed, which doesn't
                # support symlinks.
                # TODO(https://fxbug.dev/81748): Add a separate mechanism for
                # declaring embedded files, and only hardlink those files
                # instead of hardlinking all sources.
                if os.path.isdir(src):
                    os.symlink(src, dstdir)
                else:
                    try:
                        os.link(src, dstdir)
                    except OSError:
                        # Hardlinking may fail, for example if `src` is in a
                        # separate filesystem on a mounted device.
                        shutil.copyfile(src, dstdir)
                link_to_source[dstdir] = src
            else:
                # Map individual files since the dependency is only on the
                # package itself, not Go subpackages. The only exception is
                # 'testdata'.
                os.makedirs(dstdir)
                for filename in os.listdir(src):
                    src_file = os.path.join(src, filename)
                    if filename == 'testdata' or os.path.isfile(src_file):
                        os.symlink(src_file, os.path.join(dstdir, filename))
                        link_to_source[os.path.join(dstdir, filename)] = src

        # Create a sorted list of (link, src) pairs, with longest paths before
        # short one. This ensures that 'foobar' will appear before 'foo'.
        link_to_source_list = sorted(
            link_to_source.items(), key=lambda x: x[0], reverse=True)

    cflags = []
    if args.sysroot:
        cflags.extend(['--sysroot', os.path.abspath(args.sysroot)])
    if args.target:
        cflags.extend(['-target', args.target])
    if args.current_cpu == 'riscv64':
        cflags.extend(['-mno-relax'])

    ldflags = cflags[:]
    if args.current_os == 'linux':
        ldflags.extend(
            [
                '-stdlib=libc++',
                # TODO(fxbug.dev/64336): the following flags are not recognized by CGo.
                # '-rtlib=compiler-rt',
                # '-unwindlib=',
            ])

    for dir in args.include_dir:
        cflags.extend(['-isystem', os.path.abspath(dir)])
    ldflags.extend(['-L' + os.path.abspath(dir) for dir in args.lib_dir])

    cflags_joined = ' '.join(cflags)
    ldflags_joined = ' '.join(ldflags)

    build_goroot = os.path.abspath(args.go_root)

    env = {
        # /usr/bin:/bin are required for basic things like bash(1) and env(1). Note
        # that on Mac, ld is also found from /usr/bin.
        'PATH': os.path.join(build_goroot, 'bin') + ':/usr/bin:/bin',
        'GOARCH': goarch,
        'GOOS': goos,
        # GOPATH won't be used, but Go still insists that we set it. Without it,
        # Go emits the succinct error: `missing $GOPATH`. Go further insists
        # that $GOPATH/go.mod not exist; if we pass `gopath_src` here (which
        # is where we symlinked our go.mod), we get another succinct error:
        # `$GOPATH/go.mod exists but should not`. Finally, GOPATH must be
        # absolute, otherwise:
        #
        # go: GOPATH entry is relative; must be absolute path: ...
        # For more details see: 'go help gopath'
        #
        # and here we are.
        'GOPATH': os.path.abspath(project_path),
        # Disallow downloading modules from any source.
        #
        # See https://golang.org/ref/mod#environment-variables under `GOPROXY`.
        'GOPROXY': 'off',
        # Some users have GOROOT set in their parent environment, which can break
        # things, so it is always set explicitly here.
        'GOROOT': build_goroot,
        # GOCACHE, CC and CXX below may be used in different working directories
        # so they have to be absolute.
        'GOCACHE': os.path.abspath(args.go_cache),
        'AR': os.path.abspath(args.ar),
        'CC': os.path.abspath(args.cc),
        'CXX': os.path.abspath(args.cxx),
        'CGO_CFLAGS': cflags_joined,
        'CGO_CPPFLAGS': cflags_joined,
        'CGO_CXXFLAGS': cflags_joined,
        'CGO_LDFLAGS': ldflags_joined,
    }

    # Infra sets $TMPDIR which is cleaned between builds.
    if os.getenv('TMPDIR'):
        env['TMPDIR'] = os.getenv('TMPDIR')

    if args.cgo:
        env['CGO_ENABLED'] = '1'
    if args.target:
        env['CC_FOR_TARGET'] = env['CC']
        env['CXX_FOR_TARGET'] = env['CXX']

    go_tool = os.path.join(build_goroot, 'bin', 'go')

    if args.vet:
        retcode = subprocess.call(
            [go_tool, 'vet', args.package], env=env, cwd=gopath_src)
        if retcode != 0:
            return retcode

    cmd = [go_tool]
    if args.is_test:
        cmd += ['test', '-c']
    else:
        cmd += ['build', '-trimpath']
    if args.verbose:
        cmd += ['-x']
    if args.tag:
        # Separate tags by spaces. This behavior is actually deprecated in the
        # go command line, but Fuchsia currently has an older version of go
        # that hasn't switched to commas.
        cmd += ['-tags', ' '.join(args.tag)]
    if args.buildmode:
        cmd += ['-buildmode', args.buildmode]
        if args.buildmode == 'c-archive':
            if not args.ar:
                parser.error('--ar=AR is required with --buildmode=c-archive')
            args.ldflag.extend(['-extar', os.path.relpath(args.ar, gopath_src)])
    if args.gcflag:
        cmd += ['-gcflags', ' '.join(args.gcflag)]
    if args.ldflag:
        cmd += ['-ldflags=' + ' '.join(args.ldflag)]
    cmd += [
        '-pkgdir',
        os.path.join(project_path, 'pkg'),
        '-o',
        os.path.relpath(args.output_path, gopath_src),
        args.package,
    ]
    retcode = subprocess.call(cmd, env=env, cwd=gopath_src)
    if retcode != 0:
        return retcode

    if args.stripped_output_path:
        if args.current_os == 'mac':
            retcode = subprocess.call(
                [
                    'xcrun', 'strip', '-x', args.output_path, '-o',
                    args.stripped_output_path
                ],
                env=env)
        else:
            retcode = subprocess.call(
                [
                    args.objcopy, '--strip-sections', args.output_path,
                    args.stripped_output_path
                ],
                env=env)
        if retcode != 0:
            return retcode

    # TODO(fxbug.dev/27215): Also invoke the buildidtool in the case of linux
    # once buildidtool knows how to deal in Go's native build ID format.
    supports_build_id = args.current_os == 'fuchsia'
    if args.dump_syms and supports_build_id:
        if args.current_os == 'fuchsia':
            with open(dist + '.sym', 'w') as f:
                retcode = subprocess.call(
                    [args.dump_syms, '-r', '-o', 'Fuchsia', args.output_path],
                    stdout=f)
                if retcode != 0:
                    return retcode

    if args.buildidtool and supports_build_id:
        if not args.build_id_dir:
            raise ValueError('Using --buildidtool requires --build-id-dir')
        retcode = subprocess.call(
            [
                args.buildidtool,
                '-build-id-dir',
                args.build_id_dir,
                '-stamp',
                dist + '.build-id.stamp',
                '-entry',
                '.debug=' + args.output_path,
                '-entry',
                '=' + dist,
            ])
        if retcode != 0:
            return retcode

    go_list_args = [go_tool, 'list', '-json', '-deps']
    if args.is_test:
        go_list_args += ['-test']
    go_list_args += [args.package]
    output = subprocess.check_output(
        go_list_args, env=env, cwd=gopath_src, text=True)
    with open(args.depfile, 'w') as into:
        into.write(os.path.relpath(dist))
        into.write(':')
        while output:
            try:
                package = json.loads(output)
                output = output[:0]
            except json.JSONDecodeError as e:
                package = json.loads(output[:e.pos])
                output = output[e.pos:]

            files_fields = [
                'GoFiles',
                'CgoFiles',
                'CompiledGoFiles',
                'CFiles',
                'CXXFiles',
                'MFiles',
                'HFiles',
                'FFiles',
                'SFiles',
                'SwigFiles',
                'SwigCXXFiles',
                'SysoFiles',
            ]
            if 'ForTest' in package:
                files_fields += [
                    'TestGoFiles',
                    'XTestGoFiles',
                ]

            src_dir = package['Dir']
            for f, t in link_to_source_list:
                if src_dir.startswith(f):
                    src_dir = t + src_dir[len(f):]
                    break

            # When files from gocache are printed in the output they have
            # normalized absolute paths, so use absolute path to filter them.
            abs_go_cache = os.path.abspath(args.go_cache)
            src_dir = os.path.relpath(src_dir)
            for field in files_fields:
                files = package.get(field)
                if files:
                    for file in files:
                        if abs_go_cache in file:
                            continue
                        into.write(' ')
                        into.write(os.path.join(src_dir, file))

    return 0


if __name__ == '__main__':
    sys.exit(main())
