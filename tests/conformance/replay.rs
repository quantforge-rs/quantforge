//! The replay: a local server that answers each request with the next
//! response of a transcript, strictly in order, and keeps a note of every
//! request the transcript did not expect.

use std::sync::{Arc, Mutex};
use url::Url;

use crate::fixture::{self, Interaction};
use crate::profiles::Profile;
use crate::server::{Answer, Captured, Handle, Server};

pub(crate) struct Replay {
    server: Server,
    state: Arc<Mutex<State>>,
}

struct State {
    interactions: Vec<Interaction>,
    next: usize,
    mismatches: Vec<String>,
}

struct Replaying {
    state: Arc<Mutex<State>>,
    profile: Profile,
}

/// What an unexpected request gets. A check that expects an error could
/// pass on it, so the harness fails the check on the mismatch itself.
const UNEXPECTED: &str = r#"{"code":-1000,"msg":"conformance replay: unexpected request"}"#;

impl Handle for Replaying {
    async fn handle(&self, request: Captured) -> Answer {
        let seen = fixture::observe(&request, &self.profile);
        let mut state = self.state.lock().expect("replay state");
        let number = state.next + 1;
        let answer = match state.interactions.get(state.next) {
            Some(expected) if fixture::matches(&expected.request, &seen) => {
                Ok(expected.response.answer())
            }
            Some(expected) => Err(format!(
                "request {number} was `{seen}`, but the transcript has `{}`",
                expected.request
            )),
            None => Err(format!(
                "request {number} was `{seen}`, past the transcript's end"
            )),
        };
        match answer {
            Ok(answer) => {
                state.next += 1;
                answer
            }
            Err(mismatch) => {
                state.mismatches.push(mismatch);
                Answer {
                    status: 500,
                    body: UNEXPECTED.to_string(),
                }
            }
        }
    }
}

impl Replay {
    pub(crate) fn start(interactions: Vec<Interaction>, profile: Profile) -> Self {
        let state = Arc::new(Mutex::new(State {
            interactions,
            next: 0,
            mismatches: Vec::new(),
        }));
        let server = Server::start(Replaying {
            state: Arc::clone(&state),
            profile,
        });
        Self { server, state }
    }

    pub(crate) fn base_url(&self) -> Url {
        self.server.base_url()
    }

    /// Everything that did not go as the transcript says: requests it did
    /// not expect, and responses it holds that nobody asked for.
    pub(crate) fn problems(&self) -> Vec<String> {
        let state = self.state.lock().expect("replay state");
        let mut problems = state.mismatches.clone();
        let unused = state.interactions.len().saturating_sub(state.next);
        if unused > 0 {
            problems.push(format!(
                "the transcript's last {unused} of {} responses were never asked for, the \
                 first being `{}`",
                state.interactions.len(),
                state.interactions[state.next].request
            ));
        }
        problems
    }
}
