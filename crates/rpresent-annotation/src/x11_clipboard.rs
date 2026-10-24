use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A clipboard request is bound to one text-editing session and an increasing
/// per-session sequence number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasteRequest {
    pub session: u64,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteResult {
    pub request: PasteRequest,
    pub text: Result<String, String>,
}

pub fn paste_belongs_to_session(active: Option<u64>, request: PasteRequest) -> bool {
    active == Some(request.session)
}

/// Convert CRLF and lone CR line endings from clipboard text.
pub(crate) fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub(crate) const PASTE_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardTarget {
    Utf8String,
    String,
}

impl ClipboardTarget {
    pub(crate) fn property(self) -> &'static str {
        match self {
            Self::Utf8String => "RPRESENT_PASTE_UTF8",
            Self::String => "RPRESENT_PASTE_STRING",
        }
    }

    fn fallback(self) -> Option<Self> {
        match self {
            Self::Utf8String => Some(Self::String),
            Self::String => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PropertyKind {
    None,
    Incr,
    Text,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PropertyRead {
    pub kind: PropertyKind,
    pub format: u8,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardAction {
    Convert {
        requestor: u32,
        target: ClipboardTarget,
    },
    Read {
        requestor: u32,
        property: &'static str,
        delete: bool,
    },
    Destroy {
        requestor: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    AwaitSelection,
    AwaitInitialRead,
    AwaitChunk,
}

struct Active {
    request: PasteRequest,
    requestor: u32,
    target: ClipboardTarget,
    stage: Stage,
    retried: bool,
    buffer: Vec<u8>,
    deadline: Instant,
}

/// Serializes clipboard conversions. The host creates one hidden requestor
/// window per request and executes the emitted actions; the machine itself
/// stays free of X11 types so transfers can be tested with simulated replies.
pub(crate) struct PasteQueue {
    queued: VecDeque<(PasteRequest, u32)>,
    active: Option<Active>,
    actions: Vec<ClipboardAction>,
    results: Vec<PasteResult>,
}

impl PasteQueue {
    pub(crate) fn new() -> Self {
        Self {
            queued: VecDeque::new(),
            active: None,
            actions: Vec::new(),
            results: Vec::new(),
        }
    }

    pub(crate) fn enqueue(&mut self, request: PasteRequest, requestor: u32, now: Instant) {
        self.queued.push_back((request, requestor));
        if self.active.is_none() {
            self.start_next(now);
        }
    }

    pub(crate) fn cancel_session(&mut self, session: u64, now: Instant) {
        self.queued.retain(|(request, _)| request.session != session);
        let cancelled = self
            .active
            .as_ref()
            .is_some_and(|active| active.request.session == session);
        if cancelled {
            if let Some(active) = self.active.take() {
                self.actions.push(ClipboardAction::Destroy {
                    requestor: active.requestor,
                });
            }
            self.start_next(now);
        }
    }

    pub(crate) fn selection_notify(
        &mut self,
        requestor: u32,
        target: ClipboardTarget,
        property: Option<&'static str>,
        now: Instant,
    ) {
        let Some(expected) = self.expected(requestor, Stage::AwaitSelection, target) else {
            return;
        };
        let Some(property) = property else {
            self.conversion_failed(now);
            return;
        };
        if property != expected.property() {
            return;
        }
        if let Some(active) = self.active.as_mut() {
            active.stage = Stage::AwaitInitialRead;
        }
        self.actions.push(ClipboardAction::Read {
            requestor,
            property,
            delete: false,
        });
    }

    pub(crate) fn property_read(
        &mut self,
        requestor: u32,
        property: &'static str,
        read: PropertyRead,
        now: Instant,
    ) {
        let Some((stage, target)) = self.read_target(requestor, property) else {
            return;
        };
        // The INCR marker carries a 32-bit lower bound instead of text.
        if read.format != 8 && !matches!(read.kind, PropertyKind::Incr) {
            self.finish(
                Err(format!(
                    "clipboard property format {} is unsupported",
                    read.format
                )),
                now,
            );
            return;
        }
        match stage {
            Stage::AwaitInitialRead => match read.kind {
                PropertyKind::None => self.conversion_failed(now),
                PropertyKind::Incr => {
                    if let Some(active) = self.active.as_mut() {
                        active.stage = Stage::AwaitChunk;
                        active.buffer.clear();
                    }
                }
                PropertyKind::Text => self.finish(decode_text(target, &read.bytes), now),
                PropertyKind::Unknown => self.finish(
                    Err("clipboard property has an unsupported type".to_owned()),
                    now,
                ),
            },
            Stage::AwaitChunk => match read.kind {
                PropertyKind::None | PropertyKind::Unknown => self.finish(
                    Err("clipboard property has an unsupported type".to_owned()),
                    now,
                ),
                PropertyKind::Incr => {}
                PropertyKind::Text => {
                    if read.bytes.is_empty() {
                        let text = self
                            .active
                            .as_ref()
                            .map(|active| decode_text(target, &active.buffer))
                            .unwrap_or_else(|| Err("clipboard transfer is no longer active".to_owned()));
                        self.finish(text, now);
                    } else if let Some(active) = self.active.as_mut() {
                        active.buffer.extend_from_slice(&read.bytes);
                    }
                }
            },
            Stage::AwaitSelection => {}
        }
    }

    pub(crate) fn property_changed(&mut self, requestor: u32, now: Instant) {
        let _ = now;
        let Some(active) = self.active.as_ref() else {
            return;
        };
        if active.requestor != requestor || active.stage != Stage::AwaitChunk {
            return;
        }
        let property = active.target.property();
        self.actions.push(ClipboardAction::Read {
            requestor,
            property,
            delete: true,
        });
    }

    pub(crate) fn tick(&mut self, now: Instant) {
        let expired = self
            .active
            .as_ref()
            .is_some_and(|active| active.deadline <= now);
        if expired {
            self.finish(Err("clipboard request timed out".to_owned()), now);
        }
    }

    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.active.as_ref().map(|active| active.deadline)
    }

    pub(crate) fn take_actions(&mut self) -> Vec<ClipboardAction> {
        std::mem::take(&mut self.actions)
    }

    pub(crate) fn take_results(&mut self) -> Vec<PasteResult> {
        std::mem::take(&mut self.results)
    }

    fn expected(
        &self,
        requestor: u32,
        stage: Stage,
        target: ClipboardTarget,
    ) -> Option<ClipboardTarget> {
        let active = self.active.as_ref()?;
        if active.requestor != requestor || active.stage != stage || active.target != target {
            return None;
        }
        Some(active.target)
    }

    fn read_target(&self, requestor: u32, property: &'static str) -> Option<(Stage, ClipboardTarget)> {
        let active = self.active.as_ref()?;
        if active.requestor != requestor || active.target.property() != property {
            return None;
        }
        Some((active.stage, active.target))
    }

    fn conversion_failed(&mut self, now: Instant) {
        let fallback = self.active.as_ref().and_then(|active| {
            if active.retried {
                None
            } else {
                active.target.fallback().map(|target| (active.requestor, target))
            }
        });
        if let Some((requestor, target)) = fallback {
            if let Some(active) = self.active.as_mut() {
                active.retried = true;
                active.target = target;
                active.stage = Stage::AwaitSelection;
                active.buffer.clear();
            }
            self.actions.push(ClipboardAction::Convert { requestor, target });
            return;
        }
        self.finish(Err("clipboard conversion failed".to_owned()), now);
    }

    fn start_next(&mut self, now: Instant) {
        let Some((request, requestor)) = self.queued.pop_front() else {
            return;
        };
        self.actions.push(ClipboardAction::Convert {
            requestor,
            target: ClipboardTarget::Utf8String,
        });
        self.active = Some(Active {
            request,
            requestor,
            target: ClipboardTarget::Utf8String,
            stage: Stage::AwaitSelection,
            retried: false,
            buffer: Vec::new(),
            deadline: now + PASTE_DEADLINE,
        });
    }

    fn finish(&mut self, text: Result<String, String>, now: Instant) {
        let Some(active) = self.active.take() else {
            return;
        };
        self.actions.push(ClipboardAction::Destroy {
            requestor: active.requestor,
        });
        self.results.push(PasteResult {
            request: active.request,
            text,
        });
        self.start_next(now);
    }
}

fn decode_text(target: ClipboardTarget, bytes: &[u8]) -> Result<String, String> {
    match target {
        ClipboardTarget::Utf8String => std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| "clipboard text is not valid UTF-8".to_owned()),
        ClipboardTarget::String => Ok(bytes.iter().map(|byte| *byte as char).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant {
        Instant::now()
    }

    fn text_read(text: &str) -> PropertyRead {
        PropertyRead {
            kind: PropertyKind::Text,
            format: 8,
            bytes: text.as_bytes().to_vec(),
        }
    }

    #[test]
    fn cancelled_paste_cannot_reach_a_new_editor() {
        let request = PasteRequest {
            session: 7,
            sequence: 1,
        };
        assert!(paste_belongs_to_session(Some(7), request));
        assert!(!paste_belongs_to_session(Some(8), request));
        assert!(!paste_belongs_to_session(None, request));
    }

    #[test]
    fn utf8_selection_is_read_and_delivered() {
        let start = now();
        let mut queue = PasteQueue::new();
        queue.enqueue(
            PasteRequest {
                session: 1,
                sequence: 1,
            },
            10,
            start,
        );
        assert_eq!(
            queue.take_actions(),
            vec![ClipboardAction::Convert {
                requestor: 10,
                target: ClipboardTarget::Utf8String,
            }]
        );
        queue.selection_notify(
            10,
            ClipboardTarget::Utf8String,
            Some("RPRESENT_PASTE_UTF8"),
            start,
        );
        assert_eq!(
            queue.take_actions(),
            vec![ClipboardAction::Read {
                requestor: 10,
                property: "RPRESENT_PASTE_UTF8",
                delete: false,
            }]
        );
        queue.property_read(
            10,
            "RPRESENT_PASTE_UTF8",
            text_read("hello"),
            start,
        );
        assert_eq!(
            queue.take_results(),
            vec![PasteResult {
                request: PasteRequest {
                    session: 1,
                    sequence: 1,
                },
                text: Ok("hello".to_owned()),
            }]
        );
        assert_eq!(
            queue.take_actions(),
            vec![ClipboardAction::Destroy { requestor: 10 }]
        );
    }

    #[test]
    fn failed_utf8_conversion_falls_back_to_string_once() {
        let start = now();
        let mut queue = PasteQueue::new();
        queue.enqueue(
            PasteRequest {
                session: 4,
                sequence: 2,
            },
            20,
            start,
        );
        queue.take_actions();
        queue.selection_notify(20, ClipboardTarget::Utf8String, None, start);
        assert_eq!(
            queue.take_actions(),
            vec![ClipboardAction::Convert {
                requestor: 20,
                target: ClipboardTarget::String,
            }]
        );
        queue.selection_notify(20, ClipboardTarget::String, None, start);
        assert_eq!(
            queue.take_results(),
            vec![PasteResult {
                request: PasteRequest {
                    session: 4,
                    sequence: 2,
                },
                text: Err("clipboard conversion failed".to_owned()),
            }]
        );
    }

    #[test]
    fn latin1_fallback_decodes_single_bytes() {
        let start = now();
        let mut queue = PasteQueue::new();
        queue.enqueue(
            PasteRequest {
                session: 2,
                sequence: 1,
            },
            21,
            start,
        );
        queue.take_actions();
        queue.selection_notify(21, ClipboardTarget::Utf8String, None, start);
        queue.take_actions();
        queue.selection_notify(
            21,
            ClipboardTarget::String,
            Some("RPRESENT_PASTE_STRING"),
            start,
        );
        queue.take_actions();
        queue.property_read(
            21,
            "RPRESENT_PASTE_STRING",
            PropertyRead {
                kind: PropertyKind::Text,
                format: 8,
                bytes: vec![0xe9],
            },
            start,
        );
        assert_eq!(
            queue.take_results()[0].text,
            Ok("\u{e9}".to_owned())
        );
    }

    #[test]
    fn incremental_transfer_accumulates_chunks_until_zero_length() {
        let start = now();
        let mut queue = PasteQueue::new();
        queue.enqueue(
            PasteRequest {
                session: 3,
                sequence: 1,
            },
            30,
            start,
        );
        queue.take_actions();
        queue.selection_notify(
            30,
            ClipboardTarget::Utf8String,
            Some("RPRESENT_PASTE_UTF8"),
            start,
        );
        queue.take_actions();
        queue.property_read(
            30,
            "RPRESENT_PASTE_UTF8",
            PropertyRead {
                kind: PropertyKind::Incr,
                format: 32,
                bytes: 1024u32.to_ne_bytes().to_vec(),
            },
            start,
        );
        assert!(queue.take_results().is_empty());

        queue.property_changed(30, start);
        assert_eq!(
            queue.take_actions(),
            vec![ClipboardAction::Read {
                requestor: 30,
                property: "RPRESENT_PASTE_UTF8",
                delete: true,
            }]
        );
        queue.property_read(30, "RPRESENT_PASTE_UTF8", text_read("ab"), start);
        assert!(queue.take_results().is_empty());

        queue.property_changed(30, start);
        queue.take_actions();
        queue.property_read(30, "RPRESENT_PASTE_UTF8", text_read("cd"), start);
        assert!(queue.take_results().is_empty());

        queue.property_changed(30, start);
        queue.take_actions();
        queue.property_read(
            30,
            "RPRESENT_PASTE_UTF8",
            PropertyRead {
                kind: PropertyKind::Text,
                format: 8,
                bytes: Vec::new(),
            },
            start,
        );
        assert_eq!(queue.take_results()[0].text, Ok("abcd".to_owned()));
    }

    #[test]
    fn timeout_fails_the_request_and_starts_the_next_one() {
        let start = now();
        let mut queue = PasteQueue::new();
        queue.enqueue(
            PasteRequest {
                session: 1,
                sequence: 1,
            },
            1,
            start,
        );
        queue.enqueue(
            PasteRequest {
                session: 1,
                sequence: 2,
            },
            2,
            start,
        );
        assert_eq!(
            queue.take_actions(),
            vec![ClipboardAction::Convert {
                requestor: 1,
                target: ClipboardTarget::Utf8String,
            }]
        );
        queue.tick(start + PASTE_DEADLINE);
        assert_eq!(
            queue.take_results(),
            vec![PasteResult {
                request: PasteRequest {
                    session: 1,
                    sequence: 1,
                },
                text: Err("clipboard request timed out".to_owned()),
            }]
        );
        let actions = queue.take_actions();
        assert!(actions.contains(&ClipboardAction::Destroy { requestor: 1 }));
        assert!(actions.contains(&ClipboardAction::Convert {
            requestor: 2,
            target: ClipboardTarget::Utf8String,
        }));
    }

    #[test]
    fn malformed_property_is_rejected_without_text() {
        let start = now();
        let mut queue = PasteQueue::new();
        queue.enqueue(
            PasteRequest {
                session: 5,
                sequence: 1,
            },
            40,
            start,
        );
        queue.take_actions();
        queue.selection_notify(
            40,
            ClipboardTarget::Utf8String,
            Some("RPRESENT_PASTE_UTF8"),
            start,
        );
        queue.take_actions();
        queue.property_read(
            40,
            "RPRESENT_PASTE_UTF8",
            PropertyRead {
                kind: PropertyKind::Text,
                format: 16,
                bytes: vec![0, 1],
            },
            start,
        );
        let results = queue.take_results();
        assert_eq!(results.len(), 1);
        assert!(results[0].text.is_err());
    }

    #[test]
    fn invalid_utf8_is_rejected_without_fallback() {
        let start = now();
        let mut queue = PasteQueue::new();
        queue.enqueue(
            PasteRequest {
                session: 6,
                sequence: 1,
            },
            41,
            start,
        );
        queue.take_actions();
        queue.selection_notify(
            41,
            ClipboardTarget::Utf8String,
            Some("RPRESENT_PASTE_UTF8"),
            start,
        );
        queue.take_actions();
        queue.property_read(
            41,
            "RPRESENT_PASTE_UTF8",
            PropertyRead {
                kind: PropertyKind::Text,
                format: 8,
                bytes: vec![0xff, 0xfe],
            },
            start,
        );
        let results = queue.take_results();
        assert_eq!(results.len(), 1);
        assert!(results[0].text.is_err());
        assert!(queue.take_actions().contains(&ClipboardAction::Destroy { requestor: 41 }));
    }

    #[test]
    fn cancellation_drops_active_and_queued_requests() {
        let start = now();
        let mut queue = PasteQueue::new();
        queue.enqueue(
            PasteRequest {
                session: 9,
                sequence: 1,
            },
            50,
            start,
        );
        queue.enqueue(
            PasteRequest {
                session: 9,
                sequence: 2,
            },
            51,
            start,
        );
        queue.take_actions();
        queue.cancel_session(9, start);
        let actions = queue.take_actions();
        assert_eq!(actions, vec![ClipboardAction::Destroy { requestor: 50 }]);
        assert!(queue.take_results().is_empty());
        assert!(queue.next_deadline().is_none());
    }

    #[test]
    fn queued_requests_are_serialized_in_sequence_order() {
        let start = now();
        let mut queue = PasteQueue::new();
        for (session, sequence, requestor) in [(1, 1, 60), (1, 2, 61)] {
            queue.enqueue(
                PasteRequest {
                    session,
                    sequence,
                },
                requestor,
                start,
            );
        }
        assert_eq!(
            queue.take_actions(),
            vec![ClipboardAction::Convert {
                requestor: 60,
                target: ClipboardTarget::Utf8String,
            }]
        );
        queue.selection_notify(
            60,
            ClipboardTarget::Utf8String,
            Some("RPRESENT_PASTE_UTF8"),
            start,
        );
        queue.take_actions();
        queue.property_read(60, "RPRESENT_PASTE_UTF8", text_read("one"), start);
        assert_eq!(queue.take_results()[0].text, Ok("one".to_owned()));
        let actions = queue.take_actions();
        assert!(actions.contains(&ClipboardAction::Convert {
            requestor: 61,
            target: ClipboardTarget::Utf8String,
        }));
        assert!(queue.next_deadline().is_some());
    }

    #[test]
    fn late_replies_from_old_requestor_are_ignored() {
        let start = now();
        let mut queue = PasteQueue::new();
        queue.enqueue(
            PasteRequest {
                session: 1,
                sequence: 1,
            },
            70,
            start,
        );
        queue.take_actions();
        queue.selection_notify(
            70,
            ClipboardTarget::Utf8String,
            Some("RPRESENT_PASTE_UTF8"),
            start,
        );
        queue.take_actions();
        queue.property_read(70, "RPRESENT_PASTE_UTF8", text_read("stale"), start);
        queue.take_results();
        queue.take_actions();
        queue.property_read(70, "RPRESENT_PASTE_UTF8", text_read("later"), start);
        assert!(queue.take_results().is_empty());
    }

    #[test]
    fn newlines_are_normalized() {
        assert_eq!(normalize_newlines("a\r\nb\rc\n"), "a\nb\nc\n");
    }

    #[test]
    fn property_read_ignores_a_mismatched_property() {
        let start = now();
        let mut queue = PasteQueue::new();
        queue.enqueue(
            PasteRequest {
                session: 1,
                sequence: 1,
            },
            80,
            start,
        );
        queue.take_actions();
        queue.selection_notify(
            80,
            ClipboardTarget::Utf8String,
            Some("RPRESENT_PASTE_UTF8"),
            start,
        );
        queue.take_actions();
        queue.property_read(80, "RPRESENT_PASTE_STRING", text_read("no"), start);
        assert!(queue.take_results().is_empty());
    }
}
