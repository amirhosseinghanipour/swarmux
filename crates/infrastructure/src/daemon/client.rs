//! `Screen` adapter: a window's end of the connection to the background
//! service, and one-off asks (`status`, `stop`).
//!
//! The connection lives on a thread of its own with a small runtime, so
//! reading and writing never wait on each other (a synchronous pipe handle
//! on Windows would serialise them).

use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use swarmux_application::daemon::{Answer, Ask, Hello, Screen, Shown, WindowInput};
use swarmux_application::{AppError, AppResult};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use super::wire::{Down, LINE_MAX, Up, line};

/// How long a busy pipe is retried, and an ask waits for its answer.
const CONNECT_WAIT: Duration = Duration::from_secs(2);
const ANSWER_WAIT: Duration = Duration::from_secs(10);

/// An open connection to the service.
pub struct Connection {
    up: UnboundedSender<String>,
    down: Receiver<Down>,
}

impl Connection {
    /// Connects to the service listening on `name`. Fails at once when
    /// nothing listens there.
    pub fn open(name: &str) -> io::Result<Connection> {
        let (up_tx, mut up_rx) = unbounded_channel::<String>();
        let (down_tx, down_rx) = channel::<Down>();
        let (ready_tx, ready_rx) = channel::<io::Result<()>>();
        let name = name.to_string();
        std::thread::Builder::new()
            .name("swarmux-window".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                rt.block_on(async move {
                    let conn = match connect(&name).await {
                        Ok(conn) => conn,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(()));
                    let (read, mut write) = tokio::io::split(conn);
                    let reading = async move {
                        let mut reader = BufReader::new(read);
                        let mut buf = Vec::new();
                        loop {
                            buf.clear();
                            match (&mut reader)
                                .take(LINE_MAX as u64)
                                .read_until(b'\n', &mut buf)
                                .await
                            {
                                Ok(0) | Err(_) => break,
                                Ok(_) => {}
                            }
                            if let Ok(down) = serde_json::from_slice::<Down>(&buf)
                                && down_tx.send(down).is_err()
                            {
                                break;
                            }
                        }
                    };
                    let writing = async move {
                        while let Some(text) = up_rx.recv().await {
                            if write.write_all(text.as_bytes()).await.is_err() {
                                break;
                            }
                            let _ = write.flush().await;
                        }
                    };
                    // Either end closing closes the connection.
                    tokio::select! {
                        () = reading => {}
                        () = writing => {}
                    }
                });
            })?;
        ready_rx
            .recv()
            .map_err(|_| io::Error::other("the connection thread ended"))??;
        Ok(Connection {
            up: up_tx,
            down: down_rx,
        })
    }

    fn send_line(&self, text: String) -> io::Result<()> {
        self.up
            .send(text)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the service went away"))
    }

    fn recv_down(&self, wait: Duration) -> io::Result<Option<Down>> {
        match self.down.recv_timeout(wait) {
            Ok(down) => Ok(Some(down)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the service closed the connection",
            )),
        }
    }
}

/// A window attached to the service.
pub struct WindowClient {
    conn: Connection,
}

impl WindowClient {
    /// Connects and says hello; the welcome is the first thing received.
    pub fn attach(name: &str, hello: &Hello) -> io::Result<WindowClient> {
        let conn = Connection::open(name)?;
        conn.send_line(line(&Up::hello(hello)))?;
        Ok(WindowClient { conn })
    }
}

impl Screen for WindowClient {
    fn send(&mut self, input: WindowInput) -> AppResult<()> {
        self.conn
            .send_line(line(&Up::of_input(&input)))
            .map_err(|e| AppError::Service(e.to_string()))
    }

    fn recv(&mut self, wait: Duration) -> AppResult<Option<Shown>> {
        let deadline = std::time::Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.conn.recv_down(left) {
                Ok(Some(down)) => {
                    if let Some(shown) = down.shown() {
                        return Ok(Some(shown));
                    }
                }
                Ok(None) => return Ok(None),
                Err(e) => return Err(AppError::Service(e.to_string())),
            }
        }
    }
}

/// Asks the service listening on `name` one thing and returns its answer.
pub fn ask(name: &str, question: Ask) -> io::Result<Answer> {
    let conn = Connection::open(name)?;
    conn.send_line(line(&Up::ask(question)))?;
    let deadline = std::time::Instant::now() + ANSWER_WAIT;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the service did not answer",
            ));
        }
        if let Some(answer) = conn.recv_down(left)?.and_then(Down::answer) {
            return Ok(answer);
        }
    }
}

#[cfg(windows)]
async fn connect(name: &str) -> io::Result<impl AsyncRead + AsyncWrite + use<>> {
    use tokio::net::windows::named_pipe::ClientOptions;
    const ERROR_PIPE_BUSY: i32 = 231;
    let start = std::time::Instant::now();
    loop {
        match ClientOptions::new().open(name) {
            Err(e)
                if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && start.elapsed() < CONNECT_WAIT =>
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            other => return other,
        }
    }
}

#[cfg(not(windows))]
async fn connect(name: &str) -> io::Result<impl AsyncRead + AsyncWrite + use<>> {
    let _ = CONNECT_WAIT;
    tokio::net::UnixStream::connect(name).await
}
