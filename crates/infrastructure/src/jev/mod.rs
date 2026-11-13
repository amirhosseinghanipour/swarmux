//! `DecisionAdvisor` adapter: JEV over HTTPS (TypeSafe or OpenRouter Decisions),
//! in the format of the open-source jev-layer project.

pub mod keys;
pub mod wire;

use std::time::{Duration, Instant};

use swarmux_application::JevProvider;
use swarmux_application::ports::{
    AdviceError, AdviceReply, AdviceRequest, AdviceSink, DecisionAdvisor,
};
use tokio::runtime::Handle;

use keys::redact;

/// Where a provider is reached and which model it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub url: String,
    pub model: String,
}

impl Endpoint {
    /// The provider's public endpoint (jev-layer's defaults).
    pub fn for_provider(provider: JevProvider) -> Option<Endpoint> {
        let (url, model) = match provider {
            JevProvider::Off => return None,
            JevProvider::TypeSafe => ("https://api.typesafe.ai/v1/systemone", "jev-1.14.0"),
            JevProvider::OpenRouter => (
                "https://openrouter.ai/api/alpha/decisions",
                "typesafe/jev-1.14",
            ),
        };
        Some(Endpoint {
            url: url.into(),
            model: model.into(),
        })
    }

    /// The host part of the URL, for the consent screen.
    pub fn host(&self) -> &str {
        let rest = self.url.split_once("://").map_or(&*self.url, |(_, r)| r);
        rest.split(['/', '?']).next().unwrap_or(rest)
    }
}

/// Asks JEV over HTTPS on the shared async runtime.
pub struct JevHttp {
    provider: String,
    endpoint: Endpoint,
    host: String,
    key: Option<String>,
    timeout: Duration,
    client: reqwest::Client,
    rt: Handle,
}

impl JevHttp {
    /// `key` is `None` when none was found; every request then fails at once.
    pub fn new(
        provider: &str,
        endpoint: Endpoint,
        key: Option<String>,
        timeout: Duration,
        rt: Handle,
    ) -> Result<JevHttp, String> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("swarmux/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("cannot set up HTTPS: {e}"))?;
        Ok(JevHttp {
            provider: provider.to_string(),
            host: endpoint.host().to_string(),
            endpoint,
            key: key.filter(|k| !k.is_empty()),
            timeout,
            client,
            rt,
        })
    }
}

impl DecisionAdvisor for JevHttp {
    fn provider(&self) -> &str {
        &self.provider
    }

    fn host(&self) -> &str {
        &self.host
    }

    fn model(&self) -> &str {
        &self.endpoint.model
    }

    fn prompt_sha256(&self, request: &AdviceRequest) -> String {
        let body = wire::request_body(&self.endpoint.model, request).to_string();
        swarmux_domain::ledger::hex(&swarmux_domain::ledger::sha256(body.as_bytes()))
    }

    fn has_key(&self) -> bool {
        self.key.is_some()
    }

    fn advise(&self, request: AdviceRequest, sink: AdviceSink) {
        let Some(key) = self.key.clone() else {
            sink(AdviceReply {
                result: Err(AdviceError::NoKey(self.provider.clone())),
                latency_ms: 0,
            });
            return;
        };
        let call = Call {
            client: self.client.clone(),
            url: self.endpoint.url.clone(),
            body: wire::request_body(&self.endpoint.model, &request).to_string(),
            key,
            timeout: self.timeout,
        };
        self.rt.spawn(async move {
            let start = Instant::now();
            let result = call.send(&request).await;
            let latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
            sink(AdviceReply { result, latency_ms });
        });
    }
}

struct Call {
    client: reqwest::Client,
    url: String,
    body: String,
    key: String,
    timeout: Duration,
}

impl Call {
    async fn send(
        self,
        request: &AdviceRequest,
    ) -> Result<swarmux_application::ports::Advice, AdviceError> {
        let timeout_ms = u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX);
        let exchange = async {
            let response = self
                .client
                .post(&self.url)
                .bearer_auth(&self.key)
                .header("Content-Type", "application/json")
                .header("X-Title", "swarmux")
                .body(self.body)
                .send()
                .await?;
            let status = response.status();
            let text = response.text().await?;
            Ok::<_, reqwest::Error>((status, text))
        };
        let (status, text) = match tokio::time::timeout(self.timeout, exchange).await {
            Err(_) => return Err(AdviceError::Timeout(timeout_ms)),
            Ok(Err(e)) if e.is_timeout() => return Err(AdviceError::Timeout(timeout_ms)),
            Ok(Err(e)) => {
                let message = e.without_url().to_string();
                return Err(AdviceError::Network(redact(&message, &self.key)));
            }
            Ok(Ok(reply)) => reply,
        };
        if !status.is_success() {
            return Err(AdviceError::Status(status.as_u16()));
        }
        wire::parse_reply(&text, request).map_err(|e| match e {
            AdviceError::Invalid(m) => AdviceError::Invalid(redact(&m, &self.key)),
            other => other,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_endpoints_match_jev_layer() {
        let t = Endpoint::for_provider(JevProvider::TypeSafe).unwrap();
        assert_eq!(t.url, "https://api.typesafe.ai/v1/systemone");
        assert_eq!(
            (t.model.as_str(), t.host()),
            ("jev-1.14.0", "api.typesafe.ai")
        );
        let o = Endpoint::for_provider(JevProvider::OpenRouter).unwrap();
        assert_eq!(o.url, "https://openrouter.ai/api/alpha/decisions");
        assert_eq!(
            (o.model.as_str(), o.host()),
            ("typesafe/jev-1.14", "openrouter.ai")
        );
        assert_eq!(Endpoint::for_provider(JevProvider::Off), None);
    }

    #[test]
    fn host_of_a_local_url_keeps_the_port() {
        let e = Endpoint {
            url: "http://127.0.0.1:4100/v1/systemone".into(),
            model: "m".into(),
        };
        assert_eq!(e.host(), "127.0.0.1:4100");
    }
}
