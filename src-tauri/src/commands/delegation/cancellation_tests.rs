use super::*;
use crate::agent_control::{CallerPrincipal, ControlAccess, ControlCaller, NativeControlState};
use crate::delegation::authority::{on_next_runtime_lookup, NativeAuthority};
use std::cell::RefCell;
use std::sync::{mpsc, Arc};

type Hook = RefCell<Option<Box<dyn FnOnce()>>>;
thread_local! {
    static AFTER_ADMISSION: Hook = Default::default();
    static BEFORE_EFFECTS: Hook = Default::default();
}

pub(super) fn after_admission() {
    AFTER_ADMISSION.with(|slot| {
        let hook = slot.borrow_mut().take();
        if let Some(hook) = hook {
            hook();
        }
    });
}

pub(super) fn before_effects() {
    BEFORE_EFFECTS.with(|slot| {
        let hook = slot.borrow_mut().take();
        if let Some(hook) = hook {
            hook();
        }
    });
}

const DEADLINE: Duration = Duration::from_secs(5);

fn live_authority(thread: &str) -> Arc<NativeAuthority> {
    let owner = NativeAuthority::with_policy(thread, ProviderKind::Claude, Some("default"));
    assert!(owner.bind(owner.begin(0), "turn"));
    owner
}

#[test]
fn native_callers_nested_cancel_parent_stop_completes_without_deadlock() {
    let app = tauri::test::mock_app();
    app.manage(DatabaseStore::new_in_memory());
    app.manage(DelegationState::default());
    app.manage(NativeControlState::default());
    app.manage(super::super::agent_chat::ProviderRegistry::new());
    let observability = crate::observability::ObservabilityStore::default();
    let mut flags = observability.feature_flags();
    flags.enable_agent_chat = true;
    observability.set_feature_flags(flags);
    app.manage(observability);

    let parent = uuid::Uuid::new_v4().to_string();
    let child = format!("{parent}-child");
    let descendant = format!("{parent}-descendant");
    let owners: Vec<_> = [&parent, &child, &descendant]
        .map(|id| live_authority(id))
        .into();
    let caller = HostCaller {
        control: Some(ControlCaller {
            identity: CallerPrincipal::Thread {
                thread_id: parent.clone(),
                provider_instance_id: "claude".into(),
                provider: ProviderKind::Claude,
                session_id: "session".into(),
                authority: owners[0].capture().identity().unwrap(),
                ceiling: crate::agent_control::permissions::Ceiling::for_mode(
                    ProviderKind::Claude,
                    Some("default"),
                )
                .unwrap(),
            },
            admitted: None,
            created_by: crate::agent_provider::message::CreatedBy::Agent,
            principal: format!("thread:{parent}"),
            access: ControlAccess::Supervised,
            grant_id: None,
        }),
        thread_id: parent.clone(),
        provider: ProviderKind::Claude,
        workspace_id: None,
        permission_mode: Some("default".into()),
    };
    let db = app.state::<DatabaseStore>();
    for (id, parent, child) in [("first", &parent, &child), ("second", &child, &descendant)] {
        let mut task = TaskRecord::new(
            id.into(),
            parent.clone(),
            Some(child.clone()),
            "claude".into(),
            "Nested task".into(),
            "A complete nested task brief".into(),
            1,
        );
        // Published children can still have live authority. Keep their result
        // while revoking it, without waiting for any provider acknowledgement.
        task.status = TaskStatus::Completed;
        task.result = Some(PublishedResult {
            version: 1,
            run_id: Some("turn".into()),
            text: format!("Saved result {id}"),
            status: TaskStatus::Completed,
            published_at: 1,
        });
        db.admit_delegation_task(&task, None, &json!({})).unwrap();
    }
    let gates: Vec<_> = [&child, &descendant]
        .map(|id| app.state::<NativeControlState>().thread_gate(id))
        .into();
    let generations: Vec<_> = gates.iter().map(|gate| gate.generation()).collect();
    let (admitted_tx, admitted_rx) = mpsc::channel();
    let (resolved_tx, resolved_rx) = mpsc::channel();
    let (effects_tx, effects_rx) = mpsc::channel();
    let (fenced_tx, fenced_rx) = mpsc::channel();
    let (cancel_tx, cancel_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel();

    let handle = app.handle().clone();
    let cancel = std::thread::spawn(move || {
        AFTER_ADMISSION.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                admitted_tx.send(()).unwrap();
                resolved_rx
                    .recv_timeout(DEADLINE)
                    .expect("Stop must resolve the parent while admission is held");
            }))
        });
        BEFORE_EFFECTS.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                effects_tx.send(()).unwrap();
                fenced_rx.recv_timeout(DEADLINE).expect(
                    "Stop must acquire parent authority before cancellation effects finish",
                );
            }))
        });
        let result = DelegationHost::new(handle)
            .call(
                &caller,
                "mcp__codemux__task_cancel",
                &json!({"task_id":"first"}),
            )
            .unwrap();
        cancel_tx.send(result).unwrap();
    });
    admitted_rx.recv_timeout(DEADLINE).unwrap();
    let handle = app.handle().clone();
    let stop = std::thread::spawn(move || {
        on_next_runtime_lookup(move || {
            resolved_tx.send(()).unwrap();
            effects_rx
                .recv_timeout(DEADLINE)
                .expect("Cancellation must release its admission before live effects");
        });
        crate::delegation::app::user_activity(&handle, &parent, true).unwrap();
        fenced_tx.send(()).unwrap();
        on_thread_stopped(&handle, &parent, StopKind::Stop);
        stop_tx.send(()).unwrap();
    });
    let result = cancel_rx
        .recv_timeout(DEADLINE)
        .expect("nested task_cancel deadlocked with parent Stop");
    assert_ne!(result["isError"], true, "{result}");
    stop_rx
        .recv_timeout(DEADLINE)
        .expect("parent Stop deadlocked with nested task_cancel");
    cancel.join().unwrap();
    stop.join().unwrap();
    for owner in owners {
        assert!(owner.capture().identity().is_err());
    }
    for (gate, generation) in gates.iter().zip(generations) {
        assert!(gate.verify(generation).is_err());
    }
    for id in ["first", "second"] {
        let task = db.delegation_task(id).unwrap().unwrap();
        assert_eq!(task.completion_delivery, "disposed_by_stop");
        assert_eq!(task.result.unwrap().text, format!("Saved result {id}"));
    }
}

