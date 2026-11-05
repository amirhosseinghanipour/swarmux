//! `HookInbox` adapter: the app's end of the local socket.
//!
//! Requests are decided right where they arrive, from the shared policy, so
//! allowed and denied calls never wait for the UI. Only calls that need the
//! user wait for [`HookInbox::respond`].

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use swarmux_application::ports::{HookInbox, InboxItem};
use swarmux_application::{
    AllowStop, BoardCall, BoardInbox, BoardReply, HookFeed, HookNotice, NoSessionContext,
    SessionStartContext, SessionStartInfo, SharedPolicy, StatuslineFeed, StopAnswer, StopContext,
    StopPolicy,
};
use swarmux_domain::telemetry::Statusline;
use swarmux_domain::{Decision, HookEvent, JobId, Verdict};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::runtime::Handle;
use tokio::sync::oneshot;
use tokio::task::AbortHandle;

use super::protocol::{WireReply, WireRequest};
use crate::SystemClock;
use crate::telemetry::statusline::parse_statusline;
use swarmux_application::ports::Clock;

/// Largest request accepted, in bytes.
const REQUEST_MAX: u64 = 1 << 20;

type Waiting = Mutex<HashMap<u64, oneshot::Sender<Decision>>>;

/// What answers the events Claude Code waits for. The defaults let agents
/// stop and add no context.
#[derive(Clone)]
pub struct Responders {
    pub stop: Arc<dyn StopPolicy>,
    pub session_start: Arc<dyn SessionStartContext>,
}

impl Default for Responders {
    fn default() -> Responders {
        Responders {
            stop: Arc::new(AllowStop),
            session_start: Arc::new(NoSessionContext),
        }
    }
}

struct Shared {
    policy: SharedPolicy,
    items: Mutex<Sender<InboxItem>>,
    notices: Mutex<Sender<HookNotice>>,
    statuslines: Mutex<Sender<Statusline>>,
    calls: Mutex<Sender<BoardCall>>,
    board_waiting: Mutex<HashMap<u64, oneshot::Sender<BoardReply>>>,
    responders: Responders,
    waiting: Waiting,
    next_id: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Listens on the socket for as long as it lives.
pub struct HookServer {
    name: String,
    shared: Arc<Shared>,
    items: Mutex<Receiver<InboxItem>>,
    notices: Mutex<Receiver<HookNotice>>,
    statuslines: Mutex<Receiver<Statusline>>,
    calls: Mutex<Receiver<BoardCall>>,
    task: AbortHandle,
}

impl HookServer {
    /// Starts listening on `name` (see [`super::endpoint_for`]) on the runtime `rt`.
    pub fn start(rt: &Handle, name: &str, policy: SharedPolicy) -> io::Result<HookServer> {
        HookServer::start_with(rt, name, policy, Responders::default())
    }

    /// Like [`start`](Self::start), answering Stop, SubagentStop and
    /// SessionStart with `responders`.
    pub fn start_with(
        rt: &Handle,
        name: &str,
        policy: SharedPolicy,
        responders: Responders,
    ) -> io::Result<HookServer> {
        let (tx, rx) = channel();
        let (ntx, nrx) = channel();
        let (stx, srx) = channel();
        let (ctx, crx) = channel();
        let shared = Arc::new(Shared {
            policy,
            items: Mutex::new(tx),
            notices: Mutex::new(ntx),
            statuslines: Mutex::new(stx),
            calls: Mutex::new(ctx),
            board_waiting: Mutex::default(),
            responders,
            waiting: Mutex::default(),
            next_id: AtomicU64::new(1),
        });
        let _guard = rt.enter();
        let task = listen(name, shared.clone())?;
        Ok(HookServer {
            name: name.to_string(),
            shared,
            items: Mutex::new(rx),
            notices: Mutex::new(nrx),
            statuslines: Mutex::new(srx),
            calls: Mutex::new(crx),
            task,
        })
    }

