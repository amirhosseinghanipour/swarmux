//! Notification adapters: a Windows toast, terminal sequences, an ntfy
//! push. Nothing here spawns a process or opens a console window.

use std::io::Write;
use std::time::Duration;

use swarmux_application::{
    AppError, AppResult, NoNotifier, Notification, Notifier, NotifySettings, TerminalMode,
};
use swarmux_domain::{AttentionKind, Priority};
use tokio::runtime::Handle;

/// The notifier the settings ask for: every enabled channel at once.
pub fn build_notifier(settings: &NotifySettings, rt: &Handle) -> Box<dyn Notifier> {
    if !settings.enabled {
        return Box::new(NoNotifier);
    }
    let mut channels: Vec<Box<dyn Notifier>> = Vec::new();
    #[cfg(windows)]
    if settings.toast {
        channels.push(Box::new(toast::ToastNotifier));
    }
    if settings.terminal != TerminalMode::Off {
        channels.push(Box::new(TerminalNotifier::new(settings.terminal)));
    }
    if let Some(url) = &settings.ntfy
        && let Ok(ntfy) = NtfyNotifier::new(url, rt.clone())
    {
        channels.push(Box::new(ntfy));
    }
    Box::new(Fanout(channels))
}

/// Tries every channel; reports the first failure.
struct Fanout(Vec<Box<dyn Notifier>>);

impl Notifier for Fanout {
    fn notify(&self, n: &Notification) -> AppResult<()> {
        let mut first = None;
        for channel in &self.0 {
            if let Err(e) = channel.notify(n) {
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(()), Err)
    }
}

#[cfg(windows)]
mod toast {
    use super::*;
    use tauri_winrt_notification::{Duration as ToastDuration, Sound, Toast};

    /// A native toast through the WinRT API. It borrows the stock
    /// PowerShell application id, which Windows already knows how to show;
    /// no PowerShell process is started.
    pub struct ToastNotifier;

    impl Notifier for ToastNotifier {
        fn notify(&self, n: &Notification) -> AppResult<()> {
            let mut toast = Toast::new(Toast::POWERSHELL_APP_ID)
                .title(&n.title)
                .text1(&n.body);
            if n.priority == Priority::P1 {
                toast = toast
                    .sound(Some(Sound::Reminder))
                    .duration(ToastDuration::Long);
            }
            toast
                .show()
                .map_err(|e| AppError::Notify(format!("toast: {e}")))
        }
    }
}

/// Writes a notification escape sequence (or the bell) to the terminal.
pub struct TerminalNotifier {
    mode: TerminalMode,
}

impl TerminalNotifier {
    pub fn new(mode: TerminalMode) -> TerminalNotifier {
        TerminalNotifier { mode }
    }
}

impl Notifier for TerminalNotifier {
    fn notify(&self, n: &Notification) -> AppResult<()> {
        let bytes = terminal_sequence(self.mode, &n.title, &n.body);
        if bytes.is_empty() {
            return Ok(());
        }
        let mut out = std::io::stdout().lock();
        out.write_all(bytes.as_bytes())
            .and_then(|()| out.flush())
            .map_err(|e| AppError::Notify(format!("terminal: {e}")))
    }
}

/// What to write for `mode`. Control characters and the `;` separator are
/// replaced in the text so nothing in it can end the sequence early.
pub(crate) fn terminal_sequence(mode: TerminalMode, title: &str, body: &str) -> String {
    let (title, body) = (clean(title), clean(body));
    match mode {
        TerminalMode::Off => String::new(),
        TerminalMode::Bell => "\x07".to_string(),
        TerminalMode::Osc777 => format!("\x1b]777;notify;{title};{body}\x07"),
        TerminalMode::Osc9 => format!("\x1b]9;{title}: {body}\x07"),
    }
}

fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() || c == ';' { ' ' } else { c })
        .collect()
}

/// Pushes to an ntfy topic. Only a fixed title and the kind of item leave
/// the machine: no command, path, session or prompt text.
pub struct NtfyNotifier {
    url: String,
    client: reqwest::Client,
    rt: Handle,
}

