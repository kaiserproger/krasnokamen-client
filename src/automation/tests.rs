use super::control::Core;
use crate::api::{Observation, Shared};
use parking_lot::Mutex;
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Condvar,
};
use std::time::{Duration, Instant};

pub(super) fn client() -> (Arc<Core>, mpsc::Receiver<crate::api::Request>) {
    let (sender, receiver) = mpsc::sync_channel(16);
    let observation = Observation {
        instance_id: "test".into(),
        connection_epoch: "1".into(),
        dimension_epoch: "2".into(),
        state_revision: "0".into(),
        last_applied_packet_sequence: "0".into(),
        client_tick: "0".into(),
        server_tick: None,
        resource_epoch: "1".into(),
        captured_frame_id: None,
        origin: "client_observed".into(),
        truncated: false,
        next_cursor: None,
        data: json!({}),
    };
    (
        Arc::new(Core {
            shared: Arc::new(Shared {
                observation: std::sync::Mutex::new(observation),
                changed: Condvar::new(),
                owner_wake: Arc::new(|| {}),
            }),
            sender,
            control: Mutex::new(Default::default()),
            stopped: AtomicBool::new(false),
            allow_control: true,
            profile: "test".into(),
            instance: "test".into(),
            artifact_dir: Default::default(),
        }),
        receiver,
    )
}
#[test]
fn entity_census_schema_export_matches_live_validation() {
    let args = json!({"radius":128,"limit":16,"entity_type":"minecraft:villager","cursor":""});
    super::catalog::validate("minecraft_list_entities", &args).unwrap();
    super::catalog::validate("minecraft_list_entities", &json!({
        "radius":128,"limit":16,"entity_type":"minecraft:villager","cursor":"18446744073709551615"
    })).unwrap();
    assert!(super::catalog::validate("minecraft_list_entities", &json!({"entity_type":""})).is_err());
    let exported = super::schema();
    for catalog in ["tools", "headless_tools"] {
        let tool = exported[catalog].as_array().unwrap().iter()
            .find(|tool| tool["name"] == "minecraft_list_entities").unwrap();
        assert_eq!(tool, super::catalog::validate("minecraft_list_entities", &args).unwrap());
        assert_eq!(tool["inputSchema"]["properties"]["cursor"]["minLength"], 0);
        assert_eq!(tool["inputSchema"]["properties"]["cursor"]["maxLength"], 20);
    }
}

#[test]
fn authenticated_transport_writer_is_exclusive_and_context_fenced() {
    let (core, _) = client();
    let live = core.admit("a", "1", "2").unwrap().fence.clone();
    assert_eq!(core.admit("b", "1", "2").unwrap_err(), "controller_busy");
    assert_eq!(core.admit("a", "9", "2").unwrap_err(), "stale_context");
    assert_eq!(core.admit("a", "1", "9").unwrap_err(), "stale_context");
    let renewed = core.admit("a", "1", "2").unwrap().fence.clone();
    assert!(Arc::ptr_eq(&live, &renewed));
}
#[test]
fn preemption_invalidates_pending_owner_fences_before_new_writer() {
    let (core, receiver) = client();
    let old = core.admit("a", "1", "2").unwrap().fence.clone();
    core.revoke("manual_input");
    assert!(!old.load(Ordering::Acquire));
    let cleanup = receiver
        .try_recv()
        .expect("cleanup queued before next writer");
    assert!(matches!(
        cleanup.command,
        crate::api::Command::Input(crate::api::InputIntent::ReleaseAll)
    ));
    let new = core.admit("b", "1", "2").unwrap().fence.clone();
    assert!(new.load(Ordering::Acquire));
    assert!(!Arc::ptr_eq(&old, &new));
}
#[test]
fn expired_writer_cannot_be_implicitly_renewed() {
    let (core, _) = client();
    core.admit("a", "1", "2").unwrap();
    core.control.lock().lease.as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
    assert_eq!(core.admit("a", "1", "2").unwrap_err(), "lease_expired");
    core.invalidate("expired");
    assert!(core.admit("b", "1", "2").is_ok());
}

#[test]
fn respawn_handoff_preserves_writer_for_one_expected_dimension_reset() {
    let (core, _) = client();
    let live = core.admit("a", "1", "2").unwrap().fence.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let mut control = core.control.lock();
        let lease = control.lease.as_mut().unwrap();
        lease.pending_respawn = Some(cancelled);
        lease.observe_respawn("1", "3");
        assert!(lease.pending_respawn.is_none());
    }
    let renewed = core.admit("a", "1", "3").unwrap().fence.clone();
    assert!(Arc::ptr_eq(&live, &renewed));
    assert_eq!(core.admit("a", "1", "2").unwrap_err(), "stale_context");
    core.control
        .lock()
        .lease
        .as_mut()
        .unwrap()
        .observe_respawn("1", "4");
    assert_eq!(core.admit("a", "1", "4").unwrap_err(), "stale_context");
}

