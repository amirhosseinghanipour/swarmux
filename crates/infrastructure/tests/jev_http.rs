//! The JEV HTTP adapter against a local mock server.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use swarmux_application::ports::{
    AdviceAnswer, AdviceError, AdviceReply, AdviceRequest, DecisionAdvisor,
};
use swarmux_infrastructure::{Endpoint, JevHttp};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::runtime::Runtime;

const KEY: &str = "sk-test-SECRET-4242";

/// What the mock answers, after an optional delay.
#[derive(Clone)]
struct Reply {
    status: u16,
    body: String,
    delay_ms: u64,
}

fn reply(status: u16, body: &str) -> Reply {
    Reply {
        status,
        body: body.into(),
        delay_ms: 0,
    }
}

/// Serves `reply` to every request; returns its URL and the raw requests seen.
fn mock(rt: &Runtime, reply: Reply) -> (String, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).expect("bind");
    let addr = listener.local_addr().unwrap();
    let log = seen.clone();
    rt.spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let reply = reply.clone();
            let log = log.clone();
            tokio::spawn(async move {
                let request = read_request(&mut socket).await;
                log.lock().unwrap().push(request);
                tokio::time::sleep(Duration::from_millis(reply.delay_ms)).await;
                let response = format!(
                    "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    reply.status,
                    reply.body.len(),
                    reply.body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://{addr}/v1/systemone"), seen)
}

/// Reads headers and a Content-Length body.
async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = socket.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).to_string();
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            if buf.len() >= end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

fn advisor(rt: &Runtime, url: &str, key: Option<&str>, timeout_ms: u64) -> JevHttp {
    JevHttp::new(
        "typesafe",
        Endpoint {
            url: url.into(),
            model: "jev-1.14.0".into(),
        },
        key.map(Into::into),
        Duration::from_millis(timeout_ms),
        rt.handle().clone(),
    )
    .unwrap()
}

fn ask(advisor: &JevHttp, request: AdviceRequest) -> AdviceReply {
    let (tx, rx) = mpsc::channel();
    advisor.advise(
        request,
        Box::new(move |r| {
            let _ = tx.send(r);
        }),
    );
    rx.recv_timeout(Duration::from_secs(10)).expect("a reply")
}

fn tool() -> AdviceRequest {
    AdviceRequest::ToolCall {
        tool: "Bash".into(),
        summary: "make deploy".into(),
    }
}

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

const ALLOW_93: &str = r#"{"id":"d1","model":"jev-1.14.0","answers":{"verdict":{"type":"choice","choice":"allow","probabilities":{"allow":0.95,"deny":0.05},"confidence":0.93}},"usage":{"input_tokens":30,"output_tokens":2}}"#;

#[test]
fn success_returns_the_typed_answer_and_sends_the_documented_request() {
    let rt = runtime();
    let (url, seen) = mock(&rt, reply(200, ALLOW_93));
    let jev = advisor(&rt, &url, Some(KEY), 2000);
    assert_eq!(jev.host(), url.split('/').nth(2).unwrap());
    let r = ask(&jev, tool());
    let advice = r.result.unwrap();
    assert_eq!(advice.answer, AdviceAnswer::Allow);
    assert_eq!(advice.confidence, 0.93);
    assert!(r.latency_ms < 2000);

    let request = seen.lock().unwrap()[0].clone();
    assert!(
        request.starts_with("POST /v1/systemone HTTP/1.1"),
        "{request}"
    );
    assert!(
        request.contains(&format!("authorization: Bearer {KEY}"))
            || request.contains(&format!("Authorization: Bearer {KEY}")),
        "{request}"
    );
    let body: serde_json::Value =
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["model"], "jev-1.14.0");
    assert_eq!(body["state"]["subject"], "make deploy");
    assert_eq!(body["questions"]["verdict"]["type"], "choice");
}

#[test]
fn session_question_maps_the_choice_back_to_a_name() {
    let rt = runtime();
    let body = r#"{"answers":{"session":{"type":"choice","choice":"s1","probabilities":{"s0":0.09,"s1":0.91}}}}"#;
    let (url, _) = mock(&rt, reply(200, body));
    let r = ask(
        &advisor(&rt, &url, Some(KEY), 2000),
        AdviceRequest::Session {
            prompt: "fix css".into(),
            candidates: vec!["api".into(), "web".into()],
        },
    );
    let advice = r.result.unwrap();
    assert_eq!(advice.answer, AdviceAnswer::Session("web".into()));
    assert_eq!(advice.confidence, 0.91);
}

#[test]
fn low_confidence_is_reported_as_is() {
    let rt = runtime();
    let body = r#"{"answers":{"verdict":{"type":"choice","choice":"deny","probabilities":{"allow":0.45,"deny":0.55},"confidence":0.1}}}"#;
    let (url, _) = mock(&rt, reply(200, body));
    let advice = ask(&advisor(&rt, &url, Some(KEY), 2000), tool())
        .result
        .unwrap();
    assert_eq!(
        (advice.answer, advice.confidence),
        (AdviceAnswer::Deny, 0.1)
    );
}

#[test]
fn slow_providers_time_out() {
    let rt = runtime();
    let (url, _) = mock(
        &rt,
        Reply {
            delay_ms: 3000,
            ..reply(200, ALLOW_93)
        },
    );
    let r = ask(&advisor(&rt, &url, Some(KEY), 200), tool());
    assert_eq!(r.result, Err(AdviceError::Timeout(200)));
    assert!(r.latency_ms < 2000, "{}", r.latency_ms);
}

#[test]
fn http_errors_are_reported_by_status() {
    let rt = runtime();
    for status in [401, 500] {
        let (url, _) = mock(
            &rt,
            reply(status, &format!("{{\"error\":\"bad key {KEY}\"}}")),
        );
        let r = ask(&advisor(&rt, &url, Some(KEY), 2000), tool());
        assert_eq!(r.result, Err(AdviceError::Status(status)));
    }
}

#[test]
fn malformed_json_is_invalid() {
    let rt = runtime();
    let (url, _) = mock(&rt, reply(200, "<html>oops</html>"));
    let r = ask(&advisor(&rt, &url, Some(KEY), 2000), tool());
    assert!(matches!(r.result, Err(AdviceError::Invalid(_))), "{r:?}");
}

#[test]
fn without_a_key_nothing_is_sent() {
    let rt = runtime();
    let (url, seen) = mock(&rt, reply(200, ALLOW_93));
    let jev = advisor(&rt, &url, None, 2000);
    assert!(!jev.has_key());
    let r = ask(&jev, tool());
    assert_eq!(r.result, Err(AdviceError::NoKey("typesafe".into())));
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn the_key_never_appears_in_errors() {
    let rt = runtime();
    let echo =
        format!("{{\"answers\":{{\"verdict\":{{\"choice\":\"{KEY}\",\"probabilities\":{{}}}}}}}}");
    let (url, _) = mock(&rt, reply(200, &echo));
    let mut errors = vec![ask(&advisor(&rt, &url, Some(KEY), 2000), tool()).result];
    let (url, _) = mock(&rt, reply(401, KEY));
    errors.push(ask(&advisor(&rt, &url, Some(KEY), 2000), tool()).result);
    // Nothing listens here: a connection error.
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/?key={KEY}", l.local_addr().unwrap())
    };
    errors.push(ask(&advisor(&rt, &closed, Some(KEY), 2000), tool()).result);
    for e in errors {
        let e = e.unwrap_err();
        let shown = format!("{e} {e:?}");
        assert!(!shown.contains(KEY), "{shown}");
    }
}
