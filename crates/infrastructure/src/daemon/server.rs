//! `WindowHub` adapter: the background service's end of the local
//! connection, a sibling of the hook socket with the same transport.
//!
//! On Windows it is a named pipe that refuses remote clients and whose
//! descriptor lets only this user open it; elsewhere a Unix socket with
//! mode 0600. Every connection is read on the runtime and handed over as
//! [`HubEvent`]s; what is sent goes out through a queue per connection, so
//! a slow window never holds the service up.

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use swarmux_application::daemon::{Answer, HubEvent, Shown, WindowHub, WindowId};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::task::AbortHandle;

use super::wire::{Down, LINE_MAX, Opening, Up, line};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Shared {
    events: Mutex<Sender<HubEvent>>,
    /// Each connection's outgoing queue; `None` closes it.
    writers: Mutex<HashMap<WindowId, UnboundedSender<Option<String>>>>,
    /// Connections that said hello and are still open.
    windows: Mutex<HashSet<WindowId>>,
    next: AtomicU64,
}

impl Shared {
    fn push(&self, event: HubEvent) {
        let _ = lock(&self.events).send(event);
    }

    fn write(&self, window: WindowId, text: Option<String>) {
        if let Some(w) = lock(&self.writers).get(&window) {
            let _ = w.send(text);
        }
    }
}

/// Listens for windows and askers for as long as it lives.
pub struct WindowServer {
    name: String,
    shared: Arc<Shared>,
    events: Mutex<Receiver<HubEvent>>,
    task: AbortHandle,
}

impl WindowServer {
    /// Starts listening on `name` (see [`super::endpoint`]). Fails when
    /// something listens there already.
    pub fn start(rt: &Handle, name: &str) -> io::Result<WindowServer> {
        let (tx, rx) = channel();
        let shared = Arc::new(Shared {
            events: Mutex::new(tx),
            writers: Mutex::default(),
            windows: Mutex::default(),
            next: AtomicU64::new(1),
        });
        let _guard = rt.enter();
        let task = listen(name, shared.clone())?;
        Ok(WindowServer {
            name: name.to_string(),
            shared,
            events: Mutex::new(rx),
            task,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Windows attached now.
    pub fn windows(&self) -> usize {
        lock(&self.shared.windows).len()
    }

    /// Sends every attached window the same thing.
    pub fn broadcast(&self, shown: &Shown) {
        let text = line(&Down::of_shown(shown));
        let windows: Vec<WindowId> = lock(&self.shared.windows).iter().copied().collect();
        for w in windows {
            self.shared.write(w, Some(text.clone()));
        }
    }
}

impl Drop for WindowServer {
    fn drop(&mut self) {
        self.task.abort();
        #[cfg(not(windows))]
        let _ = std::fs::remove_file(&self.name);
    }
}

impl WindowHub for WindowServer {
    fn poll(&self, wait: Duration) -> Vec<HubEvent> {
        let rx = lock(&self.events);
        let mut out = Vec::new();
        if let Ok(first) = rx.recv_timeout(wait) {
            out.push(first);
            out.extend(rx.try_iter());
        }
        out
    }

    fn send(&self, window: WindowId, shown: Shown) {
        self.shared
            .write(window, Some(line(&Down::of_shown(&shown))));
    }

    fn answer(&self, window: WindowId, answer: Answer) {
        self.shared
            .write(window, Some(line(&Down::of_answer(&answer))));
        self.close(window);
    }

    fn close(&self, window: WindowId) {
        lock(&self.shared.windows).remove(&window);
        if let Some(w) = lock(&self.shared.writers).remove(&window) {
            let _ = w.send(None);
        }
    }
}

#[cfg(windows)]
fn listen(name: &str, shared: Arc<Shared>) -> io::Result<AbortHandle> {
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    let security = super::security::UserOnly::new()?;
    let create = move |name: &str, first: bool| -> io::Result<NamedPipeServer> {
        let mut attrs = security.attributes();
        // SAFETY: `attrs` points at a descriptor `security` keeps alive;
        // the system copies it into the pipe.
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(name, (&raw mut attrs).cast())
        }
    };
    let mut server = create(name, true)?;
    let name = name.to_string();
    let task = tokio::spawn(async move {
        loop {
            if server.connect().await.is_ok() {
                let next = loop {
                    match create(&name, false) {
                        Ok(next) => break next,
                        Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                    }
                };
                let conn = std::mem::replace(&mut server, next);
                tokio::spawn(serve(conn, shared.clone()));
            } else if let Ok(fresh) = create(&name, false) {
                server = fresh;
            } else {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    });
    Ok(task.abort_handle())
}

#[cfg(not(windows))]
fn listen(name: &str, shared: Arc<Shared>) -> io::Result<AbortHandle> {
    use std::os::unix::fs::PermissionsExt;

    // The caller holds the service's lock: a socket file left here is a
    // crashed run's.
    let _ = std::fs::remove_file(name);
    let listener = tokio::net::UnixListener::bind(name)?;
    std::fs::set_permissions(name, std::fs::Permissions::from_mode(0o600))?;
    let task = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((conn, _)) => {
                    tokio::spawn(serve(conn, shared.clone()));
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    });
    Ok(task.abort_handle())
}

/// Reads one connection to its end.
async fn serve<S: AsyncRead + AsyncWrite + Send + 'static>(conn: S, shared: Arc<Shared>) {
    let (read, mut write) = tokio::io::split(conn);
    let id = WindowId(shared.next.fetch_add(1, Ordering::Relaxed));
    let (tx, mut rx) = unbounded_channel::<Option<String>>();
    lock(&shared.writers).insert(id, tx);
    // Once the writer is done, the reading stops too and the connection
    // closes: a window let go hears so at once.
    let (done_tx, mut done) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        while let Some(Some(text)) = rx.recv().await {
            if write.write_all(text.as_bytes()).await.is_err() {
                break;
            }
            let _ = write.flush().await;
        }
        let _ = write.shutdown().await;
        let _ = done_tx.send(());
    });
    let mut reader = BufReader::new(read);
    let mut buf = Vec::new();
    let mut hello = false;
    for first in std::iter::once(true).chain(std::iter::repeat(false)) {
        buf.clear();
        let mut limited = (&mut reader).take(LINE_MAX as u64);
        let read = limited.read_until(b'\n', &mut buf);
        tokio::select! {
            r = read => match r {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            },
            _ = &mut done => break,
        }
        if buf.last() != Some(&b'\n') {
            // Longer than any message: not a window of ours.
            break;
        }
        let Ok(up) = serde_json::from_slice::<Up>(&buf) else {
            continue;
        };
        if first {
            match up.opening() {
                Some(Opening::Hello(h)) => {
                    hello = true;
                    lock(&shared.windows).insert(id);
                    shared.push(HubEvent::Hello(id, h));
                }
                Some(Opening::Ask(ask)) => shared.push(HubEvent::Asked(id, ask)),
                None => break,
            }
        } else if let Some(input) = up.input().filter(|_| hello) {
            shared.push(HubEvent::Input(id, input));
        }
    }
    lock(&shared.windows).remove(&id);
    if hello {
        shared.push(HubEvent::Left(id));
    }
    // What is queued still goes out; then the writer ends.
    lock(&shared.writers).remove(&id);
}