impl NtfyNotifier {
    pub fn new(url: &str, rt: Handle) -> Result<NtfyNotifier, String> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("swarmux/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("cannot set up HTTPS: {e}"))?;
        Ok(NtfyNotifier {
            url: url.to_string(),
            client,
            rt,
        })
    }
}

impl Notifier for NtfyNotifier {
    fn notify(&self, n: &Notification) -> AppResult<()> {
        let push = ntfy_push(n);
        let request = self
            .client
            .post(&self.url)
            .header("Title", push.title)
            .header("Priority", push.priority)
            .header("Tags", push.tag)
            .body(push.body);
        // Fire and forget: a slow server must not stall the interface.
        self.rt.spawn(async move {
            let _ = request.send().await;
        });
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct Push {
    pub title: String,
    pub body: String,
    pub priority: &'static str,
    pub tag: &'static str,
}

pub(crate) fn ntfy_push(n: &Notification) -> Push {
    let label = match n.kind {
        AttentionKind::Approval => "an approval is waiting",
        AttentionKind::Question => "an agent has a question",
        AttentionKind::JobFailed => "a job failed",
        AttentionKind::ReviewConcerns => "a review has concerns",
        AttentionKind::ResultReady => "a result is ready",
        AttentionKind::Stalled => "a job looks stalled",
        AttentionKind::GateEscalated => "the done-gate gave up",
        AttentionKind::Recovery => "the watchdog could not get a job unstuck",
        AttentionKind::PlanReady => "a plan is waiting for approval",
        AttentionKind::Drift => "a build strayed from its plan",
        AttentionKind::Merge => "a task of a mission can't merge",
        AttentionKind::BoardQuestion => "an agent has a question for you",
    };
    let (priority, tag) = match n.priority {
        Priority::P1 => ("urgent", "rotating_light"),
        Priority::P2 => ("high", "warning"),
        Priority::P3 => ("default", "information_source"),
    };
    Push {
        title: format!("swarmux {}", n.priority.as_str()),
        body: label.to_string(),
        priority,
        tag,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::AttentionId;

    fn note(priority: Priority, body: &str) -> Notification {
        Notification {
            id: AttentionId::new("approval:1"),
            priority,
            kind: AttentionKind::Approval,
            title: "swarmux P2: approval".into(),
            body: body.into(),
        }
    }

    #[test]
    fn sequences_are_well_formed() {
        assert_eq!(
            terminal_sequence(TerminalMode::Osc777, "t", "b"),
            "\x1b]777;notify;t;b\x07"
        );
        assert_eq!(
            terminal_sequence(TerminalMode::Osc9, "t", "b"),
            "\x1b]9;t: b\x07"
        );
        assert_eq!(terminal_sequence(TerminalMode::Bell, "t", "b"), "\x07");
        assert_eq!(terminal_sequence(TerminalMode::Off, "t", "b"), "");
    }

    #[test]
    fn text_cannot_break_out_of_the_sequence() {
        let s = terminal_sequence(TerminalMode::Osc777, "t", "x\x07\x1b[2J;evil");
        assert_eq!(s.matches('\x07').count(), 1);
        assert_eq!(s.matches('\x1b').count(), 1);
        assert_eq!(s.matches(';').count(), 3);
    }

    #[test]
    fn push_carries_no_item_text() {
        let p = ntfy_push(&note(Priority::P2, "Bash rm -rf /secret/path"));
        assert_eq!(p.body, "an approval is waiting");
        assert!(!format!("{p:?}").contains("secret"));
        assert_eq!((p.priority, p.title.as_str()), ("high", "swarmux P2"));
        assert_eq!(ntfy_push(&note(Priority::P1, "")).priority, "urgent");
    }

    /// Shows one real toast. Run by hand: `cargo test -p swarmux-infrastructure
    /// -- --ignored toast_shows`.
    #[cfg(windows)]
    #[test]
    #[ignore = "pops a real desktop notification"]
    fn toast_shows() {
        let n = note(Priority::P2, "swarmux toast check");
        toast::ToastNotifier.notify(&n).unwrap();
    }
}
