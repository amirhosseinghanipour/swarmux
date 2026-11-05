//! The background service's connection, end to end over the real local
//! socket: a window says hello, is sent frames, types, and leaves; asks
//! are answered; only one service can listen on an endpoint.

use std::time::{Duration, Instant};

use swarmux_application::daemon::{
    Answer, Ask, CellStyle, DaemonStatus, Frame, GlyphHints, Hello, HubEvent, KeyInput, KeyName,
    PROTOCOL, Run, Screen, Shown, WindowHub, WindowInput,
};
use swarmux_infrastructure::daemon::{WindowClient, WindowServer, ask, endpoint};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap()
}

/// An endpoint of a folder of this test's own.
fn name(dir: &tempfile::TempDir) -> String {
    endpoint(dir.path())
}

/// Polls until `pred` holds for some event, or panics after a while.
fn wait_for(server: &WindowServer, mut pred: impl FnMut(&HubEvent) -> bool) -> HubEvent {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        for e in server.poll(Duration::from_millis(50)) {
            if pred(&e) {
                return e;
            }
        }
    }
    panic!("the event never came");
}

fn hello() -> Hello {
    Hello {
        version: "0.1.0".into(),
        protocol: PROTOCOL,
        cols: 80,
        rows: 24,
        glyphs: GlyphHints::default(),
        splash: true,
        cwd: None,
    }
}

#[test]
fn a_window_attaches_is_shown_frames_types_and_leaves() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let name = name(&dir);
    let server = WindowServer::start(rt.handle(), &name).unwrap();
    let mut client = WindowClient::attach(&name, &hello()).unwrap();
    let HubEvent::Hello(id, got) = wait_for(&server, |e| matches!(e, HubEvent::Hello(..))) else {
        unreachable!()
    };
    assert_eq!(got, hello());
    assert_eq!(server.windows(), 1);

    let frame = Frame {
        cols: 80,
        rows: 24,
        full: true,
        runs: vec![Run {
            x: 0,
            y: 0,
            cells: vec!["界".into(), String::new(), "x".into()],
            style: CellStyle::default(),
        }],
    };
    server.send(id, Shown::Frame(frame.clone()));
    server.broadcast(&Shown::Notify("\x07".into()));
    let mut got = Vec::new();
    let start = Instant::now();
    while got.len() < 2 && start.elapsed() < Duration::from_secs(10) {
        if let Some(s) = client.recv(Duration::from_millis(50)).unwrap() {
            got.push(s);
        }
    }
    assert_eq!(got, [Shown::Frame(frame), Shown::Notify("\x07".into())]);

    let key = WindowInput::Key(KeyInput::plain(KeyName::Char('y')));
    client.send(key.clone()).unwrap();
    let e = wait_for(&server, |e| matches!(e, HubEvent::Input(..)));
    assert_eq!(e, HubEvent::Input(id, key));

    // Let go by the service: the window is told why, then the line closes.
    server.send(id, Shown::Bye("bye".into()));
    server.close(id);
    let start = Instant::now();
    let mut bye = None;
    while start.elapsed() < Duration::from_secs(10) {
        match client.recv(Duration::from_millis(50)) {
            Ok(Some(Shown::Bye(t))) => bye = Some(t),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert_eq!(bye.as_deref(), Some("bye"));
    drop(client);
    assert_eq!(server.windows(), 0);
}

#[test]
fn a_window_that_closes_is_seen_to_leave() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let name = name(&dir);
    let server = WindowServer::start(rt.handle(), &name).unwrap();
    let client = WindowClient::attach(&name, &hello()).unwrap();
    let HubEvent::Hello(id, _) = wait_for(&server, |e| matches!(e, HubEvent::Hello(..))) else {
        unreachable!()
    };
    drop(client);
    assert_eq!(
        wait_for(&server, |e| matches!(e, HubEvent::Left(_))),
        HubEvent::Left(id)
    );
}

#[test]
fn asks_are_answered_and_the_line_closed() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let name = name(&dir);
    let server = std::sync::Arc::new(WindowServer::start(rt.handle(), &name).unwrap());
    let answering = {
        let server = server.clone();
        std::thread::spawn(move || {
            let HubEvent::Asked(id, Ask::Status) =
                wait_for(&server, |e| matches!(e, HubEvent::Asked(..)))
            else {
                panic!()
            };
            server.answer(
                id,
                Answer::Status(DaemonStatus {
                    pid: 7,
                    version: "0.1.0".into(),
                    ..DaemonStatus::default()
                }),
            );
        })
    };
    let Answer::Status(s) = ask(&name, Ask::Status).unwrap() else {
        panic!()
    };
    assert_eq!((s.pid, s.version.as_str()), (7, "0.1.0"));
    answering.join().unwrap();
    assert_eq!(server.windows(), 0, "an asker is not a window");
}

#[test]
fn one_service_per_endpoint_and_nobody_listening_fails_fast() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let name = name(&dir);
    let start = Instant::now();
    assert!(ask(&name, Ask::Status).is_err());
    assert!(WindowClient::attach(&name, &hello()).is_err());
    assert!(start.elapsed() < Duration::from_secs(3));
    let _first = WindowServer::start(rt.handle(), &name).unwrap();
    #[cfg(windows)]
    assert!(
        WindowServer::start(rt.handle(), &name).is_err(),
        "a second pipe server is refused"
    );
}

#[cfg(unix)]
#[test]
fn the_socket_is_this_users_alone() {
    use std::os::unix::fs::PermissionsExt;
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let name = name(&dir);
    let _server = WindowServer::start(rt.handle(), &name).unwrap();
    let mode = std::fs::metadata(&name).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
}
