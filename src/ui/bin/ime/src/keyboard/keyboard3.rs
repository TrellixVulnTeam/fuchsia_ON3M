// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use {
    anyhow::{format_err, Context as _, Error},
    fidl_fuchsia_input as input, fidl_fuchsia_ui_input3 as ui_input3,
    fidl_fuchsia_ui_views as ui_views,
    fuchsia_async::{self as fasync, TimeoutExt},
    fuchsia_scenic as scenic,
    fuchsia_syslog::{fx_log_info, fx_log_warn},
    fuchsia_zircon::{self as zx, AsHandleRef},
    futures::{future, lock::Mutex, TryStreamExt},
    std::{
        collections::{HashMap, HashSet},
        convert::TryFrom,
        fmt,
        hash::{Hash, Hasher},
        sync::Arc,
    },
};

const DEFAULT_LISTENER_TIMEOUT: zx::Duration = zx::Duration::from_seconds(2);

/// Abstraction wrapper for fidl_fuchsia_ui_views::ViewRef.
/// See https://fuchsia.dev/fuchsia-src/concepts/graphics/scenic/view_ref
#[derive(Eq)]
pub(crate) struct ViewRef {
    inner: ui_views::ViewRef,
}

impl ViewRef {
    pub fn new(view_ref: ui_views::ViewRef) -> Self {
        Self { inner: view_ref }
    }
}

impl Clone for ViewRef {
    fn clone(&self) -> Self {
        let inner = scenic::duplicate_view_ref(&self.inner).expect("valid view_ref");
        Self { inner }
    }
}

/// Compares ViewRefs by underlying zx::Koid.
impl PartialEq for ViewRef {
    fn eq(&self, other: &Self) -> bool {
        self.inner.reference.as_handle_ref().get_koid()
            == other.inner.reference.as_handle_ref().get_koid()
    }
}

impl fmt::Debug for ViewRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ViewRef")
            .field("koid", &self.inner.reference.as_handle_ref().get_koid().unwrap())
            .finish()
    }
}

/// Hashes based on the underlying zx::Koid.
impl Hash for ViewRef {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let koid = self.inner.reference.as_handle_ref().get_koid().unwrap();
        koid.hash(state);
    }
}

impl TryFrom<ViewRef> for ui_views::ViewRef {
    type Error = Error;

    fn try_from(view_ref: ViewRef) -> Result<Self, Error> {
        Ok(view_ref.clone().inner)
    }
}

/// Provides implementation for fuchsia.ui.input3.Keyboard FIDL.
#[derive(Clone)]
pub struct KeyboardService {
    store: Arc<Mutex<KeyListenerStore>>,
}

/// Holder for fuchsia.ui.input3.KeyboardListener client requests.
#[derive(Default)]
struct KeyListenerStore {
    /// Listener viewrefs, keyed by self-generated monotonic id.
    /// Used to remove listeners when a client disconnects from Keyboard
    /// service.
    view_refs: HashMap<usize, ViewRef>,

    /// Last monotonic id used as a `view_refs` hash key.
    last_id: usize,

    /// Client subscribers for the Keyboard service, keyed by `ViewRef`.
    subscribers: HashMap<ViewRef, Arc<Mutex<Subscriber>>>,

    /// Currently focused View.
    focused_view: Option<ViewRef>,

    /// Currently pressed keys.
    keys_pressed: HashSet<input::Key>,
}

/// A client of fuchsia.ui.input3.Keyboard.SetListener()
struct Subscriber {
    pub view_ref: ViewRef,
    pub listener: ui_input3::KeyboardListenerProxy,
}

impl KeyboardService {
    /// Starts new instance of keyboard service.
    pub fn new() -> KeyboardService {
        KeyboardService { store: Arc::new(Mutex::new(KeyListenerStore::default())) }
    }

    /// Dispatches key event to clients.
    pub async fn handle_key_event(&mut self, event: ui_input3::KeyEvent) -> Result<bool, Error> {
        self.update_keys_pressed(&event).await;
        Ok(self.store.lock().await.dispatch_key(event).await?)
    }

    async fn update_focused_view(&self, focused_view: ViewRef) {
        self.store.lock().await.focused_view = Some(focused_view);
    }

