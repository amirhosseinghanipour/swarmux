//! The board-on versus board-off A/B harness.
//!
//! Two scripted agents get the same mission: six areas of work, each
//! agent told to do all of them in the same order (the case where a
//! planner gave two agents overlapping work). The script is deterministic
//! and runs twice: with the board, where an agent claims an area before
//! starting it, skips what another job holds and skips what a fact says is
//! done; and without it, where each agent works alone.
//!
//! The metrics are the work done twice, the claims that collided and the
//! tokens the digest costs a session that starts at the end. Run it with
//! `cargo test -p swarmux-application ab -- --nocapture` to see them.

use std::collections::BTreeSet;

use super::*;
use crate::ports::{JobRepository, NewJob, SessionRepository};
use crate::testing::{FixedClock, InMemoryBoard, InMemoryStore};
use swarmux_domain::estimate_tokens;
use swarmux_domain::{Session, SessionId};

const AREAS: usize = 6;
const ROUNDS: usize = AREAS;

/// What one run of the script came to.
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Areas worked on, counting each time.
    pub executions: usize,
    /// Distinct areas worked on.
    pub distinct: usize,
    /// Claims that were refused because another job held the area.
    pub claims_refused: usize,
    /// Tokens of the digest the next session would start with; `0` off.
    pub digest_tokens: usize,
}

impl Outcome {
    /// Work done twice.
    pub fn duplicates(&self) -> usize {
        self.executions - self.distinct
    }
}

fn area(i: usize) -> String {
    format!("src/m{i}/**")
}

/// Runs the script with the board on or off.
pub fn run(board_on: bool) -> Outcome {
    let db = Rc::new(InMemoryStore::default());
    let session = Session::new(SessionId::random(), "s", "/repo".into(), "sonnet", 0).unwrap();
    SessionRepository::insert(&*db, &session).unwrap();
    let hub = Board::new();
    let clock = Rc::new(FixedClock::new(9 * 3_600_000));
    let svc = BoardService::new(
        Rc::new(InMemoryBoard::default()),
        hub.clone(),
        db.clone(),
        clock.clone(),
    )
    .unwrap();
    let jobs: Vec<JobId> = (0..2)
        .map(|_| {
            let id = JobRepository::insert(
                &*db,
                &NewJob {
                    session_id: session.id,
                    prompt_id: None,
                    prompt: "p".into(),
                    model: "sonnet".into(),
                    priority: 0,
                    fork: false,
                    wants_review: false,
                    review_of: None,
                },
                0,
            )
            .unwrap();
            hub.join(id, "m", None);
            id
        })
        .collect();
    let call = |job: JobId, request: BoardRequest| {
        svc.handle(&BoardCall {
            id: 1,
            job: Some(job),
            request,
        })
        .unwrap()
    };

    let mut own_done: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); 2];
    let mut executions = Vec::new();
    let mut refused = 0;
    for _ in 0..ROUNDS {
        clock.advance(60_000);
        // Each agent chooses; the second sees the first one's claim.
        let mut chosen = Vec::new();
        for (a, job) in jobs.iter().enumerate() {
            let pick = (0..AREAS).find(|i| {
                if own_done[a].contains(i) {
                    return false;
                }
                if !board_on {
                    return true;
                }
                let facts = call(
                    *job,
                    BoardRequest::Read {
                        kinds: vec![BoardKind::Fact],
                        since: None,
                    },
                );
                if facts.text.contains(&format!("area {i} done")) {
                    return false;
                }
                let claimed = call(
                    *job,
                    BoardRequest::Claim {
                        paths: vec![area(*i)],
                        note: format!("area {i}"),
                    },
                );
                if claimed.error {
                    refused += 1;
                }
                !claimed.error
            });
            chosen.push(pick);
        }
        // Then both work, and the board learns what is done.
        for (a, pick) in chosen.iter().enumerate() {
            let Some(i) = pick else { continue };
            executions.push(*i);
            own_done[a].insert(*i);
            if board_on {
                call(
                    jobs[a],
                    BoardRequest::Fact {
                        text: format!("area {i} done"),
                        evidence: "cargo test".into(),
                    },
                );
            }
        }
    }
    let distinct = executions.iter().collect::<BTreeSet<_>>().len();
    let digest_tokens = if board_on {
        let text = BoardDigest::new(hub, None)
            .context(&SessionStartInfo {
                job: Some(jobs[0]),
                session: "s".into(),
                source: "startup".into(),
            })
            .unwrap_or_default();
        estimate_tokens(&text)
    } else {
        0
    };
    Outcome {
        executions: executions.len(),
        distinct,
        claims_refused: refused,
        digest_tokens,
    }
}

#[test]
fn the_board_stops_overlapping_agents_from_doing_the_same_work() {
    let off = run(false);
    let on = run(true);
    println!("board  | work done | distinct | duplicates | claims refused | digest tokens");
    for (name, o) in [("off", &off), ("on", &on)] {
        println!(
            "{name:<6} | {:>9} | {:>8} | {:>10} | {:>14} | {:>13}",
            o.executions,
            o.distinct,
            o.duplicates(),
            o.claims_refused,
            o.digest_tokens
        );
    }
    // Without the board both agents do all six areas.
    assert_eq!((off.executions, off.distinct), (12, 6));
    assert_eq!(off.duplicates(), 6);
    // With it every area is done once, and the second agent was turned
    // away from the areas the first one held.
    assert_eq!(on.distinct, 6);
    assert_eq!(on.duplicates(), 0);
    assert!(on.claims_refused > 0);
    assert!(on.digest_tokens > 0 && on.digest_tokens <= DIGEST_TOKENS);
}
