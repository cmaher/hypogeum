use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvFile {
    Default(PathBuf),
    Required(PathBuf),
    Disabled,
}

pub fn resolve_env_file(setting: Option<OsString>) -> EnvFile {
    match setting {
        None => EnvFile::Default(PathBuf::from("env/local.env")),
        Some(s) if s.is_empty() => EnvFile::Disabled,
        Some(s) => EnvFile::Required(PathBuf::from(s)),
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("Environment configuration file is missing: {}", path.display())]
    EnvFileMissing { path: PathBuf },
    #[error("Environment configuration file is unreadable: {}", path.display())]
    EnvFileUnreadable { path: PathBuf },
    #[error("Environment configuration file is malformed at {} (line index {})", path.display(), line)]
    EnvFileMalformed { path: PathBuf, line: usize },
    #[error("Invalid configuration: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvSource(BTreeMap<String, String>);

impl EnvSource {
    pub fn load() -> Result<Self, ConfigError> {
        let setting = std::env::var_os("APP_ENV_FILE");
        let process_pairs =
            std::env::vars_os().filter_map(|(k, v)| match (k.into_string(), v.into_string()) {
                (Ok(k), Ok(v)) => Some((k, v)),
                _ => None,
            });
        Self::load_from(setting, process_pairs)
    }

    pub fn from_pairs<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        let mut map = BTreeMap::new();
        for (k, v) in pairs {
            map.insert(k.into(), v.into());
        }
        Self(map)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(|s| s.as_str())
    }

    pub fn deserialize<T: serde::de::DeserializeOwned>(
        &self,
        prefix: &str,
    ) -> Result<T, ConfigError> {
        envy::prefixed(prefix)
            .from_iter(self.0.clone())
            .map_err(|err| match err {
                envy::Error::MissingValue(field) => {
                    let field_clean = field.strip_prefix("field ").unwrap_or(field);
                    let var_name = format!("{}{}", prefix, field_clean.to_uppercase());
                    ConfigError::Invalid(format!("missing value for {}", var_name))
                }
                envy::Error::Custom(msg) => ConfigError::Invalid(msg),
            })
    }

    pub fn load_from(
        setting: Option<OsString>,
        process: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ConfigError> {
        let resolved = resolve_env_file(setting);
        let mut map = BTreeMap::new();

        match &resolved {
            EnvFile::Disabled => {}
            EnvFile::Default(path) | EnvFile::Required(path) => {
                match dotenvy::from_path_iter(path) {
                    Ok(iter) => {
                        for item in iter {
                            match item {
                                Ok((k, v)) => {
                                    map.insert(k, v);
                                }
                                Err(dotenvy::Error::LineParse(_, line)) => {
                                    return Err(ConfigError::EnvFileMalformed {
                                        path: path.clone(),
                                        line,
                                    });
                                }
                                Err(_) => {
                                    return Err(ConfigError::EnvFileUnreadable {
                                        path: path.clone(),
                                    });
                                }
                            }
                        }
                    }
                    Err(e) if e.not_found() => {
                        if matches!(resolved, EnvFile::Required(_)) {
                            return Err(ConfigError::EnvFileMissing { path: path.clone() });
                        }
                    }
                    Err(_) => {
                        return Err(ConfigError::EnvFileUnreadable { path: path.clone() });
                    }
                }
            }
        }

        for (k, v) in process {
            map.insert(k, v);
        }

        Ok(Self(map))
    }
}

#[derive(serde::Deserialize, Clone, PartialEq, Eq)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret([redacted])")
    }
}