    /// Handles event of current subscriber losing focus.
    /// Will lock KeyListenerStore and current subscriber.
    async fn handle_focus_lost(&self) {
        let mut store = self.store.lock().await;
        let focused_view = match &store.focused_view {
            Some(view_ref) => view_ref,
            None => return,
        };

        if let Some(subscriber) = store.subscribers.get(&focused_view) {
            let subscriber = subscriber.lock().await;

            // Create CANCEL events for keys are currently pressed.
            let cancel_events = store.keys_pressed.iter().map(|key| ui_input3::KeyEvent {
                key: Some(*key),
                type_: Some(ui_input3::KeyEventType::Cancel),
                ..ui_input3::KeyEvent::EMPTY
            });

            // Send the CANCEL events to the client that's about to lose focus.
            let dispatches = cancel_events.map(|event| subscriber.listener.on_key_event(event));

            if let Err(e) = future::try_join_all(dispatches).await {
                fx_log_warn!("Error sending cancel events {:?} to {:?}", e, subscriber.view_ref);
            };
        };
        store.focused_view = None;
    }

    /// New client was focused, send SYNC/CANCEL events if necessary.
    /// Will lock KeyListenerStore and current subscriber.
    async fn handle_client_focused(&self, focused_view: ViewRef) {
        let store = self.store.lock().await;
        let subscriber = match store.subscribers.get(&focused_view) {
            Some(subscriber) => subscriber,
            None => return,
        };
        let subscriber = subscriber.lock().await;

        // Create SYNC events for keys that were already pressed before the client got focus.
        let sync_events = store.keys_pressed.iter().map(|key| ui_input3::KeyEvent {
            key: Some(*key),
            type_: Some(ui_input3::KeyEventType::Sync),
            ..ui_input3::KeyEvent::EMPTY
        });

        // Send the SYNC events to the newly focused client.
        let dispatches = sync_events.map(|event| subscriber.listener.on_key_event(event));

        if let Err(e) = future::try_join_all(dispatches).await {
            fx_log_warn!("Error sending sync events {:?} to {:?}", e, subscriber.view_ref);
        };
    }

    /// Handle focus change.
    /// Updates current focused client, performs bookkeeping necessary for
    /// SYNC/CANCEL events, and dispatches those for newly focused client
    /// if necessary.
    pub(crate) async fn handle_focus_change(&self, focused_view: ViewRef) {
        self.handle_focus_lost().await;
        self.update_focused_view(focused_view.clone()).await;
        self.handle_client_focused(focused_view).await;
    }

    /// Starts serving fuchsia.ui.input3.Keyboard protocol.
    pub async fn spawn_service(
        &self,
        mut stream: ui_input3::KeyboardRequestStream,
    ) -> Result<(), Error> {
        let store = self.store.clone();
        // KeyListenerStore subscriber ids to cleanup once client disconnects.
        let mut view_ref_ids: Vec<usize> = Vec::new();
        while let Some(ui_input3::KeyboardRequest::AddListener {
            view_ref,
            listener,
            responder,
            ..
        }) = stream.try_next().await.context("error running keyboard service")?
        {
            let view_ref = ViewRef::new(view_ref);
            // Scope store modification to release store lock as soon as possible.
            {
                // Get the store lock to make sure all data is modified exclusively.
                let mut store = store.lock().await;
                store.add_new_subscriber(view_ref.clone(), listener.into_proxy()?);
                let id = store.add_view_ref(view_ref);
                view_ref_ids.push(id);
            }
            responder.send()?;
        }
        // Remove subscribers from the store.
        let mut store = store.lock().await;
        view_ref_ids
            .iter()
            .map(|i| {
                let view_ref = store.view_refs.remove(i).ok_or(format_err!(
                    "Unable to cleanup after client disconnecting: view_ref lost."
                ))?;
                store.subscribers.remove(&view_ref).ok_or(format_err!(
                    "Unable to cleanup after client disconnecting: subscriber lost."
                ))?;
                Ok(())
            })
            .collect::<Result<_, Error>>()?;
        Ok(())
    }

    async fn update_keys_pressed(&mut self, event: &ui_input3::KeyEvent) {
        let (type_, key) = match (event.type_, event.key) {
            (Some(t), Some(k)) => (t, k),
            _ => return,
        };
        let keys_pressed = &mut self.store.lock().await.keys_pressed;
        match type_ {
            ui_input3::KeyEventType::Sync | ui_input3::KeyEventType::Pressed => {
                keys_pressed.insert(key);
            }
            ui_input3::KeyEventType::Cancel | ui_input3::KeyEventType::Released => {
                keys_pressed.remove(&key);
            }
        }
    }