#[test]
fn native_callers_other_chat_lookup_during_blocked_authority_change() {
    use crate::delegation::authority::DispatchPermit;
    for action in ["stop", "human_send", "replacement", "capture"] {
        let thread = uuid::Uuid::new_v4().to_string();
        let other = format!("{thread}-other");
        let owner = live_authority(&thread);
        let other_owner = live_authority(&other);
        let permit = owner.capture();
        let admission = permit.admit().unwrap();
        let (resolved_tx, resolved_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let change = std::thread::spawn(move || {
            on_next_runtime_lookup(move || {
                resolved_tx.send(()).unwrap();
                release_rx.recv_timeout(DEADLINE).unwrap();
            });
            match action {
                "replacement" => {
                    NativeAuthority::new(&thread);
                }
                "capture" => {
                    NativeAuthority::current_permit(&thread);
                }
                _ => NativeAuthority::user_activity_thread(&thread, action == "stop"),
            }
            finished_tx.send(()).unwrap();
        });
        resolved_rx.recv_timeout(DEADLINE).unwrap();
        release_tx.send(()).unwrap();
        let (lookup_tx, lookup_rx) = mpsc::channel();
        let lookup = std::thread::spawn(move || {
            assert!(NativeAuthority::effective_ceiling(&other)
                .unwrap()
                .is_some());
            NativeAuthority::current_permit(&other)
                .identity()
                .unwrap()
                .verify(&other)
                .unwrap();
            drop(DispatchPermit::capture(&other).unwrap().admit().unwrap());
            lookup_tx.send(()).unwrap();
        });
        // The first chat's state remains locked throughout this lookup. A
        // registry guard retained by Stop/replacement would block this chat.
        let result = lookup_rx.recv_timeout(DEADLINE);
        drop(admission);
        result.expect("another chat's authority lookup was blocked by the registry lock");
        finished_rx.recv_timeout(DEADLINE).unwrap();
        lookup.join().unwrap();
        change.join().unwrap();
        drop(other_owner);
    }
}
