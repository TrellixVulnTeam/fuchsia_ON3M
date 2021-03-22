// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fidlgen_cpp

import (
	fidl "go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

//
// Code generation for unified services.
// See https://fuchsia.dev/fuchsia-src/contribute/governance/fidl/ftp/ftp-041.
//

type Service struct {
	fidl.Attributes
	DeclName
	ServiceName string
	Members     []ServiceMember
}

func (Service) Kind() declKind {
	return Kinds.Service
}

var _ Kinded = (*Service)(nil)

type ServiceMember struct {
	fidl.Attributes
	ProtocolType DeclName
	Name         string
	MethodName   string
}

func (c *compiler) compileService(val fidl.Service) Service {
	s := Service{
		Attributes:  val.Attributes,
		DeclName:    c.compileDeclName(val.Name),
		ServiceName: val.GetServiceName(),
	}

	for _, v := range val.Members {
		s.Members = append(s.Members, c.compileServiceMember(v))
	}
	return s
}

func (c *compiler) compileServiceMember(val fidl.ServiceMember) ServiceMember {
	return ServiceMember{
		Attributes:   val.Attributes,
		ProtocolType: c.compileDeclName(val.Type.Identifier),
		Name:         string(val.Name),
		MethodName:   changeIfReserved(val.Name),
	}
}
