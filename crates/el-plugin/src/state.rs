//! Plugin-wide session state.
//!
//! The host gives a plugin a *document snapshot* per dispatch, not a live
//! handle on the app. Anything that must outlive a single command — the last
//! inspected entity, output that could not be printed where it was produced —
//! therefore lives here, in process-global state guarded by a mutex.

use std::sync::{Arc, Mutex};

use ocs_plugin_api::host::codec::EntityType;
use ocs_plugin_api::host::HostApi;
use ocs_plugin_api::host::PluginRequestSender;
use ocs_plugin_api::ipc::protocol::PluginRequest;
use ocs_plugin_api::ipc::protocol::PluginResponse;


/// The entity most recently inspected or named, so `@SEL` in a later command
/// resolves without a second pick.
static LAST_TARGET: Mutex<Option<u64>> = Mutex::new(None);

/// Pin the entity a subsequent `@SEL` should resolve to.
pub fn set_last_target(handle: u64) {
    if let Ok(mut guard) = LAST_TARGET.lock() {
        *guard = Some(handle);
    }
}

pub fn last_target() -> Option<u64> {
    LAST_TARGET.lock().ok().and_then(|g| *g)
}

pub fn clear_last_target() {
    if let Ok(mut guard) = LAST_TARGET.lock() {
        *guard = None;
    }
}

/// Resolve the `@SEL` / `LAST` token. `Ok(None)` means the token was the
/// selection alias but nothing is pinned yet — a distinct case from "not the
/// alias at all", so the caller can name the real problem instead of reporting
/// an unknown entity.
pub fn resolve_sel_token(token: &str) -> Option<Option<u64>> {
    let upper = token.trim().to_ascii_uppercase();
    if upper == "@SEL" || upper == "LAST" {
        return Some(last_target());
    }
    None
}

// ── deferred command-line output ─────────────────────────────────────────
//
// Interactive callbacks (`on_point`, `on_object_pick`, `on_enter`) run against
// the host's `NullHost` stub, where every `push_*` is dropped. A command that
// reports something on pick — `EL_INFO`, a properties readout — therefore
// cannot print where it discovered the answer. These lines queue here and are
// flushed either through the registered request sender (immediately, so the
// user sees the result without typing another command) or, failing that, at
// the top of the next dispatch.

/// One deferred line, carrying the severity it should surface at.
#[derive(Debug, Clone, PartialEq)]
pub enum PendingLine {
    Output(String),
    Error(String),
}

static PENDING_LINES: Mutex<Vec<PendingLine>> = Mutex::new(Vec::new());
static SENDER: Mutex<Option<Arc<dyn PluginRequestSender>>> = Mutex::new(None);

/// Remember the host request sender so interactive callbacks can reach the
/// host even though their own `HostApi` is a stub. Called at every dispatch.
pub fn register_sender(sender: Arc<dyn PluginRequestSender>) {
    if let Ok(mut guard) = SENDER.lock() {
        *guard = Some(sender);
    }
}

pub fn get_sender() -> Option<Arc<dyn PluginRequestSender>> {
    SENDER.lock().ok().and_then(|g| g.clone())
}

/// Forget the request channel. Test-only: a live plugin keeps the channel it
/// was handed at dispatch.
#[cfg(test)]
pub fn clear_sender() {
    if let Ok(mut g) = SENDER.lock() {
        *g = None;
    }
}

/// Queue a line for the command line.
pub fn push_deferred(line: PendingLine) {
    if let Ok(mut guard) = PENDING_LINES.lock() {
        guard.push(line);
    }
}

/// Publish one line right now: through the request sender when we have one
/// (works from an interactive callback), otherwise into the queue.
pub fn emit(line: PendingLine) {
    let Some(sender) = get_sender() else {
        push_deferred(line);
        return;
    };
    let request = match &line {
        PendingLine::Output(t) => PluginRequest::PushOutput(t.clone()),
        PendingLine::Error(t) => PluginRequest::PushError(t.clone()),
    };
    // A transport failure *or* a host-side rejection (the host parks or refuses
    // requests it cannot service now and answers with an error) means the line
    // was not printed; keep the text so the next dispatch still shows it rather
    // than losing the result silently.
    let rejected = matches!(
        sender.request(request),
        Err(_) | Ok(PluginResponse::Error(_))
    );
    if rejected {
        push_deferred(line);
    }
}