    /// The socket name, for `SWARMUX_HOOK_PIPE`.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for HookServer {
    fn drop(&mut self) {
        self.task.abort();
        #[cfg(not(windows))]
        let _ = std::fs::remove_file(&self.name);
    }
}

impl HookFeed for HookServer {
    fn poll_notices(&self) -> Vec<HookNotice> {
        lock(&self.notices).try_iter().collect()
    }
}

impl StatuslineFeed for HookServer {
    fn poll_statuslines(&self) -> Vec<Statusline> {
        lock(&self.statuslines).try_iter().collect()
    }
}

impl BoardInbox for HookServer {
    fn poll(&self) -> Vec<BoardCall> {
        lock(&self.calls).try_iter().collect()
    }

    fn respond(&self, call: u64, reply: BoardReply) {
        if let Some(waiter) = lock(&self.shared.board_waiting).remove(&call) {
            let _ = waiter.send(reply);
        }
    }
}

impl HookInbox for HookServer {
    fn poll(&self) -> Vec<InboxItem> {
        lock(&self.items).try_iter().collect()
    }

    fn respond(&self, request_id: u64, decision: &Decision) {
        if let Some(waiter) = lock(&self.shared.waiting).remove(&request_id) {
            // The hook may have given up already; then nobody is listening.
            let _ = waiter.send(decision.clone());
        }
    }
}

#[cfg(windows)]
fn listen(name: &str, shared: Arc<Shared>) -> io::Result<AbortHandle> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(name)?;
    let name = name.to_string();
    let task = tokio::spawn(async move {
        loop {
            if server.connect().await.is_ok() {
                let next = loop {
                    match ServerOptions::new()
                        .reject_remote_clients(true)
                        .create(&name)
                    {
                        Ok(next) => break next,
                        Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
                    }
                };
                let conn = std::mem::replace(&mut server, next);
                tokio::spawn(serve(conn, shared.clone()));
            } else if let Ok(fresh) = ServerOptions::new()
                .reject_remote_clients(true)
                .create(&name)
            {
                server = fresh;
            } else {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    });
    Ok(task.abort_handle())
}

#[cfg(not(windows))]
fn listen(name: &str, shared: Arc<Shared>) -> io::Result<AbortHandle> {
    use std::os::unix::fs::PermissionsExt;

    // A socket left by a crashed run with the same pid is stale.
    let _ = std::fs::remove_file(name);
    let listener = tokio::net::UnixListener::bind(name)?;
    std::fs::set_permissions(name, std::fs::Permissions::from_mode(0o600))?;
    let task = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((conn, _)) => {
                    tokio::spawn(serve(conn, shared.clone()));
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        }
    });
    Ok(task.abort_handle())
}

/// Reads one request, answers it, closes the connection.
async fn serve<S: AsyncRead + AsyncWrite>(conn: S, shared: Arc<Shared>) {
    let (read, mut write) = tokio::io::split(conn);
    let mut reader = BufReader::new(read.take(REQUEST_MAX));
    let mut line = String::new();
    let reply = match reader.read_line(&mut line).await {
        Ok(_) => match serde_json::from_str::<WireRequest>(&line) {
            Ok(wire) if wire.is_statusline() => {
                let job = wire.job_id.map(JobId);
                if let Some(line) = parse_statusline(&wire.raw, job, SystemClock.now()) {
                    let _ = lock(&shared.statuslines).send(line);
                }
                WireReply::pass()
            }
            Ok(wire) if wire.is_board() => WireReply::board(&board(wire, &shared).await),
            Ok(wire) => match wire.hook_event() {
                Some(HookEvent::PreToolUse) => {
                    WireReply::from_decision(&decide(wire, &shared).await)
                }
                Some(event) => lifecycle(event, wire, shared.clone()).await,
                // An event this version doesn't know: leave it alone.
                None => WireReply::pass(),
            },
            Err(_) => WireReply::pass(),
        },
        Err(_) => return,
    };
    let Ok(mut out) = serde_json::to_string(&reply) else {
        return;
    };
    out.push('\n');
    let _ = write.write_all(out.as_bytes()).await;
    let _ = write.flush().await;
    let _ = write.shutdown().await;
}