fn default_log() -> String {
    "info".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct LogConfig {
    #[serde(default = "default_log")]
    pub log: String,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self { log: default_log() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_resolve_env_file() {
        assert_eq!(
            resolve_env_file(None),
            EnvFile::Default(PathBuf::from("env/local.env"))
        );
        assert_eq!(
            resolve_env_file(Some(OsString::from(""))),
            EnvFile::Disabled
        );
        assert_eq!(
            resolve_env_file(Some(OsString::from("custom/path.env"))),
            EnvFile::Required(PathBuf::from("custom/path.env"))
        );
    }

    #[test]
    fn test_load_from_tempfile_and_process_override() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "APP_FOO=file_value").unwrap();
        writeln!(tmp, "APP_BAR=file_bar").unwrap();
        tmp.flush().unwrap();

        let path = tmp.path().as_os_str().to_os_string();
        let process_pairs = vec![("APP_BAR".to_string(), "process_bar".to_string())];

        let env = EnvSource::load_from(Some(path), process_pairs).unwrap();
        assert_eq!(env.get("APP_FOO"), Some("file_value"));
        assert_eq!(env.get("APP_BAR"), Some("process_bar"));
    }

    #[test]
    fn test_default_path_absent_ok_empty() {
        let nonexistent = std::env::temp_dir().join("nonexistent_default_test.env");
        let _ = std::fs::remove_file(&nonexistent);

        // When setting is None, resolve_env_file uses "env/local.env".
        // If that file does not exist, load_from should return Ok with only process pairs.
        let env =
            EnvSource::load_from(None, vec![("PROCESS_VAR".to_string(), "value".to_string())])
                .unwrap();
        assert_eq!(env.get("PROCESS_VAR"), Some("value"));
    }

    #[test]
    fn test_required_path_absent_err_missing() {
        let nonexistent = std::env::temp_dir().join("nonexistent_required_test.env");
        let _ = std::fs::remove_file(&nonexistent);

        let err =
            EnvSource::load_from(Some(nonexistent.as_os_str().to_os_string()), vec![]).unwrap_err();
        assert_eq!(
            err,
            ConfigError::EnvFileMissing {
                path: nonexistent.clone()
            }
        );
    }

    #[test]
    fn test_disabled_env_file() {
        let env = EnvSource::load_from(
            Some(OsString::from("")),
            vec![("PROCESS_VAR".to_string(), "val".to_string())],
        )
        .unwrap();
        assert_eq!(env.get("PROCESS_VAR"), Some("val"));
    }

    #[test]
    fn test_malformed_file_omits_line_content() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        // A line without '=' is malformed in dotenv syntax
        writeln!(tmp, "THIS_LINE_IS_MALFORMED_AND_HAS_SECRET_PASSWORD_12345").unwrap();
        tmp.flush().unwrap();

        let path = tmp.path().as_os_str().to_os_string();
        let err = EnvSource::load_from(Some(path), vec![]).unwrap_err();

        match &err {
            ConfigError::EnvFileMalformed { line, .. } => {
                assert_eq!(*line, 52);
            }
            other => panic!("Expected EnvFileMalformed, got {other:?}"),
        }

        let err_display = format!("{err}");
        assert!(!err_display.contains("SECRET_PASSWORD_12345"));
        assert!(!err_display.contains("THIS_LINE_IS_MALFORMED"));
        assert!(err_display.contains("52"));
    }

    #[test]
    fn test_secret_redaction() {
        let secret = Secret::new("super_sensitive_token");
        assert_eq!(secret.expose(), "super_sensitive_token");

        let debug_output = format!("{secret:?}");
        assert_eq!(debug_output, "Secret([redacted])");
        assert!(!debug_output.contains("super_sensitive_token"));
    }

    #[test]
    fn test_envy_deserialize_and_errors() {
        #[derive(serde::Deserialize, Debug, PartialEq)]
        struct TestConfig {
            required_key: String,
            port: u16,
        }

        // Success case
        let env = EnvSource::from_pairs([("MYAPP_REQUIRED_KEY", "hello"), ("MYAPP_PORT", "8080")]);
        let config: TestConfig = env.deserialize("MYAPP_").unwrap();
        assert_eq!(
            config,
            TestConfig {
                required_key: "hello".to_string(),
                port: 8080,
            }
        );

        // Missing field
        let env = EnvSource::from_pairs([("MYAPP_PORT", "8080")]);
        let err = env.deserialize::<TestConfig>("MYAPP_").unwrap_err();
        match err {
            ConfigError::Invalid(msg) => {
                assert!(msg.to_lowercase().contains("required_key"));
            }
            other => panic!("Expected ConfigError::Invalid, got {other:?}"),
        }

        // Invalid value
        let env = EnvSource::from_pairs([
            ("MYAPP_REQUIRED_KEY", "hello"),
            ("MYAPP_PORT", "not_a_number"),
        ]);
        let err = env.deserialize::<TestConfig>("MYAPP_").unwrap_err();
        match err {
            ConfigError::Invalid(msg) => {
                assert!(msg.to_lowercase().contains("port"));
            }
            other => panic!("Expected ConfigError::Invalid, got {other:?}"),
        }
    }

    #[test]
    fn test_log_config() {
        // Default when APP_LOG is absent
        let env = EnvSource::from_pairs(Vec::<(&str, &str)>::new());
        let log_config: LogConfig = env.deserialize("APP_").unwrap();
        assert_eq!(log_config.log, "info");

        // Overridden when APP_LOG is present
        let env = EnvSource::from_pairs([("APP_LOG", "debug")]);
        let log_config: LogConfig = env.deserialize("APP_").unwrap();
        assert_eq!(log_config.log, "debug");
    }
}
