//! JEV's JSON, as jev-layer sends it to both providers:
//! `{ model, state, questions }` with `questions` keyed by id, and replies
//! with `answers` keyed by the same ids. A `choice` answer carries `choice`,
//! `probabilities` (option id → 0..1) and an optional `confidence`.

use serde_json::{Map, Value, json};
use swarmux_application::ports::{Advice, AdviceAnswer, AdviceError, AdviceRequest};

const TOOL_QUESTION: &str = "verdict";
const SESSION_QUESTION: &str = "session";
const RECOVERY_QUESTION: &str = "recovery";

/// The request body for `request`.
pub fn request_body(model: &str, request: &AdviceRequest) -> Value {
    let (state, id, question) = match request {
        AdviceRequest::ToolCall { tool, summary } => (
            json!({
                "intent": "A coding agent wants to make a tool call that no permission rule covers. \
                           Decide whether it is routine and safe enough to run without asking the user.",
                "tool": tool,
                "subject": summary,
            }),
            TOOL_QUESTION,
            json!({
                "type": "choice",
                "instructions": "Should this tool call run without asking the user? Choose only from the supplied options.",
                "criteria": {
                    "allow": "Routine, in scope and low risk: run it.",
                    "deny": "Destructive, risky, off-task or unclear: do not run it.",
                },
            }),
        ),
        AdviceRequest::Session { prompt, candidates } => {
            let criteria: Map<String, Value> = candidates
                .iter()
                .enumerate()
                .map(|(i, name)| (option_id(i), json!({ "name": name })))
                .collect();
            (
                json!({
                    "intent": "Pick the coding session that should take this prompt.",
                    "prompt": prompt,
                }),
                SESSION_QUESTION,
                json!({
                    "type": "choice",
                    "instructions": "Which session should take this prompt? Choose only from the supplied options.",
                    "criteria": criteria,
                }),
            )
        }
        AdviceRequest::Recovery { situation, tried } => (
            json!({
                "intent": "A coding agent looks stuck. Judge from these notes whether it should \
                           carry on, change approach, or end this attempt.",
                "situation": situation,
                "tried": tried,
            }),
            RECOVERY_QUESTION,
            json!({
                "type": "choice",
                "instructions": "What should the agent do now? Choose only from the supplied options.",
                "criteria": {
                    "continue": "It is making progress, or about to: leave it alone.",
                    "change": "It keeps trying one approach that does not work: it should try another way.",
                    "stop": "This attempt is going nowhere: end it.",
                },
            }),
        ),
    };
    json!({ "model": model, "state": state, "questions": { id: question } })
}

/// Reads the answer to `request` from a reply body.
pub fn parse_reply(body: &str, request: &AdviceRequest) -> Result<Advice, AdviceError> {
    let invalid = |m: &str| AdviceError::Invalid(m.to_string());
    let reply: Value = serde_json::from_str(body).map_err(|_| invalid("not JSON"))?;
    let answers = reply
        .get("answers")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("no answers object"))?;
    let id = match request {
        AdviceRequest::ToolCall { .. } => TOOL_QUESTION,
        AdviceRequest::Session { .. } => SESSION_QUESTION,
        AdviceRequest::Recovery { .. } => RECOVERY_QUESTION,
    };
    let answer = answers
        .get(id)
        .or_else(|| {
            answers
                .values()
                .find(|a| a.get("type").and_then(Value::as_str) == Some("choice"))
        })
        .ok_or_else(|| invalid("no choice answer"))?;
    let choice = answer
        .get("choice")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("choice is not a string"))?;
    let probabilities = answer
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("no probabilities"))?;
    let mut top: f64 = 0.0;
    for p in probabilities.values() {
        let p = p
            .as_f64()
            .filter(|p| unit(*p))
            .ok_or_else(|| invalid("probability outside 0..1"))?;
        top = top.max(p);
    }
    let confidence = match answer.get("confidence") {
        None | Some(Value::Null) => top,
        Some(c) => c
            .as_f64()
            .filter(|c| unit(*c))
            .ok_or_else(|| invalid("confidence outside 0..1"))?,
    };
    let answer = match request {
        AdviceRequest::ToolCall { .. } => match choice {
            "allow" => AdviceAnswer::Allow,
            "deny" => AdviceAnswer::Deny,
            _ => return Err(invalid("choice is not allow or deny")),
        },
        AdviceRequest::Session { candidates, .. } => {
            let name = (0..candidates.len())
                .find(|&i| option_id(i) == choice)
                .map(|i| candidates[i].clone())
                .ok_or_else(|| invalid("choice is not a candidate"))?;
            AdviceAnswer::Session(name)
        }
        AdviceRequest::Recovery { .. } => match choice {
            "continue" => AdviceAnswer::Continue,
            "change" => AdviceAnswer::ChangeApproach,
            "stop" => AdviceAnswer::Stop,
            _ => return Err(invalid("choice is not continue, change or stop")),
        },
    };
    Ok(Advice { answer, confidence })
}

/// Sessions are offered as `s0`, `s1`, … so names need no escaping.
fn option_id(i: usize) -> String {
    format!("s{i}")
}