    pub(crate) async fn get_keys_pressed(&self) -> HashSet<input::Key> {
        self.store.lock().await.keys_pressed.clone()
    }
}

impl KeyListenerStore {
    fn add_new_subscriber(
        &mut self,
        view_ref: ViewRef,
        listener: ui_input3::KeyboardListenerProxy,
    ) {
        let subscriber = Arc::new(Mutex::new(Subscriber { view_ref: view_ref.clone(), listener }));
        self.subscribers.insert(view_ref, subscriber);
    }

    fn add_view_ref(&mut self, view_ref: ViewRef) -> usize {
        self.last_id += 1;
        self.view_refs.insert(self.last_id, view_ref);
        self.last_id
    }

    fn is_focused(&self, view_ref: &ViewRef) -> bool {
        if let Some(focused_view) = &self.focused_view {
            focused_view == view_ref
        } else {
            false
        }
    }

    async fn dispatch_key(&self, event: ui_input3::KeyEvent) -> Result<bool, Error> {
        let subscribers = self
            .subscribers
            .iter()
            .map(|(_view_ref, listener)| listener.clone())
            .into_iter()
            .map(|r| Ok(r));

        // Early exit for `try_for_each()` on error is used to propagate shortcut handle success.
        // When shortcut was handled, closure returns a `Err(KeyEventStatus::Handled)`.
        let result = futures::stream::iter(subscribers)
            .try_for_each({
                // Capture by reference, since `async` non-`move` closures with
                // arguments are not currently supported
                let event = &event;
                move |subscriber| async move {
                    let subscriber = subscriber.lock().await;
                    if !self.is_focused(&subscriber.view_ref) {
                        return Ok(());
                    }
                    let event = ui_input3::KeyEvent {
                        key: event.key,
                        modifiers: event.modifiers,
                        timestamp: event.timestamp,
                        type_: event.type_,
                        ..ui_input3::KeyEvent::EMPTY
                    };
                    let handled = subscriber
                        .listener
                        .on_key_event(event)
                        .on_timeout(fasync::Time::after(DEFAULT_LISTENER_TIMEOUT), || {
                            fx_log_info!("Key listener timeout! {:?}", subscriber.view_ref);
                            Ok(ui_input3::KeyEventStatus::NotHandled)
                        })
                        .await
                        .unwrap_or_else(|e| {
                            fx_log_info!("key listener handle error: {:?}", e);
                            ui_input3::KeyEventStatus::NotHandled
                        });
                    if handled == ui_input3::KeyEventStatus::Handled {
                        return Err(handled);
                    } else {
                        return Ok(());
                    }
                }
            })
            .await;

        match result {
            Err(ui_input3::KeyEventStatus::Handled) => Ok(true),
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        fidl_fuchsia_input as input, fuchsia_async as fasync,
        fuchsia_syslog::fx_log_err,
        futures::{future, StreamExt, TryFutureExt},
        std::iter::FromIterator,
        std::task::Poll,
    };

    type Client = (ui_views::ViewRef, ui_input3::KeyboardListenerRequestStream);

    struct Helper {
        service: KeyboardService,
        keyboard_proxy: ui_input3::KeyboardProxy,
    }

    impl Helper {
        /// Starts a new Task for a new instance of Keyboard service for tests.
        fn new() -> Self {
            let (keyboard_proxy, keyboard_request_stream) =
                fidl::endpoints::create_proxy_and_stream::<ui_input3::KeyboardMarker>()
                    .expect("Failed to create KeyboardProxy and stream.");

            let service = KeyboardService::new();
            let service_clone = service.clone();
            fuchsia_async::Task::spawn(
                async move { service_clone.spawn_service(keyboard_request_stream).await }
                    .unwrap_or_else(|e: anyhow::Error| fx_log_err!("couldn't run: {:?}", e)),
            )
            .detach();
            Helper { service, keyboard_proxy }
        }

        /// Create and setup a fake keyboard client.
        /// Returns fake client viewref and keyboard listener.
        async fn create_fake_client(&self) -> Result<Client, Error> {
            let (listener_client_end, listener) =
                fidl::endpoints::create_request_stream::<ui_input3::KeyboardListenerMarker>()?;
            let view_ref = scenic::ViewRefPair::new()?.view_ref;

            self.keyboard_proxy
                .add_listener(&mut scenic::duplicate_view_ref(&view_ref)?, listener_client_end)
                .await
                .expect("add_listener");
            Ok((view_ref, listener))
        }

        async fn create_and_focus_client(&self) -> Result<Client, Error> {
            let (view_ref, listener) = self.create_fake_client().await?;
            let wrapped_view_ref = ViewRef::new(scenic::duplicate_view_ref(&view_ref)?);
            self.service.handle_focus_change(wrapped_view_ref).await;
            Ok((view_ref, listener))
        }

        fn create_and_focus_client_sync(
            &self,
            exec: &mut fasync::Executor,
        ) -> Result<Client, Error> {
            let fut = self.create_and_focus_client();
            futures::pin_mut!(fut);

            match exec.run_until_stalled(&mut fut) {
                Poll::Ready(value) => value,
                Poll::Pending => panic!("create fake client did not complete"),
            }
        }
    }

    fn create_key_event(key: input::Key, modifiers: ui_input3::Modifiers) -> ui_input3::KeyEvent {
        ui_input3::KeyEvent {
            key: Some(key),
            modifiers: Some(modifiers),
            type_: Some(ui_input3::KeyEventType::Pressed),
            ..ui_input3::KeyEvent::EMPTY
        }
    }

    async fn expect_key_and_modifiers(
        listener: &mut ui_input3::KeyboardListenerRequestStream,
        key: input::Key,
        modifiers: ui_input3::Modifiers,
    ) {
        let listener_request = listener.next().await;
        if let Some(Ok(ui_input3::KeyboardListenerRequest::OnKeyEvent {
            event, responder, ..
        })) = listener_request
        {
            responder
                .send(ui_input3::KeyEventStatus::Handled)
                .expect("responding from key listener");
            assert_eq!(event.key, Some(key));
            assert_eq!(event.modifiers, Some(modifiers));
        } else {
            panic!("Expected key error: {:?}, got {:?}", (key, modifiers), listener_request);
        }
    }

    async fn expect_key_and_type(
        listener: &mut ui_input3::KeyboardListenerRequestStream,
        key: input::Key,
        type_: ui_input3::KeyEventType,
    ) {
        let listener_request = listener.next().await;
        if let Some(Ok(ui_input3::KeyboardListenerRequest::OnKeyEvent {
            event, responder, ..
        })) = listener_request
        {
            responder
                .send(ui_input3::KeyEventStatus::Handled)
                .expect("responding from key listener");
            assert_eq!(event.key, Some(key));
            assert_eq!(event.type_, Some(type_));
        } else {
            panic!("Expected key and type error: {:?}, got {:?}", (key, type_), listener_request);
        }
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_single_client() -> Result<(), Error> {
        let mut helper = Helper::new();

        let (_view_ref, mut listener) = helper.create_and_focus_client().await?;

        let (key, modifiers) = (input::Key::A, ui_input3::Modifiers::CapsLock);
        let dispatched_event = create_key_event(key, modifiers);

        let (was_handled, _) = future::join(
            helper.service.handle_key_event(dispatched_event),
            expect_key_and_modifiers(&mut listener, key, modifiers),
        )
        .await;

        assert_eq!(was_handled?, true);

        Ok(())
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_not_focused() -> Result<(), Error> {
        let mut helper = Helper::new();
        helper.create_fake_client().await?;

        let (key, modifiers) = (input::Key::A, ui_input3::Modifiers::CapsLock);
        let was_handled = helper.service.handle_key_event(create_key_event(key, modifiers)).await?;
        assert_eq!(was_handled, false);

        Ok(())
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_switching_focus() -> Result<(), Error> {
        let mut helper = Helper::new();

        // Create fake clients.
        let (view_ref, mut listener) = helper.create_fake_client().await?;
        let view_ref = ViewRef::new(view_ref);
        let (other_view_ref, mut other_listener) = helper.create_fake_client().await?;
        let other_view_ref = ViewRef::new(other_view_ref);

        // Set focus to the first fake client.
        helper.service.handle_focus_change(view_ref).await;

        // Scope part of the test case to release listeners and borrows once done.
        {
            let (key, modifiers) = (input::Key::A, ui_input3::Modifiers::CapsLock);
            let dispatched_event = create_key_event(key, modifiers);

            // Setup key handing for both clients.
            let expect_key_fut = expect_key_and_modifiers(&mut listener, key, modifiers);
            let expect_other_key_fut =
                expect_key_and_modifiers(&mut other_listener, key, modifiers);
            futures::pin_mut!(expect_key_fut);
            futures::pin_mut!(expect_other_key_fut);

            let (was_handled, activated_listener) = future::join(
                helper.service.handle_key_event(dispatched_event),
                // Correct listener is passed as first parameter to be resolved as Left.
                future::select(expect_key_fut, expect_other_key_fut),
            )
            .await;

            assert_eq!(was_handled?, true);
            assert!(matches!(activated_listener, future::Either::Left { .. }));
        }

        // Change focus to another client, expect CANCEL for unfocused client and SYNC for focused
        // client.
        future::join3(
            helper.service.handle_focus_change(other_view_ref),
            expect_key_and_type(&mut listener, input::Key::A, ui_input3::KeyEventType::Cancel),
            expect_key_and_type(&mut other_listener, input::Key::A, ui_input3::KeyEventType::Sync),
        )
        .await;

        // Scope part of the test case to release listeners and borrows once done.
        {
            let (key, modifiers) = (input::Key::B, ui_input3::Modifiers::NumLock);
            let dispatched_event = create_key_event(key, modifiers);

            // Setup key handing for both clients.
            let expect_key_fut = expect_key_and_modifiers(&mut listener, key, modifiers);
            let expect_other_key_fut =
                expect_key_and_modifiers(&mut other_listener, key, modifiers);
            futures::pin_mut!(expect_key_fut);
            futures::pin_mut!(expect_other_key_fut);

            let (was_handled, activated_listener) = future::join(
                helper.service.handle_key_event(dispatched_event),
                // Correct listener is passed as first parameter to be resolved as Left.
                future::select(expect_other_key_fut, expect_key_fut),
            )
            .await;

            assert_eq!(was_handled?, true);
            assert!(matches!(activated_listener, future::Either::Left { .. }));
        }

        Ok(())
    }

    #[test]
    fn test_client_timeout() -> Result<(), Error> {
        let mut exec = fasync::Executor::new_with_fake_time().unwrap();
        let mut helper = Helper::new();

        let (_view_ref, _listener) = helper.create_and_focus_client_sync(&mut exec)?;
        let (key, modifiers) = (input::Key::D, ui_input3::Modifiers::NumLock);
        let handle_fut = helper.service.handle_key_event(create_key_event(key, modifiers));

        // Do not respond to KeyboardListenerRequest to emulate client timeout.

        futures::pin_mut!(handle_fut);
        assert!(matches!(exec.run_until_stalled(&mut handle_fut), Poll::Pending));

        // Roll time forward past the listener timeout.
        let new_time = fasync::Time::from_nanos(
            exec.now().into_nanos() + DEFAULT_LISTENER_TIMEOUT.into_nanos(),
        );
        exec.set_fake_time(new_time);
        exec.wake_expired_timers();

        let result = exec.run_until_stalled(&mut handle_fut);
        assert!(matches!(result, Poll::Ready(Ok(false))));

        Ok(())
    }

    #[fasync::run_singlethreaded(test)]
    async fn update_keys_pressed() -> Result<(), Error> {
        let mut helper = Helper::new();

        // Press a key.
        let (key, modifiers) = (input::Key::A, ui_input3::Modifiers::CapsLock);
        helper.service.handle_key_event(create_key_event(key, modifiers)).await?;

        assert_eq!(helper.service.get_keys_pressed().await, HashSet::from_iter(vec![key,]));

        // Release a key.
        helper
            .service
            .handle_key_event(ui_input3::KeyEvent {
                key: Some(key),
                type_: Some(ui_input3::KeyEventType::Released),
                ..ui_input3::KeyEvent::EMPTY
            })
            .await?;

        assert_eq!(helper.service.get_keys_pressed().await, HashSet::new());

        // Trigger SYNC event.
        helper
            .service
            .handle_key_event(ui_input3::KeyEvent {
                key: Some(input::Key::LeftShift),
                type_: Some(ui_input3::KeyEventType::Sync),
                ..ui_input3::KeyEvent::EMPTY
            })
            .await?;

        assert_eq!(
            helper.service.get_keys_pressed().await,
            HashSet::from_iter(vec![input::Key::LeftShift])
        );

        // Trigger CANCEL event.
        helper
            .service
            .handle_key_event(ui_input3::KeyEvent {
                key: Some(input::Key::LeftShift),
                type_: Some(ui_input3::KeyEventType::Cancel),
                ..ui_input3::KeyEvent::EMPTY
            })
            .await?;

        assert_eq!(helper.service.get_keys_pressed().await, HashSet::new());

        Ok(())
    }
}
