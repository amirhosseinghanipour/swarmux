//! `swarmux jev ...`: storing a key and trying the provider, plus building
//! the advisor for the app.

use std::io::{BufRead, IsTerminal};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, bail};
use swarmux_application::ports::{AdviceAnswer, AdviceRequest, DecisionAdvisor};
use swarmux_application::{JevProvider, JevSettings};
use swarmux_infrastructure::jev::keys::{self, KeySource};
use swarmux_infrastructure::{Endpoint, JevHttp, Paths, load_settings};
use tokio::runtime::Handle;

/// The advisor for the configured provider, or `None` when JEV is off.
/// A missing key still gives an advisor; it reports `has_key() == false`.
pub fn advisor(
    settings: &JevSettings,
    rt: Handle,
) -> anyhow::Result<Option<(JevHttp, Option<KeySource>)>> {
    let Some(endpoint) = Endpoint::for_provider(settings.provider) else {
        return Ok(None);
    };
    let found = keys::find_key(settings.provider);
    let source = found.as_ref().map(|(_, s)| *s);
    let advisor = JevHttp::new(
        settings.provider.as_str(),
        endpoint,
        found.map(|(k, _)| k),
        Duration::from_millis(settings.timeout_ms()),
        rt,
    )
    .map_err(anyhow::Error::msg)?;
    Ok(Some((advisor, source)))
}

/// `swarmux jev set-key <provider>`: reads the key without echo and keeps it
/// in the OS keychain.
pub fn set_key(provider: &str) -> anyhow::Result<()> {
    let provider = parse_provider(provider)?;
    let key = if std::io::stdin().is_terminal() {
        rpassword::prompt_password(format!("{} API key (not shown): ", provider.as_str()))
            .context("cannot read the key")?
    } else {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .context("cannot read the key from stdin")?;
        line
    };
    keys::store_key(provider, &key).map_err(anyhow::Error::msg)?;
    println!(
        "saved the {} key in the OS keychain (service \"swarmux\", account \"{}\")",
        provider.as_str(),
        provider.as_str()
    );
    if let Some(var) = keys::env_var(provider)
        && std::env::var_os(var).is_some()
    {
        println!("note: ${var} is set and is used before the keychain");
    }
    Ok(())
}

/// `swarmux jev test [provider]`: one sample decision with no data of yours.
pub fn test(provider: Option<&str>) -> anyhow::Result<()> {
    let paths = Paths::default_home()?;
    let mut settings = load_settings(&paths.config)?.jev;
    if let Some(p) = provider {
        let chosen = parse_provider(p)?;
        if chosen != settings.provider {
            settings.timeout_ms = None;
        }
        settings.provider = chosen;
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()?;
    let Some((advisor, source)) = advisor(&settings, rt.handle().clone())? else {
        bail!("JEV is off: set [jev] provider in config.toml, or run `swarmux jev test typesafe`");
    };
    let Some(source) = source else {
        let var = keys::env_var(settings.provider).unwrap_or_default();
        bail!(
            "no {} key: run `swarmux jev set-key {}` or set {var}",
            advisor.provider(),
            advisor.provider()
        );
    };
    println!(
        "asking {} at {} (key from {source}, timeout {} ms)",
        advisor.provider(),
        advisor.host(),
        settings.timeout_ms()
    );
    println!(
        "sample: may `Bash cargo test --workspace` run without asking? (no data of yours is sent)"
    );
    let (tx, rx) = mpsc::channel();
    advisor.advise(
        AdviceRequest::ToolCall {
            tool: "Bash".into(),
            summary: "cargo test --workspace".into(),
        },
        Box::new(move |reply| {
            let _ = tx.send(reply);
        }),
    );
    let wait = Duration::from_millis(settings.timeout_ms() + 2_000);
    let reply = rx.recv_timeout(wait).context("no reply from the adapter")?;
    match reply.result {
        Ok(advice) => {
            let verdict = match advice.answer {
                AdviceAnswer::Allow => "allow",
                AdviceAnswer::Deny => "deny",
                _ => "?",
            };
            let applied = if advice.confidence >= settings.threshold {
                "applied"
            } else {
                "below the threshold: you would be asked"
            };
            println!(
                "ok   {verdict} with confidence {:.2} in {} ms ({applied}, threshold {:.2})",
                advice.confidence, reply.latency_ms, settings.threshold
            );
            Ok(())
        }
        Err(e) => bail!("{e} (after {} ms)", reply.latency_ms),
    }
}

fn parse_provider(name: &str) -> anyhow::Result<JevProvider> {
    match JevProvider::parse(name.trim()) {
        Some(JevProvider::Off) | None => {
            bail!("unknown provider {name:?}: use typesafe or openrouter")
        }
        Some(p) => Ok(p),
    }
}