/// How long an agent's board call waits for the app to answer it.
const BOARD_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Hands a board call to the app and waits for its answer.
async fn board(wire: WireRequest, shared: &Shared) -> BoardReply {
    let request = match crate::board::parse_request(&wire.tool_name, &wire.tool_input) {
        Ok(request) => request,
        Err(why) => return BoardReply::failed(format!("invalid arguments: {why}")),
    };
    let id = shared.next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    lock(&shared.board_waiting).insert(id, tx);
    let call = BoardCall {
        id,
        job: wire.job_id.map(JobId),
        request,
    };
    if lock(&shared.calls).send(call).is_err() {
        lock(&shared.board_waiting).remove(&id);
        return BoardReply::failed("swarmux is shutting down");
    }
    match tokio::time::timeout(BOARD_WAIT, rx).await {
        Ok(Ok(reply)) => reply,
        _ => {
            lock(&shared.board_waiting).remove(&id);
            BoardReply::failed("swarmux did not answer in time; try again")
        }
    }
}

async fn decide(wire: WireRequest, shared: &Shared) -> Decision {
    let id = shared.next_id.fetch_add(1, Ordering::Relaxed);
    let request = wire.into_hook_request(id);
    let (decision, micros) = shared.policy.decide(&request);
    if decision.verdict != Verdict::Ask {
        let item = InboxItem {
            request,
            answered: Some((decision.clone(), micros)),
        };
        let _ = lock(&shared.items).send(item);
        return decision;
    }
    let (tx, rx) = oneshot::channel();
    lock(&shared.waiting).insert(id, tx);
    let _ = lock(&shared.items).send(InboxItem {
        request,
        answered: None,
    });
    // If the app goes away, leave the call to Claude Code's own rules.
    rx.await
        .unwrap_or_else(|_| Decision::new(Verdict::Pass, None))
}

/// Hands a lifecycle event to the app and answers it: the events Claude
/// Code waits for ask the responders, the rest get an empty pass.
///
/// Responders may take their time (the done-gate runs a test suite), so
/// they run on a blocking thread, never on the socket's.
async fn lifecycle(event: HookEvent, wire: WireRequest, shared: Arc<Shared>) -> WireReply {
    let notice = wire.into_notice(event);
    let answer = tokio::task::spawn_blocking({
        let shared = shared.clone();
        let notice = notice.clone();
        move || respond(event, &notice, &shared)
    })
    .await;
    let _ = lock(&shared.notices).send(notice);
    // A responder that panicked leaves the agent alone.
    answer.unwrap_or_else(|_| WireReply::pass())
}

fn respond(event: HookEvent, notice: &HookNotice, shared: &Shared) -> WireReply {
    match event {
        HookEvent::Stop | HookEvent::SubagentStop => {
            let stop = StopContext {
                event,
                job: notice.job,
                session: notice.session.clone(),
                cwd: notice.cwd.clone(),
                last_message: notice.detail.clone(),
            };
            match shared.responders.stop.on_stop(&stop) {
                StopAnswer::Allow => WireReply::pass(),
                StopAnswer::Block { reason } => WireReply::print(
                    serde_json::json!({"decision": "block", "reason": reason}).to_string(),
                ),
            }
        }
        HookEvent::SessionStart => {
            shared.responders.stop.observe(notice);
            let start = SessionStartInfo {
                job: notice.job,
                session: notice.session.clone(),
                source: notice.detail.clone(),
            };
            match shared.responders.session_start.context(&start) {
                Some(text) => WireReply::print(
                    serde_json::json!({"hookSpecificOutput": {
                        "hookEventName": "SessionStart",
                        "additionalContext": text,
                    }})
                    .to_string(),
                ),
                None => WireReply::pass(),
            }
        }
        _ => {
            shared.responders.stop.observe(notice);
            WireReply::pass()
        }
    }
}
