// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use {
    component_events::events::*, component_events::matcher::*, fidl_fuchsia_io as fio,
    fidl_fuchsia_sys2 as fsys, fuchsia_async as fasync, fuchsia_component as fcomponent,
};

#[fasync::run_singlethreaded(test)]
async fn test() {
    // Bind to the component manager component, causing it to start
    let realm_svc = fcomponent::client::connect_to_service::<fsys::RealmMarker>()
        .expect("Could not connect to Realm service");
    let mut child = fsys::ChildRef { name: "component_manager".to_string(), collection: None };

    // Create endpoints for the fuchsia.io.Directory protocol.
    // Component manager will connect us to the exposed directory of the component we bound to.
    // This isn't needed for this test, but we must do it anyway.
    let (_, exposed_dir) = fidl::endpoints::create_endpoints::<fio::DirectoryMarker>().unwrap();
    realm_svc
        .bind_child(&mut child, exposed_dir)
        .await
        .expect("Could not send bind_child command")
        .expect("bind_child command did not succeed");

    // Wait for the component manager to stop because of a panic
    let source = EventSource::new().expect("Could not connect to fuchsia.sys2.EventSource");
    let mut stream = source
        .take_static_event_stream("panic_test_event_stream")
        .await
        .expect("Could not take static event stream");
    let mut matcher = EventMatcher::ok().stop(Some(ExitStatusMatcher::Crash(11)));
    matcher.expect_match::<Stopped>(&mut stream).await;
}
