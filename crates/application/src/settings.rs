use swarmux_domain::{DEFAULT_MODEL, Limits};

/// User settings (`config.toml`); every field has a default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Jobs that may run at the same time.
    pub max_concurrent: usize,
    /// Queued jobs wait while free memory is below this.
    pub min_free_ram_mb: u64,
    /// Model for new sessions.
    pub default_model: String,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            max_concurrent: 3,
            min_free_ram_mb: 1536,
            default_model: DEFAULT_MODEL.to_string(),
        }
    }
}

impl Settings {
    pub fn limits(&self) -> Limits {
        Limits {
            max_concurrent: self.max_concurrent,
            min_free_ram: self.min_free_ram_mb * 1024 * 1024,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_limits() {
        let s = Settings::default();
        assert_eq!((s.max_concurrent, s.default_model.as_str()), (3, "sonnet"));
        assert_eq!(s.limits().min_free_ram, 1536 * 1024 * 1024);
    }
}
