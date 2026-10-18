//! Editing and reordering a provider's follow-up queue.
//!
//! Every provider keeps queued turns in its own `VecDeque`, popped before
//! dispatch, so an item still in the queue has not started. These helpers
//! do the queue surgery once; each provider calls them under its own state
//! lock and emits [`ProviderRuntimeEvent::QueuedTurnEdited`] or
//! [`ProviderRuntimeEvent::QueuedTurnsReordered`].
//!
//! An edit swaps only the user's words. The prompt the provider receives
//! may frame them with Codemux context (mode, effort, attachments, skills);
//! that framing, the images and the item's position are kept. When the
//! user's words cannot be found in the prompt to swap safely, the edit is
//! refused rather than guessed.

use std::collections::VecDeque;

use super::{ProviderError, SendTurnInput, ThreadId};

const CANNOT_EDIT: &str =
    "This queued message can't be edited in place. Cancel it to edit it in the composer.";
pub(crate) fn cannot_edit() -> ProviderError {
    ProviderError::ValidationError {
        message: CANNOT_EDIT.into(),
    }
}

const PINNED: &str = "This message is about to send, so it can't be moved.";

/// One queued item, whatever shape its provider stores.
pub(crate) trait QueueEntry {
    fn queued_id(&self) -> &str;
    fn input(&self) -> &SendTurnInput;
    fn input_mut(&mut self) -> &mut SendTurnInput;
}

impl QueueEntry for (String, SendTurnInput) {
    fn queued_id(&self) -> &str {
        &self.0
    }
    fn input(&self) -> &SendTurnInput {
        &self.1
    }
    fn input_mut(&mut self) -> &mut SendTurnInput {
        &mut self.1
    }
}

/// `prompt` with the user's words `old` swapped for `text`, or `None` when
/// they cannot be located safely.
pub(crate) fn splice_prompt(prompt: &str, old: &str, text: &str) -> Option<String> {
    let old = old.trim();
    if prompt.trim() == old {
        return Some(text.to_owned());
    }
    if old.is_empty() {
        return None;
    }
    let command = crate::agent_provider::slash_commands::leading_command;
    // Framing ahead of a slash command would turn it into prose.
    if command(text).is_none() {
        if let Some(framing) = prompt.strip_suffix(old) {
            return Some(format!("{framing}{text}"));
        }
    }
    // A slash command keeps its attachment context after it.
    prompt
        .strip_prefix(old)
        .filter(|rest| rest.starts_with("\n\n"))
        .map(|rest| format!("{text}{rest}"))
}

/// Replace the text of `thread`'s queued item `id`. Returns the new
/// user-visible text, or `None` when the item is no longer queued.
pub(crate) fn edit<T: QueueEntry>(
    queue: &mut VecDeque<T>,
    thread: &ThreadId,
    id: &str,
    text: &str,
) -> Result<Option<String>, ProviderError> {
    let Some(entry) = queue
        .iter_mut()
        .find(|entry| entry.queued_id() == id && &entry.input().thread_id == thread)
    else {
        return Ok(None);
    };
    let text = text.trim();
    let input = entry.input_mut();
    if text.is_empty() && input.images.is_empty() {
        return Err(ProviderError::ValidationError {
            message: "A queued message can't be empty. Cancel it instead.".into(),
        });
    }
    let old = input.display_text.as_deref().unwrap_or(&input.text);
    let prompt = splice_prompt(&input.text, old, text).ok_or_else(cannot_edit)?;
    input.text = prompt;
    input.display_text = Some(text.to_owned());
    Ok(Some(text.to_owned()))
}

/// Move `thread`'s queued item `id` to just before `before`, or behind the
/// thread's last item when `before` is `None`. `pinned` is an item a held
/// queue is about to send from the front; it stays there. Returns the
/// thread's queue order afterwards, or `None` when `id` is not queued. A
/// `before` that has already left the queue leaves the order unchanged.
pub(crate) fn move_before<T: QueueEntry>(
    queue: &mut VecDeque<T>,
    thread: &ThreadId,
    id: &str,
    before: Option<&str>,
    pinned: Option<&str>,
) -> Result<Option<Vec<String>>, ProviderError> {
    let ours = |entry: &T| &entry.input().thread_id == thread;
    let Some(from) = queue
        .iter()
        .position(|entry| entry.queued_id() == id && ours(entry))
    else {
        return Ok(None);
    };
    let pinned_front = pinned.filter(|p| queue.front().is_some_and(|f| f.queued_id() == *p));
    if before == Some(id) {
        return Ok(Some(order(queue, thread)));
    }
    if pinned_front == Some(id) || (before.is_some() && before == pinned_front) {
        return Err(ProviderError::ValidationError {
            message: PINNED.into(),
        });
    }
    let item = queue.remove(from).expect("index in range");
    let to = match before {
        Some(before) => queue
            .iter()
            .position(|entry| entry.queued_id() == before && ours(entry))
            .unwrap_or(from),
        None => queue
            .iter()
            .rposition(|entry| ours(entry))
            .map_or(from, |last| (last + 1).max(from)),
    };
    queue.insert(to.min(queue.len()), item);
    Ok(Some(order(queue, thread)))
}

