//! Where releases are fetched from: GitHub, or a folder laid out like one.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use swarmux_application::dist::{DistError, DistResult, ReleaseSource};
use swarmux_domain::dist::Version;

/// The repository releases are published to.
pub const REPOSITORY: &str = "amirhosseinghanipour/swarmux";

/// A release asset is never larger than this.
const MAX_DOWNLOAD: u64 = 300 * 1024 * 1024;

/// Releases on GitHub.
pub struct GithubReleases {
    api: String,
    downloads: String,
    client: reqwest::blocking::Client,
}

impl GithubReleases {
    pub fn new(user_agent: &str) -> DistResult<Self> {
        Self::with_bases(
            user_agent,
            &format!("https://api.github.com/repos/{REPOSITORY}"),
            &format!("https://github.com/{REPOSITORY}/releases/download"),
        )
    }

    /// Releases served from other base addresses (for tests).
    pub fn with_bases(user_agent: &str, api: &str, downloads: &str) -> DistResult<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(user_agent)
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| DistError::Network(e.to_string()))?;
        Ok(Self {
            api: api.trim_end_matches('/').to_string(),
            downloads: downloads.trim_end_matches('/').to_string(),
            client,
        })
    }

    fn get(&self, url: &str) -> DistResult<Vec<u8>> {
        let resp = self
            .client
            .get(url)
            .send()
            .map_err(|e| DistError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(DistError::Network(format!("{url}: {}", resp.status())));
        }
        let mut out = Vec::new();
        resp.take(MAX_DOWNLOAD + 1)
            .read_to_end(&mut out)
            .map_err(|e| DistError::Network(e.to_string()))?;
        if out.len() as u64 > MAX_DOWNLOAD {
            return Err(DistError::Network("the download is too large".into()));
        }
        Ok(out)
    }
}

impl ReleaseSource for GithubReleases {
    fn latest(&self) -> DistResult<Version> {
        let body = self.get(&format!("{}/releases/latest", self.api))?;
        let json: serde_json::Value = serde_json::from_slice(&body)
            .map_err(|e| DistError::Network(format!("unreadable release data: {e}")))?;
        json.get("tag_name")
            .and_then(|t| t.as_str())
            .and_then(Version::parse)
            .ok_or_else(|| DistError::Network("the latest release has no usable version".into()))
    }

    fn fetch(&self, version: &Version, asset: &str) -> DistResult<Vec<u8>> {
        self.get(&format!("{}/{}/{asset}", self.downloads, version.tag()))
    }
}

/// A folder holding `latest.txt` (the newest tag) and the release files.
pub struct DirReleases {
    dir: PathBuf,
}

impl DirReleases {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

impl ReleaseSource for DirReleases {
    fn latest(&self) -> DistResult<Version> {
        let path = self.dir.join("latest.txt");
        let text = std::fs::read_to_string(&path)
            .map_err(|e| DistError::Io(format!("cannot read {}: {e}", path.display())))?;
        Version::parse(&text)
            .ok_or_else(|| DistError::Network(format!("{} has no usable version", path.display())))
    }

    fn fetch(&self, _version: &Version, asset: &str) -> DistResult<Vec<u8>> {
        let path = self.dir.join(asset);
        std::fs::read(&path)
            .map_err(|e| DistError::Io(format!("cannot read {}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn a_folder_is_a_release_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("latest.txt"), "v1.2.3\n").unwrap();
        std::fs::write(dir.path().join("a.zip"), b"zz").unwrap();
        let src = DirReleases::new(dir.path().to_path_buf());
        let v = src.latest().unwrap();
        assert_eq!(v.to_string(), "1.2.3");
        assert_eq!(src.fetch(&v, "a.zip").unwrap(), b"zz");
        assert!(src.fetch(&v, "missing").is_err());
    }

    /// A one-thread HTTP server answering each request from `routes`.
    fn serve(routes: Vec<(&'static str, &'static [u8])>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { return };
                let mut buf = [0u8; 2048];
                let n = s.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                let (status, body) = match routes.iter().find(|(p, _)| *p == path) {
                    Some((_, b)) => ("200 OK", *b),
                    None => ("404 Not Found", &b""[..]),
                };
                let _ = write!(
                    s,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(body);
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn github_style_release_over_http() {
        let base = serve(vec![
            ("/api/releases/latest", br#"{"tag_name":"v0.9.0"}"#),
            ("/dl/v0.9.0/file.tar.gz", b"payload"),
        ]);
        let src = GithubReleases::with_bases("test", &format!("{base}/api"), &format!("{base}/dl"))
            .unwrap();
        let v = src.latest().unwrap();
        assert_eq!(v.to_string(), "0.9.0");
        assert_eq!(src.fetch(&v, "file.tar.gz").unwrap(), b"payload");
        assert!(matches!(src.fetch(&v, "nope"), Err(DistError::Network(_))));
    }
}