/// Flush queued lines through a real host. Dispatch context only.
pub fn flush_lines(host: &mut dyn HostApi) -> usize {
    let queued = match PENDING_LINES.lock() {
        Ok(mut g) => std::mem::take(&mut *g),
        Err(_) => return 0,
    };
    for line in &queued {
        match line {
            PendingLine::Output(t) => host.push_output(t),
            PendingLine::Error(t) => host.push_error(t),
        }
    }
    queued.len()
}

/// Number of lines waiting to be flushed (diagnostics, and the probe's
/// session-hygiene check).
pub fn pending_len() -> usize {
    PENDING_LINES.lock().map(|g| g.len()).unwrap_or(0)
}

pub fn clear_pending() {
    if let Ok(mut g) = PENDING_LINES.lock() {
        g.clear();
    }
}

// ── deferred entity writes ───────────────────────────────────────────────
//
// `CommandStep` carries exactly one entity per step, and the host has no
// plugin-visible equivalent of its internal `CommitEntities`. An interactive
// command that produces several entities — a ladder is a bus, rungs and a
// reference per rung — therefore cannot commit them all through the step
// return.
//
// A nested request made from *inside* a callback is worse than useless: the
// host services it with `NullHost`, so `AddEntities` silently no-ops. What does
// work is asking from a thread, once the callback has returned and the host is
// back in its own event loop. That is the mechanism below, and the short delay
// is what keeps the request outside the callback's lifetime.

static PENDING_ENTITIES: Mutex<Vec<EntityType>> = Mutex::new(Vec::new());

/// Queue entities to be added once a real host is available.
pub fn push_entities(entities: Vec<EntityType>) {
    if let Ok(mut g) = PENDING_ENTITIES.lock() {
        g.extend(entities);
    }
}

fn take_entities() -> Vec<EntityType> {
    PENDING_ENTITIES
        .lock()
        .map(|mut g| std::mem::take(&mut *g))
        .unwrap_or_default()
}

/// Apply everything queued through a real host. Dispatch context only.
///
/// This is the safety net for a deferred flush that never reached the host —
/// a dropped channel, a shutdown mid-flight — so queued work is delivered by
/// the next command rather than lost.
pub fn flush(host: &mut dyn HostApi) -> usize {
    let entities = take_entities();
    let n = entities.len();
    if n > 0 {
        host.add_entities(entities);
        host.set_dirty();
    }
    n
}

/// Publish queued entities from a background thread, so a placement that
/// finished inside an interactive callback still reaches the document.
///
/// `undo_label` opens an undo window for the whole batch; pass `None` for
/// purely cosmetic work (a label or a helper glyph) that should not become its
/// own undo step.
pub fn spawn_flush(undo_label: Option<String>) {
    let Some(sender) = get_sender() else {
        // No channel — leave the queue for the next dispatch to pick up.
        return;
    };
    let entities = take_entities();
    if entities.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        // The delay is load-bearing: a request issued while the host is still
        // inside our callback is serviced by `NullHost` and does nothing.
        std::thread::sleep(std::time::Duration::from_millis(60));
        if let Some(label) = undo_label {
            let _ = sender.request(PluginRequest::PushUndo { label });
        }
        let _ = sender.request(PluginRequest::AddEntities(entities));
        let _ = sender.request(PluginRequest::SetDirty);
    });
}

#[cfg(test)]
pub mod test_support {
    use super::*;

    /// `LAST_TARGET` is process-global, so tests that touch it must serialise.
    /// Recover from a poisoned guard rather than cascading one failure into
    /// every later test.
    pub static LAST_TARGET_LOCK: Mutex<()> = Mutex::new(());

    pub fn lock() -> std::sync::MutexGuard<'static, ()> {
        LAST_TARGET_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// A sender that records what it was asked to do, so the deferred paths are
    /// testable without a host process.
    #[derive(Default)]
    pub struct MockSender {
        pub sent: Mutex<Vec<&'static str>>,
    }