fn unit(x: f64) -> bool {
    x.is_finite() && (0.0..=1.0).contains(&x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> AdviceRequest {
        AdviceRequest::ToolCall {
            tool: "Bash".into(),
            summary: "make deploy".into(),
        }
    }

    fn session() -> AdviceRequest {
        AdviceRequest::Session {
            prompt: "fix the css".into(),
            candidates: vec!["api".into(), "web ui".into()],
        }
    }

    #[test]
    fn tool_request_has_model_state_and_a_choice_question() {
        let body = request_body("jev-1.14.0", &tool());
        assert_eq!(body["model"], "jev-1.14.0");
        assert_eq!(body["state"]["tool"], "Bash");
        assert_eq!(body["state"]["subject"], "make deploy");
        let q = &body["questions"]["verdict"];
        assert_eq!(q["type"], "choice");
        assert!(q["instructions"].as_str().unwrap().contains("Choose only"));
        assert!(q["criteria"]["allow"].is_string());
        assert!(q["criteria"]["deny"].is_string());
        let keys: Vec<_> = body.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 3, "{keys:?}");
    }

    #[test]
    fn session_request_offers_candidates_by_id() {
        let body = request_body("typesafe/jev-1.14", &session());
        assert_eq!(body["state"]["prompt"], "fix the css");
        let c = &body["questions"]["session"]["criteria"];
        assert_eq!(c["s0"]["name"], "api");
        assert_eq!(c["s1"]["name"], "web ui");
    }

    #[test]
    fn choice_answers_are_read() {
        let body = r#"{"id":"d","answers":{"verdict":{"type":"choice","choice":"deny",
            "probabilities":{"allow":0.08,"deny":0.92},"confidence":0.9}},
            "usage":{"input_tokens":10,"output_tokens":2}}"#;
        assert_eq!(
            parse_reply(body, &tool()).unwrap(),
            Advice {
                answer: AdviceAnswer::Deny,
                confidence: 0.9
            }
        );
        let body = r#"{"answers":{"session":{"type":"choice","choice":"s1",
            "probabilities":{"s0":0.2,"s1":0.8}}}}"#;
        let advice = parse_reply(body, &session()).unwrap();
        assert_eq!(advice.answer, AdviceAnswer::Session("web ui".into()));
        assert_eq!(advice.confidence, 0.8, "falls back to the top probability");
    }

    #[test]
    fn recovery_asks_continue_change_or_stop() {
        let req = AdviceRequest::Recovery {
            situation: "loop: Bash `cargo test` failed the same way 2 times".into(),
            tried: vec!["nudge: waiting for the agent's next call".into()],
        };
        let body = request_body("jev-1.14.0", &req);
        assert_eq!(
            body["state"]["tried"][0],
            "nudge: waiting for the agent's next call"
        );
        let c = &body["questions"]["recovery"]["criteria"];
        assert!(c["continue"].is_string() && c["change"].is_string() && c["stop"].is_string());
        for (choice, answer) in [
            ("continue", AdviceAnswer::Continue),
            ("change", AdviceAnswer::ChangeApproach),
            ("stop", AdviceAnswer::Stop),
        ] {
            let reply = format!(
                r#"{{"answers":{{"recovery":{{"type":"choice","choice":"{choice}","probabilities":{{"{choice}":0.9}}}}}}}}"#
            );
            assert_eq!(parse_reply(&reply, &req).unwrap().answer, answer);
        }
        let e = parse_reply(
            r#"{"answers":{"recovery":{"choice":"maybe","probabilities":{"maybe":1}}}}"#,
            &req,
        )
        .unwrap_err();
        assert!(e.to_string().contains("continue, change or stop"), "{e}");
    }

    #[test]
    fn a_choice_answer_under_another_id_is_accepted() {
        let body = r#"{"answers":{"tool":{"type":"choice","choice":"allow",
            "probabilities":{"allow":1},"confidence":1}}}"#;
        assert_eq!(
            parse_reply(body, &tool()).unwrap().answer,
            AdviceAnswer::Allow
        );
    }

    #[test]
    fn malformed_replies_are_rejected() {
        let cases = [
            ("not json", "not JSON"),
            ("{}", "no answers"),
            (r#"{"answers":{}}"#, "no choice"),
            (
                r#"{"answers":{"verdict":{"choice":1,"probabilities":{}}}}"#,
                "not a string",
            ),
            (
                r#"{"answers":{"verdict":{"choice":"allow"}}}"#,
                "no probabilities",
            ),
            (
                r#"{"answers":{"verdict":{"choice":"allow","probabilities":{"allow":1.5}}}}"#,
                "outside",
            ),
            (
                r#"{"answers":{"verdict":{"choice":"allow","probabilities":{"allow":1},"confidence":"high"}}}"#,
                "confidence",
            ),
            (
                r#"{"answers":{"verdict":{"choice":"maybe","probabilities":{"maybe":1}}}}"#,
                "allow or deny",
            ),
        ];
        for (body, why) in cases {
            let e = parse_reply(body, &tool()).unwrap_err();
            assert!(e.to_string().contains(why), "{body}: {e}");
        }
        let e = parse_reply(
            r#"{"answers":{"session":{"choice":"s9","probabilities":{"s9":1}}}}"#,
            &session(),
        )
        .unwrap_err();
        assert!(e.to_string().contains("not a candidate"), "{e}");
    }
}
