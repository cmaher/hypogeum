use std::io;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::subscriber::SetGlobalDefaultError;
use tracing_subscriber::filter::ParseError;

#[derive(Debug, thiserror::Error)]
pub enum LoggingError {
    #[error("Invalid log format: {0}")]
    InvalidFormat(String),
    #[error("Invalid log filter: {0}")]
    InvalidFilter(#[from] ParseError),
    #[error("Failed to set global default subscriber: {0}")]
    Init(#[from] SetGlobalDefaultError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}

impl std::str::FromStr for LogFormat {
    type Err = LoggingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("text") {
            Ok(Self::Text)
        } else if s.eq_ignore_ascii_case("json") {
            Ok(Self::Json)
        } else {
            Err(LoggingError::InvalidFormat(s.to_string()))
        }
    }
}

impl<'de> serde::Deserialize<'de> for LogFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse::<LogFormat>().map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for LogFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text => write!(f, "text"),
            Self::Json => write!(f, "json"),
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct TestWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl TestWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn output_bytes(&self) -> Vec<u8> {
        self.buf.lock().expect("lock test writer buffer").clone()
    }

    pub fn output_string(&self) -> String {
        String::from_utf8(self.output_bytes()).expect("test writer output must be valid utf-8")
    }

    pub fn lines(&self) -> Vec<String> {
        self.output_string()
            .lines()
            .map(|s| s.to_string())
            .collect()
    }

    pub fn clear(&self) {
        self.buf.lock().expect("lock test writer buffer").clear();
    }
}

impl io::Write for TestWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.lock().expect("lock test writer buffer").write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.buf.lock().expect("lock test writer buffer").flush()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TestWriter {
    type Writer = TestWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

pub fn create_subscriber<W>(
    filter: &str,
    format: LogFormat,
    writer: W,
) -> Result<Box<dyn tracing::Subscriber + Send + Sync>, LoggingError>
where
    W: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Send + Sync + 'static,
{
    let env_filter = tracing_subscriber::EnvFilter::try_new(filter)?;
    match format {
        LogFormat::Text => {
            let subscriber = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_writer(writer)
                .finish();
            Ok(Box::new(subscriber))
        }
        LogFormat::Json => {
            let subscriber = tracing_subscriber::fmt()
                .json()
                .with_env_filter(env_filter)
                .with_writer(writer)
                .finish();
            Ok(Box::new(subscriber))
        }
    }
}

pub fn create_dispatch<W>(
    filter: &str,
    format: LogFormat,
    writer: W,
) -> Result<tracing::Dispatch, LoggingError>
where
    W: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Send + Sync + 'static,
{
    let subscriber = create_subscriber(filter, format, writer)?;
    Ok(tracing::Dispatch::new(subscriber))
}

/// Keeps two inert dispatchers registered for the life of the test process.
///
/// While at most one dispatcher is registered, tracing-core computes callsite
/// interest from the current thread's default without taking its registry
/// lock. Parallel tests that use scoped subscribers (or none) then race: a
/// callsite first hit on a thread with no subscriber caches `never` for every
/// thread, and concurrent rebuilds can overwrite each other. With two extra
/// dispatchers always registered, every rebuild takes the locked path and
/// combines interest from all live subscribers. The inert dispatchers enable
/// nothing, so they never change what a test subscriber records.
fn pin_callsite_registry() {
    use tracing_subscriber::layer::SubscriberExt;

    static PINNED: OnceLock<[tracing::Dispatch; 2]> = OnceLock::new();
    PINNED.get_or_init(|| {
        let inert = || {
            tracing::Dispatch::new(
                tracing_subscriber::registry().with(tracing_subscriber::filter::LevelFilter::OFF),
            )
        };
        [inert(), inert()]
    });
}

pub fn create_test_subscriber(
    filter: &str,
    format: LogFormat,
) -> Result<(Box<dyn tracing::Subscriber + Send + Sync>, TestWriter), LoggingError> {
    pin_callsite_registry();
    let writer = TestWriter::new();
    let subscriber = create_subscriber(filter, format, writer.clone())?;
    Ok((subscriber, writer))
}

pub fn create_test_dispatch(
    filter: &str,
    format: LogFormat,
) -> Result<(tracing::Dispatch, TestWriter), LoggingError> {
    let (subscriber, writer) = create_test_subscriber(filter, format)?;
    Ok((tracing::Dispatch::new(subscriber), writer))
}

pub fn init(filter: &str, format: LogFormat) -> Result<(), LoggingError> {
    let subscriber = create_subscriber(filter, format, std::io::stderr)?;
    tracing::subscriber::set_global_default(subscriber)?;
    Ok(())
}