    impl PluginRequestSender for MockSender {
        fn request(
            &self,
            req: PluginRequest,
        ) -> Result<ocs_plugin_api::ipc::protocol::PluginResponse, ocs_plugin_api::host::PluginRequestError>
        {
            let kind = match req {
                PluginRequest::AddEntities(_) => "add_entities",
                PluginRequest::PushUndo { .. } => "push_undo",
                PluginRequest::SetDirty => "set_dirty",
                PluginRequest::RemoveEntity { .. } => "remove",
                PluginRequest::WriteRecord { .. } => "write_record",
                _ => "other",
            };
            if let Ok(mut g) = self.sent.lock() {
                g.push(kind);
            }
            Ok(ocs_plugin_api::ipc::protocol::PluginResponse::Ok)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{lock, MockSender};
    use super::*;
    use ocs_plugin_api::host::codec::Point;
    use ocs_plugin_api::host::codec::types::Vector3;

    fn a_point() -> EntityType {
        EntityType::Point(Point::at(Vector3::new(1.0, 2.0, 0.0)))
    }

    #[test]
    fn sel_token_round_trips() {
        let _g = lock();
        clear_last_target();
        assert_eq!(resolve_sel_token("@SEL"), Some(None));
        set_last_target(42);
        assert_eq!(resolve_sel_token("@sel"), Some(Some(42)));
        assert_eq!(resolve_sel_token("LAST"), Some(Some(42)));
        assert_eq!(resolve_sel_token("K1"), None);
        clear_last_target();
    }

    #[test]
    fn clear_resets_the_pin() {
        let _g = lock();
        set_last_target(7);
        clear_last_target();
        assert_eq!(last_target(), None);
    }

    #[test]
    fn deferred_lines_queue_when_no_sender_is_registered() {
        let _g = lock();
        clear_sender();
        clear_pending();
        // No sender, so `emit` must queue rather than drop.
        emit(PendingLine::Output("K1 relay".to_string()));
        assert_eq!(pending_len(), 1);
        clear_pending();
    }

    #[test]
    fn queued_entities_survive_until_flushed() {
        let _g = lock();
        let _ = take_entities();
        push_entities(vec![a_point(), a_point(), a_point()]);
        assert_eq!(take_entities().len(), 3);
        assert!(take_entities().is_empty(), "taking must drain");
    }

    #[test]
    fn a_spawned_flush_without_a_sender_leaves_the_queue_intact() {
        // Dropping the work silently would be the worst outcome; keeping it
        // means the next dispatch still delivers it.
        let _g = lock();
        clear_sender();
        let _ = take_entities();
        push_entities(vec![a_point()]);
        spawn_flush(None);
        assert_eq!(take_entities().len(), 1, "no sender yet, so nothing may be lost");
    }

    #[test]
    fn a_spawned_flush_delivers_through_the_sender() {
        let _g = lock();
        let _ = take_entities();
        // Send an undo label plus an entity, and check both reach the host.
        let mock = Arc::new(MockSender::default());
        register_sender(mock.clone());
        push_entities(vec![a_point()]);
        spawn_flush(Some("EL_LADDER".to_string()));
        // The flush is deliberately delayed, so give it time to land.
        for _ in 0..40 {
            if mock.sent.lock().map(|g| g.len()).unwrap_or(0) >= 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let sent = mock.sent.lock().unwrap().clone();
        assert!(sent.contains(&"push_undo"), "the batch needs an undo window: {sent:?}");
        assert!(sent.contains(&"add_entities"), "the entity must be added: {sent:?}");
        assert!(sent.contains(&"set_dirty"), "the drawing must be marked dirty: {sent:?}");
        assert!(take_entities().is_empty(), "a successful flush drains the queue");
    }

    #[test]
    fn a_cosmetic_flush_opens_no_undo_window() {
        let _g = lock();
        let _ = take_entities();
        let mock = Arc::new(MockSender::default());
        register_sender(mock.clone());
        push_entities(vec![a_point()]);
        spawn_flush(None);
        for _ in 0..40 {
            if mock.sent.lock().map(|g| g.len()).unwrap_or(0) >= 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let sent = mock.sent.lock().unwrap().clone();
        assert!(
            !sent.contains(&"push_undo"),
            "a label must not become its own undo step: {sent:?}"
        );
        assert!(sent.contains(&"add_entities"));
    }
}