#[test]
fn ui_respawn_click_preserves_lease_until_one_dimension_handoff() {
    let (core, receiver) = client();
    core.shared.observation.lock().unwrap().data = json!({"presentation_available": true});
    let live = core.admit("a", "1", "2").unwrap().fence.clone();
    let owner = std::thread::spawn(move || {
        let request = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        request.reply.send(Ok(json!({"status": "ok"}))).unwrap();
    });
    super::tools::execute(
        &core,
        "a",
        "minecraft_click_screen_button",
        &json!({
            "expected_screen_class": "net.minecraft.client.gui.screens.DeathScreen",
            "expected_title": "You Died!",
            "button_label": "Respawn"
        }),
        &Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    owner.join().unwrap();
    let (verdict, _) = Core::lease_verdict(&mut core.control.lock(), "1", "3");
    assert!(verdict.is_none(), "respawn must preserve the controller");
    assert!(Arc::ptr_eq(&live, &core.admit("a", "1", "3").unwrap().fence));
    let (verdict, _) = Core::lease_verdict(&mut core.control.lock(), "1", "4");
    assert_eq!(verdict.unwrap().0, "context_changed");
}

#[test]
fn observe_queued_before_respawn_reads_the_new_dimension() {
    let (core, receiver) = client();
    core.shared.observation.lock().unwrap().data = json!({"presentation_available": true});
    core.admit("a", "1", "2").unwrap();
    let owner_core = core.clone();
    let owner = std::thread::spawn(move || {
        let click = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        click.reply.send(Ok(json!({"status": "ok"}))).unwrap();
        let observe = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        owner_core.shared.observation.lock().unwrap().dimension_epoch = "3".into();
        let (verdict, _) = Core::lease_verdict(&mut owner_core.control.lock(), "1", "3");
        assert!(verdict.is_none());
        let result = if observe.expected_dimension_epoch.as_deref().is_some_and(|epoch| epoch != "3") {
            Err("stale_dimension_epoch".to_string())
        } else {
            Ok(json!({"dimension_epoch": "3"}))
        };
        observe.reply.send(result).unwrap();
    });
    let cancelled = Arc::new(AtomicBool::new(false));
    super::tools::execute(
        &core,
        "a",
        "minecraft_click_screen_button",
        &json!({
            "expected_screen_class": "net.minecraft.client.gui.screens.DeathScreen",
            "expected_title": "You Died!",
            "button_label": "Respawn"
        }),
        &cancelled,
    ).unwrap();
    let result = super::tools::execute(&core, "a", "minecraft_observe", &json!({}), &cancelled)
        .expect("observe must return the current dimension after the queued handoff");
    assert_eq!(result["dimension_epoch"], "3");
    owner.join().unwrap();
}

#[test]
fn writer_queued_before_dimension_change_remains_epoch_fenced() {
    let (core, receiver) = client();
    let owner = std::thread::spawn(move || {
        let write = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(write.expected_dimension_epoch.as_deref(), Some("2"));
        write.reply.send(Err("stale_dimension_epoch".into())).unwrap();
    });
    let error = super::tools::execute(
        &core, "a", "minecraft_send_chat", &json!({"message": "hello"}),
        &Arc::new(AtomicBool::new(false)),
    ).unwrap_err();
    assert_eq!(error, "stale_dimension_epoch");
    owner.join().unwrap();
}

#[test]
fn ui_respawn_authorization_expires_on_next_writer_and_rejects_other_clicks() {
    for (class, label, succeeds, next_writer, connection, dimension) in [
        ("net.minecraft.client.gui.screens.DeathScreen", "Respawn", true, true, "1", "3"),
        ("net.minecraft.client.gui.screens.DeathScreen", "Respawn", false, false, "1", "3"),
        ("net.minecraft.client.gui.screens.DeathScreen", "Quit", true, false, "1", "3"),
        ("net.minecraft.client.gui.screens.PauseScreen", "Respawn", true, false, "1", "3"),
        ("net.minecraft.client.gui.screens.DeathScreen", "Respawn", true, false, "1", "4"),
        ("net.minecraft.client.gui.screens.DeathScreen", "Respawn", true, false, "2", "3"),
    ] {
        let (core, receiver) = client();
        core.shared.observation.lock().unwrap().data = json!({"presentation_available": true});
        core.admit("a", "1", "2").unwrap();
        let owner = std::thread::spawn(move || {
            let request = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
            let result = if succeeds {
                Ok(json!({"status": "ok"}))
            } else {
                Err("button not found".to_string())
            };
            request.reply.send(result).unwrap();
        });
        let result = super::tools::execute(
            &core,
            "a",
            "minecraft_click_screen_button",
            &json!({"expected_screen_class": class, "expected_title": "title", "button_label": label}),
            &Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(result.is_ok(), succeeds);
        owner.join().unwrap();
        if next_writer {
            core.admit("a", "1", "2").unwrap();
        }
        let (verdict, _) = Core::lease_verdict(&mut core.control.lock(), connection, dimension);
        assert_eq!(verdict.unwrap().0, "context_changed");
    }
}

#[test]
fn respawn_handoff_refuses_unarmed_cancelled_switched_or_skipped_contexts() {
    for (connection, dimension, armed, cancelled) in [
        ("1", "3", false, false),
        ("1", "3", true, true),
        ("2", "3", true, false),
        ("1", "4", true, false),
    ] {
        let (core, _) = client();
        core.admit("a", "1", "2").unwrap();
        {
            let mut control = core.control.lock();
            let lease = control.lease.as_mut().unwrap();
            if armed {
                lease.pending_respawn = Some(Arc::new(AtomicBool::new(cancelled)));
            }
            lease.observe_respawn(connection, dimension);
        }
        assert_eq!(
            core.admit("a", connection, dimension).unwrap_err(),
            "stale_context"
        );
        assert!(core.admit("a", "1", "2").is_ok());
    }
}

#[test]
fn ordinary_owner_error_keeps_lease_and_queues_no_cleanup() {
    let (core, receiver) = client();
    let live = core.admit("a", "1", "2").unwrap().fence.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let owner = std::thread::spawn(move || {
        let request = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("write must reach the owner");
        let fence = request
            .automation_fence
            .clone()
            .expect("write carries a writer fence");
        request
            .reply
            .send(Err(
                "target is unreachable from the current loaded client position: 1, 2, 3"
                    .to_string(),
            ))
            .expect("owner reply");
        let no_cleanup = receiver.try_recv().is_err();
        (fence, no_cleanup)
    });
    let error = super::tools::execute(
        &core,
        "a",
        "minecraft_send_chat",
        &json!({"message": "hi"}),
        &cancelled,
    )
    .unwrap_err();
    assert!(
        error.contains("target is unreachable"),
        "unexpected: {error}"
    );
    let (fence, no_cleanup) = owner.join().expect("owner thread");
    assert!(
        Arc::ptr_eq(&fence, &live),
        "owner must receive the admitted writer fence"
    );
    assert!(
        fence.load(Ordering::Acquire),
        "ordinary owner error must not invalidate the writer fence"
    );
    assert!(
        core.control
            .lock()
            .lease
            .as_ref()
            .is_some_and(|lease| lease.session == "a"),
        "ordinary owner error must not drop the lease"
    );
    assert!(
        no_cleanup,
        "no ReleaseAll cleanup may be queued for an ordinary error"
    );
}

#[test]
fn next_write_after_plain_error_reuses_the_live_writer_fence() {
    let (core, receiver) = client();
    core.admit("a", "1", "2").unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let owner = std::thread::spawn(move || {
        let mut fences = Vec::new();
        for first in [true, false] {
            let request = receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("write must reach the owner");
            fences.push(
                request
                    .automation_fence
                    .clone()
                    .expect("write carries a writer fence"),
            );
            let reply: Result<serde_json::Value, String> = if first {
                Err("ordinary raycast does not target requested entity".to_string())
            } else {
                Ok(json!({"status": "ok"}))
            };
            request.reply.send(reply).expect("owner reply");
        }
        fences
    });
    let args = json!({"message": "hi"});
    let error =
        super::tools::execute(&core, "a", "minecraft_send_chat", &args, &cancelled).unwrap_err();
    assert!(error.contains("raycast"), "unexpected: {error}");
    super::tools::execute(&core, "a", "minecraft_send_chat", &args, &cancelled)
        .expect("next write must be admitted after a plain error");
    let fences = owner.join().expect("owner thread");
    assert_eq!(fences.len(), 2);
    assert!(
        fences.iter().all(|fence| fence.load(Ordering::Acquire)),
        "both fences must stay live"
    );
    assert!(
        Arc::ptr_eq(&fences[0], &fences[1]),
        "lease must survive a plain error so the next write reuses it"
    );
}

#[test]
fn dead_session_lease_is_reclaimable_by_a_new_session() {
    // Clock-expired: the previous MCP session is gone, nobody renews.
    let (core, _) = client();
    let old = core.admit("a", "1", "2").unwrap().fence.clone();
    core.control.lock().lease.as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
    let new = core
        .admit("b", "1", "2")
        .expect("a new session must reclaim a clock-expired lease").fence.clone();
    assert!(!old.load(Ordering::Acquire));
    assert!(!Arc::ptr_eq(&old, &new));
    assert!(new.load(Ordering::Acquire));
    assert_eq!(core.admit("a", "1", "2").unwrap_err(), "controller_busy");
    // Event-invalidated: the same recovery must apply.
    let (core, _) = client();
    let old = core.admit("a", "1", "2").unwrap().fence.clone();
    assert!(core.invalidate("transport_session_deleted"));
    assert!(!old.load(Ordering::Acquire));
    assert!(core.admit("b", "1", "2").is_ok());
}

fn is_release_all(request: &crate::api::Request) -> bool {
    matches!(
        request.command,
        crate::api::Command::Input(crate::api::InputIntent::ReleaseAll)
    )
}

#[test]
fn reclaiming_a_dead_lease_releases_held_inputs_before_the_new_writer() {
    let (core, receiver) = client();
    let old = core.admit("a", "1", "2").unwrap().fence.clone();
    core.control.lock().lease.as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
    let new = core.admit("b", "1", "2").unwrap().fence.clone();
    assert!(!old.load(Ordering::Acquire));
    assert!(new.load(Ordering::Acquire));
    let cleanup = receiver
        .try_recv()
        .expect("reclaim must queue ReleaseAll before the new writer");
    assert!(is_release_all(&cleanup));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn reclaim_with_a_full_owner_queue_refuses_until_release_all_is_delivered() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let (core, _) = client();
    let core = {
        let mut core = Arc::try_unwrap(core).ok().unwrap();
        core.sender = sender.clone();
        Arc::new(core)
    };
    let old = core.admit("a", "1", "2").unwrap().fence.clone();
    core.control.lock().lease.as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
    let filler = crate::api::Request {
        command: crate::api::Command::Input(crate::api::InputIntent::ReleaseAll),
        reply: mpsc::sync_channel(1).0,
        expected_connection_epoch: None,
        expected_dimension_epoch: None,
        automation_fence: None,
        cancelled: None,
    };
    sender.try_send(filler).unwrap();
    assert_eq!(
        core.admit("b", "1", "2").unwrap_err(),
        "controller_releasing"
    );
    assert!(!old.load(Ordering::Acquire));
    // Draining lets the helper thread deliver ReleaseAll and drop `releasing`.
    assert!(is_release_all(&receiver.recv().unwrap()));
    assert!(is_release_all(&receiver.recv().unwrap()));
    while core.control.lock().releasing != 0 {
        std::thread::yield_now();
    }
    assert!(core.admit("b", "1", "2").unwrap().fence.load(Ordering::Acquire));
}

#[test]
fn deferred_expiry_revoke_never_hits_a_writer_admitted_after_the_check() {
    let (core, receiver) = client();
    let old = core.admit("a", "1", "2").unwrap().fence.clone();
    core.control.lock().lease.as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
    // Watchdog: expiry decided under the lock...
    let (verdict, _) = Core::lease_verdict(&mut core.control.lock(), "1", "2");
    let (reason, checked) = verdict.expect("expired lease must be selected");
    assert_eq!(reason, "lease_expired");
    assert!(Arc::ptr_eq(&checked, &old));
    // ...a new session replaces the lease before the deferred revoke runs...
    let new = core.admit("b", "1", "2").unwrap().fence.clone();
    assert!(is_release_all(&receiver.try_recv().unwrap()));
    // ...and the deferred revoke must target only the lease that was checked.
    core.revoke_lease(reason, Some(&checked));
    assert!(new.load(Ordering::Acquire), "new writer fence was revoked");
    assert!(core.control.lock().lease.is_some());
    assert!(receiver.try_recv().is_err(), "no spurious ReleaseAll");
    assert!(Arc::ptr_eq(&core.admit("b", "1", "2").unwrap().fence, &new));
    // The still-current lease is revoked as before.
    core.revoke_lease("lease_expired", Some(&new));
    assert!(!new.load(Ordering::Acquire));
    assert!(is_release_all(&receiver.try_recv().unwrap()));
}


#[test]
fn validation_error_before_dispatch_never_touches_lease_or_queue() {
    let (core, receiver) = client();
    core.admit("a", "1", "2").unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let error = super::tools::execute(&core, "a", "minecraft_send_chat", &json!({}), &cancelled)
        .unwrap_err();
    assert!(error.contains("invalid_arguments"), "unexpected: {error}");
    assert!(
        receiver.try_recv().is_err(),
        "a validation error must never reach the owner queue"
    );
    assert!(
        core.control
            .lock()
            .lease
            .as_ref()
            .is_some_and(|lease| lease.session == "a"),
        "a validation error must not drop the lease"
    );
}

#[test]
fn cancelled_and_fenced_owner_errors_release_the_lease() {
    for error in [
        "cancelled",
        "cancelled: request_cancelled",
        "controller_lease_revoked",
        "lease_expired",
        "stale_connection_epoch",
    ] {
        let (core, receiver) = client();
        core.admit("a", "1", "2").unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let owner = std::thread::spawn(move || {
            let request = receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("write must reach the owner");
            request
                .reply
                .send(Err(error.to_string()))
                .expect("owner reply");
            let cleanup = receiver.recv_timeout(Duration::from_secs(10)).ok();
            cleanup.is_some()
        });
        let seen = super::tools::execute(
            &core,
            "a",
            "minecraft_send_chat",
            &json!({"message": "hi"}),
            &cancelled,
        )
        .unwrap_err();
        assert_eq!(seen, error);
        assert!(
            owner.join().expect("owner thread"),
            "a genuine session-loss error ({error}) must queue ReleaseAll cleanup"
        );
        assert!(
            core.control.lock().lease.is_none(),
            "a genuine session-loss error ({error}) must drop the lease"
        );
        assert!(core.admit("b", "1", "2").is_ok());
    }
}
#[test]
fn stale_container_state_keeps_lease_for_retry() {
    let (core, receiver) = client();
    core.admit("a", "1", "2").unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let owner = std::thread::spawn(move || {
        let request = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("write must reach the owner");
        request
            .reply
            .send(Err("stale_container_state".to_string()))
            .expect("owner reply");
        receiver.try_recv().is_err()
    });
    let seen = super::tools::execute(
        &core,
        "a",
        "minecraft_send_chat",
        &json!({"message": "hi"}),
        &cancelled,
    )
    .unwrap_err();
    assert_eq!(seen, "stale_container_state");
    assert!(
        owner.join().expect("owner thread"),
        "a per-click state race must not queue ReleaseAll cleanup"
    );
    assert!(
        core.control
            .lock()
            .lease
            .as_ref()
            .is_some_and(|lease| lease.session == "a"),
        "a per-click state race must keep the lease"
    );
}


#[test]
fn active_writer_action_survives_idle_expiry_then_returns_to_idle() {
    let (core, receiver) = client();
    let caller = core.clone();
    let worker = std::thread::spawn(move || {
        super::tools::execute(&caller, "a", "minecraft_send_chat",
            &json!({"message": "activity"}), &Arc::new(AtomicBool::new(false)))
    });
    let request = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    let fence = request.automation_fence.as_ref().unwrap().clone();
    {
        let mut control = core.control.lock();
        control.lease.as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
        let (revoke, deadline) = Core::lease_verdict(&mut control, "1", "2");
        assert!(revoke.is_none(), "an admitted owner action is not an idle writer");
        assert!(deadline.is_none(), "wait for completion, not an elapsed idle deadline");
    }
    assert_eq!(core.controller()["active"], true);
    assert_eq!(core.admit("b", "1", "2").unwrap_err(), "controller_busy");
    request.reply.send(Err("target is unreachable".into())).unwrap();
    assert_eq!(worker.join().unwrap().unwrap_err(), "target is unreachable");
    let (reason, checked) = {
        let mut control = core.control.lock();
        let lease = control.lease.as_mut().unwrap();
        assert_eq!(lease.active_actions, 0);
        assert!(lease.expires > Instant::now());
        assert!(Arc::ptr_eq(&lease.live, &fence));
        lease.expires = Instant::now() - Duration::from_secs(1);
        Core::lease_verdict(&mut control, "1", "2").0.unwrap()
    };
    assert_eq!(reason, "lease_expired");
    core.revoke_lease(reason, Some(&checked));
    assert!(!fence.load(Ordering::Acquire));
    assert!(is_release_all(&receiver.try_recv().unwrap()));
}

#[test]
fn active_writer_still_obeys_context_and_manual_preemption() {
    for reason in ["context_changed", "manual_input"] {
        let (core, receiver) = client();
        let old = core.admit("a", "1", "2").unwrap();
        core.control.lock().lease.as_mut().unwrap().expires =
            Instant::now() - Duration::from_secs(1);
        if reason == "context_changed" {
            let (revocation, _) = Core::lease_verdict(&mut core.control.lock(), "1", "3");
            let (observed, fence) = revocation.unwrap();
            assert_eq!(observed, reason);
            core.revoke_lease(observed, Some(&fence));
        } else {
            core.revoke(reason);
        }
        assert!(!old.fence.load(Ordering::Acquire));
        assert!(is_release_all(&receiver.try_recv().unwrap()));
        let new = core.admit("b", "1", "3").unwrap();
        let deadline = core.control.lock().lease.as_ref().unwrap().expires;
        drop(old);
        {
            let control = core.control.lock();
            let lease = control.lease.as_ref().unwrap();
            assert_eq!(lease.active_actions, 1);
            assert_eq!(lease.expires, deadline, "old completion must not renew a replacement");
            assert!(Arc::ptr_eq(&lease.live, &new.fence));
        }
        assert!(new.fence.load(Ordering::Acquire));
    }
}

#[test]
fn last_writer_action_ends_activity_not_an_earlier_concurrent_reply() {
    let (core, _) = client();
    let first = core.admit("a", "1", "2").unwrap();
    let second = core.admit("a", "1", "2").unwrap();
    assert!(Arc::ptr_eq(&first.fence, &second.fence));
    core.control.lock().lease.as_mut().unwrap().expires =
        Instant::now() - Duration::from_secs(1);
    drop(second);
    let (revoke, deadline) = Core::lease_verdict(&mut core.control.lock(), "1", "2");
    assert!(revoke.is_none());
    assert!(deadline.is_none());
    assert!(first.fence.load(Ordering::Acquire));
    drop(first);
    let (revoke, deadline) = Core::lease_verdict(&mut core.control.lock(), "1", "2");
    assert!(revoke.is_none());
    assert!(deadline.is_some_and(|deadline| deadline > Instant::now()));
}

#[test]
fn readonly_request_does_not_keep_an_idle_writer_alive() {
    let (core, receiver) = client();
    let fence = core.admit("a", "1", "2").unwrap().fence.clone();
    let caller = core.clone();
    let worker = std::thread::spawn(move || {
        super::tools::execute(&caller, "a", "minecraft_observe", &json!({}),
            &Arc::new(AtomicBool::new(false)))
    });
    let request = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(request.automation_fence.is_none());
    core.control.lock().lease.as_mut().unwrap().expires =
        Instant::now() - Duration::from_secs(1);
    let (reason, checked) =
        Core::lease_verdict(&mut core.control.lock(), "1", "2").0.unwrap();
    assert_eq!(reason, "lease_expired");
    core.revoke_lease(reason, Some(&checked));
    assert!(!fence.load(Ordering::Acquire));
    request.reply.send(Ok(json!({"in_play": true}))).unwrap();
    worker.join().unwrap().unwrap();
}

#[test]
fn late_cancelled_owner_result_cannot_revoke_replacement_writer() {
    let (core, receiver) = client();
    let caller = core.clone();
    let old_action = std::thread::spawn(move || {
        super::tools::execute(
            &caller,
            "a",
            "minecraft_send_chat",
            &json!({"message": "pending"}),
            &Arc::new(AtomicBool::new(false)),
        )
    });
    let request = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    let old_fence = request.automation_fence.as_ref().unwrap().clone();
    core.revoke("request_cancelled");
    assert!(!old_fence.load(Ordering::Acquire));
    assert!(is_release_all(&receiver.recv_timeout(Duration::from_secs(10)).unwrap()));
    let replacement = core.admit("b", "1", "2").unwrap();
    request.reply.send(Err("cancelled".into())).unwrap();
    assert_eq!(old_action.join().unwrap().unwrap_err(), "cancelled");
    assert!(
        replacement.fence.load(Ordering::Acquire),
        "the old owner's cancelled reply must not revoke the replacement"
    );
    let control = core.control.lock();
    let lease = control.lease.as_ref().unwrap();
    assert!(Arc::ptr_eq(&lease.live, &replacement.fence));
    assert_eq!(lease.active_actions, 1);
    drop(control);
    assert!(receiver.try_recv().is_err(), "no cleanup for the replacement");
}

#[test]
fn owner_exit_returns_unknown_result_and_signals_cancellation() {
    let (core, receiver) = client();
    let cancelled = Arc::new(AtomicBool::new(false));
    let owner = std::thread::spawn(move || {
        let request = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        drop(request.reply);
    });
    assert_eq!(
        super::tools::execute(
            &core, "a", "minecraft_send_chat", &json!({"message": "pending"}), &cancelled,
        ).unwrap_err(),
        "owner_result_unknown",
    );
    assert!(cancelled.load(Ordering::Acquire));
    owner.join().unwrap();
}

#[test]
fn screenshot_sandbox_and_idle_writer_are_preserved() {
    let (mut core, receiver) = client();
    let directory = std::env::temp_dir().join(format!(
        "krasnokamen-capture-{}", super::control::nonce().unwrap(),
    ));
    std::fs::create_dir(&directory).unwrap();
    Arc::get_mut(&mut core).unwrap().artifact_dir = directory.clone();
    core.shared.observation.lock().unwrap().data = json!({"presentation_available": true});
    let cancelled = Arc::new(AtomicBool::new(false));
    for (path, error) in [
        ("../screenshots/escape.png", "screenshot_path_escape"),
        ("not-screenshots/image.png", "screenshot_requires_artifact_screenshots_directory"),
        ("/screenshots/escape.png", "screenshot_requires_artifact_screenshots_directory"),
        ("screenshots/image.jpg", "screenshot_png_required"),
    ] {
        assert_eq!(super::tools::execute(
            &core, "a", "minecraft_screenshot", &json!({"path": path}), &cancelled,
        ).unwrap_err(), error);
        assert!(receiver.try_recv().is_err());
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            &directory, directory.join("screenshots").join("escape"),
        ).unwrap();
        assert_eq!(super::tools::execute(
            &core, "a", "minecraft_screenshot",
            &json!({"path": "screenshots/escape/image.png"}), &cancelled,
        ).unwrap_err(), "screenshot_symlink_refused");
        assert!(receiver.try_recv().is_err());
    }
    assert!(core.control.lock().lease.is_none(), "rejected captures cannot claim control");
    for (name, args) in [
        ("client_capture", json!({"name": "capture-without-writer"})),
        ("minecraft_screenshot", json!({"path": "screenshots/without-writer.png"})),
    ] {
        let caller = core.clone();
        let worker = std::thread::spawn(move || super::tools::execute(
            &caller, "a", name, &args, &Arc::new(AtomicBool::new(false)),
        ));
        let request = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(request.automation_fence.is_none());
        assert!(core.control.lock().lease.is_none());
        request.reply.send(Ok(json!({"gpu_completion": true}))).unwrap();
        worker.join().unwrap().unwrap();
        assert!(core.control.lock().lease.is_none());
    }
    let fence = core.admit("a", "1", "2").unwrap().fence.clone();
    let expiry = Instant::now() + Duration::from_secs(30);
    core.control.lock().lease.as_mut().unwrap().expires = expiry;
    let owner_core = core.clone();
    let owner_fence = fence.clone();
    let owner = std::thread::spawn(move || {
        let request = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(Arc::ptr_eq(request.automation_fence.as_ref().unwrap(), &owner_fence));
        {
            let control = owner_core.control.lock();
            let lease = control.lease.as_ref().unwrap();
            assert_eq!(lease.active_actions, 0, "capture is not writer activity");
            assert_eq!(lease.expires, expiry);
        }
        assert!(matches!(request.command, crate::api::Command::Capture { .. }));
        request.reply.send(Ok(json!({"gpu_completion": true}))).unwrap();
    });
    super::tools::execute(
        &core, "a", "minecraft_screenshot",
        &json!({"path": "screenshots/allowed.png"}), &cancelled,
    ).unwrap();
    owner.join().unwrap();
    let control = core.control.lock();
    let lease = control.lease.as_ref().unwrap();
    assert_eq!(lease.expires, expiry, "capture must not extend idle authority");
    assert_eq!(lease.active_actions, 0);
    assert!(Arc::ptr_eq(&lease.live, &fence));
    drop(control);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn capture_remains_locally_authorized_focused_and_session_fenced() {
    let (mut core, receiver) = client();
    core.shared.observation.lock().unwrap().data = json!({"presentation_available": true});
    let cancelled = Arc::new(AtomicBool::new(false));
    Arc::get_mut(&mut core).unwrap().allow_control = false;
    assert_eq!(super::tools::execute(
        &core, "a", "client_capture", &json!({"name": "denied"}), &cancelled,
    ).unwrap_err(), "control_not_locally_authorized");
    Arc::get_mut(&mut core).unwrap().allow_control = true;
    core.shared.observation.lock().unwrap().data["window"] = json!({"focused": false});
    assert_eq!(super::tools::execute(
        &core, "a", "client_capture", &json!({"name": "unfocused"}), &cancelled,
    ).unwrap_err(), "window_not_focused");
    assert!(receiver.try_recv().is_err());
    assert!(core.control.lock().lease.is_none());
    core.shared.observation.lock().unwrap().data["window"] = json!({"focused": true});
    let fence = core.admit("a", "1", "2").unwrap().fence.clone();
    assert_eq!(core.capture_fence("b", "1", "2").unwrap_err(), "controller_busy");
    assert_eq!(core.capture_fence("a", "9", "2").unwrap_err(), "stale_context");
    assert_eq!(core.capture_fence("a", "1", "9").unwrap_err(), "stale_context");
    core.control.lock().lease.as_mut().unwrap().expires =
        Instant::now() - Duration::from_secs(1);
    assert_eq!(core.capture_fence("a", "1", "2").unwrap_err(), "lease_expired");
    core.control.lock().lease.as_mut().unwrap().active_actions = 1;
    assert!(Arc::ptr_eq(&core.capture_fence("a", "1", "2").unwrap().unwrap(), &fence));
    fence.store(false, Ordering::Release);
    assert_eq!(core.capture_fence("a", "1", "2").unwrap_err(), "lease_expired");
}

#[test]
fn capture_does_not_extend_an_existing_idle_writer() {
    let (core, receiver) = client();
    core.shared.observation.lock().unwrap().data = json!({"presentation_available": true});
    let fence = core.admit("a", "1", "2").unwrap().fence.clone();
    let expiry = Instant::now() + Duration::from_secs(30);
    core.control.lock().lease.as_mut().unwrap().expires = expiry;
    let caller = core.clone();
    let worker = std::thread::spawn(move || super::tools::execute(
        &caller, "a", "client_capture", &json!({"name": "idle"}),
        &Arc::new(AtomicBool::new(false)),
    ));
    let request = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(Arc::ptr_eq(request.automation_fence.as_ref().unwrap(), &fence));
    let during = {
        let control = core.control.lock();
        let lease = control.lease.as_ref().unwrap();
        (lease.expires, lease.active_actions)
    };
    request.reply.send(Ok(json!({"gpu_completion": true}))).unwrap();
    worker.join().unwrap().unwrap();
    assert_eq!(during, (expiry, 0), "capture must not renew or extend writer activity");
    assert_eq!(core.control.lock().lease.as_ref().unwrap().expires, expiry);
}

#[test]
fn capture_after_authorized_respawn_preserves_idle_writer() {
    let (core, receiver) = client();
    let fence = core.admit("a", "1", "2").unwrap().fence.clone();
    let expiry = Instant::now() + Duration::from_secs(30);
    {
        let mut control = core.control.lock();
        let lease = control.lease.as_mut().unwrap();
        lease.expires = expiry;
        lease.pending_respawn = Some(Arc::new(AtomicBool::new(false)));
    }
    {
        let mut observation = core.shared.observation.lock().unwrap();
        observation.data = json!({"presentation_available": true});
        observation.dimension_epoch = "3".into();
    }
    // No watchdog/admit/drop observer runs between respawn and this capture.
    let owner = std::thread::spawn(move || {
        let request = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(request.expected_dimension_epoch.as_deref(), Some("3"));
        request.reply.send(Ok(json!({"gpu_completion": true}))).unwrap();
    });
    super::tools::execute(&core, "a", "client_capture", &json!({"name": "after-respawn"}),
        &Arc::new(AtomicBool::new(false))).unwrap();
    owner.join().unwrap();
    let control = core.control.lock();
    let lease = control.lease.as_ref().unwrap();
    assert!(Arc::ptr_eq(&lease.live, &fence));
    assert_eq!(lease.dimension, "3");
    assert!(lease.pending_respawn.is_none());
    assert_eq!((lease.expires, lease.active_actions), (expiry, 0));
    drop(control);
    assert_eq!(core.capture_fence("a", "1", "4").unwrap_err(), "stale_context");
}

#[test]
fn capture_owner_errors_never_revoke_or_renew_writer() {
    let (mut core, receiver) = client();
    let directory = std::env::temp_dir().join(format!(
        "krasnokamen-capture-owner-error-{}", super::control::nonce().unwrap()));
    std::fs::create_dir(&directory).unwrap();
    Arc::get_mut(&mut core).unwrap().artifact_dir = directory.clone();
    core.shared.observation.lock().unwrap().data = json!({"presentation_available": true});
    let fence = core.admit("a", "1", "2").unwrap().fence.clone();
    let expiry = Instant::now() + Duration::from_secs(30);
    core.control.lock().lease.as_mut().unwrap().expires = expiry;
    for error in ["cancelled", "cancelled: request_cancelled", "controller_lease_revoked",
        "lease_expired", "stale_connection_epoch", "stale_dimension_epoch", "capture_cancelled"] {
        for (name, args) in [
            ("client_capture", json!({"name": "owner-error"})),
            ("minecraft_screenshot", json!({"path": "screenshots/owner-error.png"})),
        ] {
            let caller = core.clone();
            let worker = std::thread::spawn(move || super::tools::execute(
                &caller, "a", name, &args, &Arc::new(AtomicBool::new(false)),
            ));
            let request = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
            assert!(Arc::ptr_eq(request.automation_fence.as_ref().unwrap(), &fence));
            request.reply.send(Err(error.into())).unwrap();
            assert_eq!(worker.join().unwrap().unwrap_err(), error);
            let control = core.control.lock();
            let lease = control.lease.as_ref().expect("capture error revoked writer");
            assert!(Arc::ptr_eq(&lease.live, &fence));
            assert!(fence.load(Ordering::Acquire));
            assert_eq!((lease.expires, lease.active_actions), (expiry, 0));
            assert!(receiver.try_recv().is_err(), "capture error queued ReleaseAll");
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
