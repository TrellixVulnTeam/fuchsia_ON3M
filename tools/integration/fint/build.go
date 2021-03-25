// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fint

import (
	"context"
	"encoding/json"
	"fmt"
	"sort"

	"go.fuchsia.dev/fuchsia/tools/build"
	fintpb "go.fuchsia.dev/fuchsia/tools/integration/fint/proto"
	"go.fuchsia.dev/fuchsia/tools/lib/hostplatform"
	"go.fuchsia.dev/fuchsia/tools/lib/runner"
	"google.golang.org/protobuf/types/known/structpb"
)

var (
	qemuImageNames = []string{"qemu-kernel", "zircon-a"}

	// Extra targets to build when building images. Needed for size checks and tracking.
	extraTargetsForImages = []string{
		"build/images:record_filesystem_sizes",
		"build/images:system_snapshot",
	}
)

type buildModules interface {
	Archives() []build.Archive
	GeneratedSources() []string
	Images() []build.Image
	PrebuiltBinarySets() []build.PrebuiltBinarySet
	TestSpecs() []build.TestSpec
	Tools() build.Tools
	ZBITests() []build.ZBITest
}

// Build runs `ninja` given a static and context spec. It's intended to be
// consumed as a library function.
func Build(ctx context.Context, staticSpec *fintpb.Static, contextSpec *fintpb.Context) (*fintpb.BuildArtifacts, error) {
	platform, err := hostplatform.Name()
	if err != nil {
		return nil, err
	}

	modules, err := build.NewModules(contextSpec.BuildDir)
	if err != nil {
		return nil, err
	}

	targets, artifacts, err := constructNinjaTargets(modules, staticSpec, platform)
	if err != nil {
		return nil, err
	}

	ninjaPath := thirdPartyPrebuilt(contextSpec.CheckoutDir, platform, "ninja")
	runner := &runner.SubprocessRunner{}
	if msg, err := runNinja(ctx, runner, ninjaPath, contextSpec.BuildDir, targets); err != nil {
		artifacts.FailureSummary = msg
		return artifacts, err
	}

	return artifacts, nil
}

func constructNinjaTargets(
	modules buildModules,
	staticSpec *fintpb.Static,
	platform string,
) ([]string, *fintpb.BuildArtifacts, error) {
	var targets []string
	var artifacts fintpb.BuildArtifacts

	if staticSpec.IncludeImages {
		targets = append(targets, extraTargetsForImages...)

		for _, image := range modules.Images() {
			if isTestingImage(image, staticSpec.Pave) {
				targets = append(targets, image.Path)
				imageStruct, err := toStructPB(image)
				if err != nil {
					return nil, nil, err
				}
				artifacts.BuiltImages = append(artifacts.BuiltImages, imageStruct)
			}
		}

		// TODO(fxbug.dev/43568): Remove once it is always false, and move
		// "build/images:updates" into `extraTargetsForImages`.
		if staticSpec.IncludeArchives {
			archivesToBuild := []string{
				"archive",  // Images and scripts for paving/netbooting.
				"packages", // Package metadata, blobs, and tools.
			}
			for _, archive := range modules.Archives() {
				if contains(archivesToBuild, archive.Name) && archive.Type == "tgz" {
					targets = append(targets, archive.Path)
					archiveStruct, err := toStructPB(archive)
					if err != nil {
						return nil, nil, err
					}
					artifacts.BuiltArchives = append(artifacts.BuiltArchives, archiveStruct)
				}
			}
		} else {
			targets = append(targets, "build/images:updates")
		}
	}

	if staticSpec.IncludeGeneratedSources {
		targets = append(targets, modules.GeneratedSources()...)
	}

	if staticSpec.IncludeHostTests {
		for _, testSpec := range modules.TestSpecs() {
			if testSpec.OS != "fuchsia" {
				targets = append(targets, testSpec.Path)
			}
		}
	}

	if staticSpec.IncludeZbiTests {
		for _, zbiTest := range modules.ZBITests() {
			targets = append(targets, zbiTest.Path)
		}
	}

	if staticSpec.IncludePrebuiltBinaryManifests {
		for _, manifest := range modules.PrebuiltBinarySets() {
			targets = append(targets, manifest.Manifest)
		}
	}

	if len(staticSpec.Tools) != 0 {
		// We only support specifying tools for the current platform. Tools
		// needed for other platforms can be included in the build indirectly
		// via higher-level targets.
		availableTools := modules.Tools().AsMap(platform)
		for _, toolName := range staticSpec.Tools {
			tool, ok := availableTools[toolName]
			if !ok {
				return nil, nil, fmt.Errorf("tool %q with platform %q does not exist", toolName, platform)
			}
			targets = append(targets, tool.Path)
		}
	}

	targets = append(targets, staticSpec.NinjaTargets...)

	sort.Strings(targets)
	return targets, &artifacts, nil
}

// isTestingImage determines whether an image is necessary for testing Fuchsia.
//
// The `pave` argument indicates whether images will be used for paving (as
// opposed to netbooting).
//
// If an image is used in paving or netbooting, its manifest entry will specify
// what flags to pass to the bootserver when doing so.
func isTestingImage(image build.Image, pave bool) bool {
	switch {
	case len(image.PaveZedbootArgs) != 0: // Used by catalyst.
		return true
	case pave && len(image.PaveArgs) != 0: // Used for paving.
		return true
	case !pave && len(image.NetbootArgs) != 0: // Used for netboot.
		return true
	case contains(qemuImageNames, image.Name): // Used for QEMU.
		return true
	case image.Name == "uefi-disk": // Used for GCE.
		return true
	case image.Type == "script":
		// In order for a user to provision without Zedboot the scripts are
		// needed too, so we want to include them such that artifactory can
		// upload them. This covers scripts like "pave.sh", "flash.sh", etc.
		return true
	// Allow-list a specific set of zbi images that are used for testing.
	case image.Type == "zbi" && image.Name == "overnet":
		return true
	default:
		return false
	}
}

// toStructPB converts a Go struct to a Struct protobuf. Unfortunately, short of
// using some complicated `reflect` logic, the only way to do this conversion is
// by using JSON as an intermediate format.
func toStructPB(s interface{}) (*structpb.Struct, error) {
	j, err := json.Marshal(s)
	if err != nil {
		return nil, err
	}
	var m map[string]interface{}
	if err := json.Unmarshal(j, &m); err != nil {
		return nil, err
	}
	return structpb.NewStruct(m)
}