/// The event announcing `thread`'s queue order, for a provider that put an
/// item somewhere other than the back (a send that goes first through a
/// held queue), so the UI and the saved queue follow.
pub(crate) fn reordered<T: QueueEntry>(queue: &VecDeque<T>, thread: &ThreadId) -> super::ProviderRuntimeEvent {
    super::ProviderRuntimeEvent::QueuedTurnsReordered {
        thread_id: thread.clone(),
        queued_ids: order(queue, thread),
    }
}

/// `thread`'s queued ids in dispatch order.
pub(crate) fn order<T: QueueEntry>(queue: &VecDeque<T>, thread: &ThreadId) -> Vec<String> {
    queue
        .iter()
        .filter(|entry| &entry.input().thread_id == thread)
        .map(|entry| entry.queued_id().to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(thread: &str, text: &str, display: Option<&str>) -> SendTurnInput {
        SendTurnInput {
            thread_id: ThreadId(thread.into()),
            text: text.into(),
            display_text: display.map(Into::into),
            images: Vec::new(),
            skill_invocations: Vec::new(),
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            turn_checkpoint: None,
            dispatch_guard: None,
        }
    }

    fn queue(items: &[(&str, &str)]) -> VecDeque<(String, SendTurnInput)> {
        items
            .iter()
            .map(|(id, thread)| (id.to_string(), input(thread, id, None)))
            .collect()
    }

    fn t(id: &str) -> ThreadId {
        ThreadId(id.into())
    }

    #[test]
    fn edit_keeps_framing_and_refuses_what_it_cannot_splice() {
        let mut q: VecDeque<(String, SendTurnInput)> = VecDeque::new();
        q.push_back(("a".into(), input("t", "<mode>plan</mode>\n\nfix it", Some("fix it"))));
        q.push_back(("b".into(), input("t", "/compact now\n\n<files/>", Some("/compact now"))));
        q.push_back(("c".into(), input("t", "ultrathink\n\nfix", Some("ultrathink fix"))));
        q.push_back(("d".into(), input("t", "plain", None)));

        assert_eq!(edit(&mut q, &t("t"), "a", " fix all ").unwrap().as_deref(), Some("fix all"));
        assert_eq!(q[0].1.text, "<mode>plan</mode>\n\nfix all");
        assert_eq!(q[0].1.display_text.as_deref(), Some("fix all"));
        // A command would become prose behind the framing.
        assert!(edit(&mut q, &t("t"), "a", "/compact").is_err());

        assert!(edit(&mut q, &t("t"), "b", "/compact later").unwrap().is_some());
        assert_eq!(q[1].1.text, "/compact later\n\n<files/>");

        assert!(edit(&mut q, &t("t"), "c", "other").is_err());
        assert_eq!(q[2].1.text, "ultrathink\n\nfix");

        assert!(edit(&mut q, &t("t"), "d", "/goal ship").unwrap().is_some());
        assert_eq!(q[3].1.text, "/goal ship");

        assert!(edit(&mut q, &t("t"), "d", "   ").is_err());
        assert_eq!(edit(&mut q, &t("t"), "gone", "x").unwrap(), None);
        assert_eq!(edit(&mut q, &t("other"), "d", "x").unwrap(), None);
    }

    #[test]
    fn move_before_reorders_within_the_thread() {
        let mut q = queue(&[("a", "t"), ("b", "t"), ("c", "t")]);
        assert_eq!(move_before(&mut q, &t("t"), "c", Some("a"), None).unwrap().unwrap(), ["c", "a", "b"]);
        assert_eq!(move_before(&mut q, &t("t"), "c", None, None).unwrap().unwrap(), ["a", "b", "c"]);
        assert_eq!(move_before(&mut q, &t("t"), "a", Some("c"), None).unwrap().unwrap(), ["b", "a", "c"]);
        // A target that already left the queue changes nothing.
        assert_eq!(move_before(&mut q, &t("t"), "a", Some("gone"), None).unwrap().unwrap(), ["b", "a", "c"]);
        assert_eq!(move_before(&mut q, &t("t"), "gone", None, None).unwrap(), None);
    }

    #[test]
    fn move_to_the_end_stays_ahead_of_other_threads() {
        let mut q = queue(&[("a", "t"), ("x", "u"), ("b", "t"), ("y", "u")]);
        assert_eq!(move_before(&mut q, &t("t"), "a", None, None).unwrap().unwrap(), ["b", "a"]);
        let ids: Vec<_> = q.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["x", "b", "a", "y"]);
        let mut solo = queue(&[("x", "u"), ("a", "t"), ("y", "u")]);
        move_before(&mut solo, &t("t"), "a", None, None).unwrap();
        assert_eq!(solo[1].0, "a");
    }

    #[test]
    fn the_item_a_held_queue_will_send_keeps_the_front() {
        let mut q = queue(&[("m", "t"), ("a", "t")]);
        assert!(move_before(&mut q, &t("t"), "m", None, Some("m")).is_err());
        assert!(move_before(&mut q, &t("t"), "a", Some("m"), Some("m")).is_err());
        // Once it is no longer at the front, it does not pin anything.
        let mut q = queue(&[("a", "t"), ("m", "t")]);
        assert!(move_before(&mut q, &t("t"), "m", Some("a"), Some("m")).is_ok());
    }
}
