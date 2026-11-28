//! 6-level diagnostic logger and HTTP/SOCKS access-log sinks.
//!
//! * Diagnostics ([`Logger`]) go to `stderr`, filtered by [`LogLevel`].
//! * Access records ([`HttpAccessRecord`], [`SocksLogRecord`]) go through the async
//!   [`AccessLogger`] to their configured files (append mode, buffered) and are ALSO dumped
//!   to `stdout` when the level is `DEBUG` or `VERBOSE`.
//!
//! Embedders can also observe every record as structured data: attach an
//! [`AccessRecordSink`] with [`AccessLogger::with_sink`] (or a channel with
//! [`AccessLogger::with_channel`]) to receive each [`AccessRecord`] alongside the formatted
//! text/JSON lines, which are unchanged.
//!
//! Every access/SOCKS record carries routing visibility: `rule`, `profile`, and a redacted
//! `upstream` (credentials are never emitted).

use std::fmt::{self, Write as _};
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use time::format_description::FormatItem;
use time::macros::format_description;
use time::{OffsetDateTime, UtcOffset};
use tokio::io::{AsyncWrite, AsyncWriteExt, BufWriter};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// Diagnostic verbosity, ordered from least to most verbose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    Quiet,
    Fatal,
    Error,
    Info,
    Debug,
    Verbose,
}

impl LogLevel {
    pub const ALL: [LogLevel; 6] = [
        LogLevel::Quiet,
        LogLevel::Fatal,
        LogLevel::Error,
        LogLevel::Info,
        LogLevel::Debug,
        LogLevel::Verbose,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Quiet => "QUIET",
            LogLevel::Fatal => "FATAL",
            LogLevel::Error => "ERROR",
            LogLevel::Info => "INFO",
            LogLevel::Debug => "DEBUG",
            LogLevel::Verbose => "VERBOSE",
        }
    }

    /// Whether access/SOCKS records are additionally dumped to `stdout` at this level.
    pub fn dumps_access_to_stdout(self) -> bool {
        self >= LogLevel::Debug
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        LogLevel::ALL
            .into_iter()
            .find(|l| l.as_str().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| {
                format!("invalid log level {s:?} (expected QUIET, FATAL, ERROR, INFO, DEBUG, or VERBOSE)")
            })
    }
}

/// Access/SOCKS log record format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogFormat {
    /// Human-readable logfmt (`key=value` pairs).
    Text,
    /// NDJSON: one JSON object per line.
    Json,
}

impl LogFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            LogFormat::Text => "text",
            LogFormat::Json => "json",
        }
    }
}

impl fmt::Display for LogFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LogFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "text" => Ok(LogFormat::Text),
            "json" => Ok(LogFormat::Json),
            _ => Err(format!("invalid log format {s:?} (expected text or json)")),
        }
    }
}

const TS_FORMAT: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");

/// RFC 3339 UTC timestamp with millisecond precision, e.g. `2026-11-22T13:30:01.123Z`.
pub fn format_timestamp(ts: OffsetDateTime) -> String {
    ts.to_offset(UtcOffset::UTC)
        .format(TS_FORMAT)
        .unwrap_or_else(|_| ts.unix_timestamp().to_string())
}

// ---------------------------------------------------------------------------
// Diagnostic logger (stderr)
// ---------------------------------------------------------------------------

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Level-filtered diagnostic logger writing to `stderr` (or an injected writer in tests).
#[derive(Clone)]
pub struct Logger {
    level: LogLevel,
    out: SharedWriter,
}

impl fmt::Debug for Logger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Logger")
            .field("level", &self.level)
            .finish_non_exhaustive()
    }
}

impl Logger {
    /// Logger writing diagnostics to `stderr`.
    pub fn stderr(level: LogLevel) -> Self {
        Self::with_writer(level, Box::new(io::stderr()))
    }

    /// Logger writing diagnostics to an arbitrary writer.
    pub fn with_writer(level: LogLevel, out: Box<dyn Write + Send>) -> Self {
        Self {
            level,
            out: Arc::new(Mutex::new(out)),
        }
    }

    pub fn level(&self) -> LogLevel {
        self.level
    }

    /// Whether a message at `level` would be written. `QUIET` messages are never emitted.
    pub fn enabled(&self, level: LogLevel) -> bool {
        level != LogLevel::Quiet && level <= self.level
    }

    /// Write a diagnostic line (`<ts> <LEVEL> <message>`) if `level` is enabled.
    pub fn log(&self, level: LogLevel, msg: impl fmt::Display) {
        if !self.enabled(level) {
            return;
        }
        let line = format!(
            "{} {:<7} {}\n",
            format_timestamp(OffsetDateTime::now_utc()),
            level.as_str(),
            msg
        );
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = out.write_all(line.as_bytes());
        let _ = out.flush();
    }

    pub fn fatal(&self, msg: impl fmt::Display) {
        self.log(LogLevel::Fatal, msg);
    }
    pub fn error(&self, msg: impl fmt::Display) {
        self.log(LogLevel::Error, msg);
    }
    pub fn info(&self, msg: impl fmt::Display) {
        self.log(LogLevel::Info, msg);
    }
    pub fn debug(&self, msg: impl fmt::Display) {
        self.log(LogLevel::Debug, msg);
    }
    pub fn verbose(&self, msg: impl fmt::Display) {
        self.log(LogLevel::Verbose, msg);
    }

    /// Write pre-built [`fmt::Arguments`] at `level` (no intermediate `String`). Pair with
    /// the crate's `log_*!` macros, which skip building the arguments when `level` is off.
    pub fn log_fmt(&self, level: LogLevel, args: fmt::Arguments<'_>) {
        self.log(level, args);
    }

    pub fn flush(&self) {
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = out.flush();
    }
}

/// Shared body of the `log_*!` macros: checks the level before evaluating the format
/// arguments, so a disabled message costs one comparison and no allocation.
macro_rules! log_at {
    ($level:expr, $logger:expr, $($arg:tt)+) => {{
        let logger: &$crate::logging::Logger = &$logger;
        let level = $level;
        if logger.enabled(level) {
            logger.log_fmt(level, ::std::format_args!($($arg)+));
        }
    }};
}

/// `log_error!(logger, "fmt", args...)` — level-checked `ERROR` diagnostic.
#[allow(unused_macros)]
macro_rules! log_error {
    ($logger:expr, $($arg:tt)+) => {
        $crate::logging::log_at!($crate::logging::LogLevel::Error, $logger, $($arg)+)
    };
}

/// `log_info!(logger, "fmt", args...)` — level-checked `INFO` diagnostic.
#[allow(unused_macros)]
macro_rules! log_info {
    ($logger:expr, $($arg:tt)+) => {
        $crate::logging::log_at!($crate::logging::LogLevel::Info, $logger, $($arg)+)
    };
}

/// `log_debug!(logger, "fmt", args...)` — level-checked `DEBUG` diagnostic.
macro_rules! log_debug {
    ($logger:expr, $($arg:tt)+) => {
        $crate::logging::log_at!($crate::logging::LogLevel::Debug, $logger, $($arg)+)
    };
}

/// `log_verbose!(logger, "fmt", args...)` — level-checked `VERBOSE` diagnostic.
macro_rules! log_verbose {
    ($logger:expr, $($arg:tt)+) => {
        $crate::logging::log_at!($crate::logging::LogLevel::Verbose, $logger, $($arg)+)
    };
}

#[allow(unused_imports)]
pub(crate) use {log_at, log_debug, log_error, log_info, log_verbose};

// ---------------------------------------------------------------------------
// Access records
// ---------------------------------------------------------------------------

/// Optional traffic direction tag emitted right after `kind=` in access logs (used by
/// consumers that run both forward and reverse listeners, such as `rusty-conduit-ssh`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProxyDirection {
    Forward,
    Reverse,
}

impl ProxyDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Reverse => "reverse",
        }
    }
}

impl fmt::Display for ProxyDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One HTTP/HTTPS access log entry (`kind=http`).
///
/// `rule`, `profile`, and `upstream` carry the routing decision that served the request;
/// `upstream` is always redacted so credentials never reach the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpAccessRecord {
    pub ts: OffsetDateTime,
    /// Optional direction tag (`forward` / `reverse`), omitted when `None`.
    pub direction: Option<ProxyDirection>,
    pub client: SocketAddr,
    /// `http` or `https`.
    pub proto: String,
    pub mitm: bool,
    pub method: String,
    pub target: String,
    /// Routing rule name that matched (empty when routed by the default).
    pub rule: String,
    /// Selected upstream profile name (e.g. `bastion`, `corp`, `direct`).
    pub profile: String,
    /// Redacted upstream address (`scheme://host:port`, never `user:pass@`).
    pub upstream: String,
    pub status: u16,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub duration_ms: u64,
}

/// One SOCKS connection log entry (`kind=socks`).
///
/// `rule`, `profile`, and `upstream` carry the routing decision that served the connection;
/// `upstream` is always redacted so credentials never reach the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocksLogRecord {
    pub ts: OffsetDateTime,
    /// Optional direction tag (`forward` / `reverse`), omitted when `None`.
    pub direction: Option<ProxyDirection>,
    pub client: SocketAddr,
    /// `socks4`, `socks4a`, or `socks5`.
    pub proto: String,
    pub cmd: String,
    pub target: String,
    /// Routing rule name that matched (empty when routed by the default).
    pub rule: String,
    /// Selected upstream profile name (e.g. `corp`, `direct`).
    pub profile: String,
    /// Redacted upstream address (`scheme://host:port`, never `user:pass@`).
    pub upstream: String,
    /// e.g. `ok`, `blocked_web_in_mitm`, or an error label.
    pub status: String,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub duration_ms: u64,
}

/// A structured access record, as delivered to an [`AccessRecordSink`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessRecord {
    Http(HttpAccessRecord),
    Socks(SocksLogRecord),
}

impl AccessRecord {
    /// `http` or `socks` (the `kind` field of the formatted line).
    pub fn kind(&self) -> &'static str {
        match self {
            AccessRecord::Http(_) => "http",
            AccessRecord::Socks(_) => "socks",
        }
    }

    /// Routing rule name that matched (empty when routed by the default).
    pub fn rule(&self) -> &str {
        match self {
            AccessRecord::Http(r) => &r.rule,
            AccessRecord::Socks(r) => &r.rule,
        }
    }

    /// Selected upstream profile name.
    pub fn profile(&self) -> &str {
        match self {
            AccessRecord::Http(r) => &r.profile,
            AccessRecord::Socks(r) => &r.profile,
        }
    }

    /// Bytes received from the client.
    pub fn bytes_in(&self) -> u64 {
        match self {
            AccessRecord::Http(r) => r.bytes_in,
            AccessRecord::Socks(r) => r.bytes_in,
        }
    }

    /// Bytes sent to the client.
    pub fn bytes_out(&self) -> u64 {
        match self {
            AccessRecord::Http(r) => r.bytes_out,
            AccessRecord::Socks(r) => r.bytes_out,
        }
    }

    /// Optional traffic direction tag (`forward` / `reverse`).
    pub fn direction(&self) -> Option<ProxyDirection> {
        match self {
            AccessRecord::Http(r) => r.direction,
            AccessRecord::Socks(r) => r.direction,
        }
    }

    /// Encode as one line (without trailing newline), exactly as the file sinks write it.
    pub fn format(&self, format: LogFormat) -> String {
        self.format_with(format, true)
    }

    /// Encode as one line, optionally omitting the routing fields (`rule`, `profile`, `upstream`).
    pub fn format_with(&self, format: LogFormat, include_routing_fields: bool) -> String {
        match self {
            AccessRecord::Http(r) => r.format_with(format, include_routing_fields),
            AccessRecord::Socks(r) => r.format_with(format, include_routing_fields),
        }
    }
}

/// Callback receiving every structured [`AccessRecord`]. It runs synchronously on the task
/// that emits the record, so it must be cheap and must not block (hand work off to a
/// channel or another task instead).
pub type AccessRecordSink = Arc<dyn Fn(&AccessRecord) + Send + Sync>;

/// A single field value, shared by the text and JSON encoders so both carry identical fields.
/// Built on the stack per field: encoding a record allocates only its output buffer.
enum Value<'a> {
    Str(&'a str),
    Ts(OffsetDateTime),
    Addr(SocketAddr),
    Bool(bool),
    U64(u64),
}

/// Displays a timestamp exactly as [`format_timestamp`] does, without an intermediate `String`.
struct Timestamp(OffsetDateTime);

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ts = self.0.to_offset(UtcOffset::UTC);
        if (0..=9999).contains(&ts.year()) {
            write!(
                f,
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
                ts.year(),
                u8::from(ts.month()),
                ts.day(),
                ts.hour(),
                ts.minute(),
                ts.second(),
                ts.millisecond()
            )
        } else {
            f.write_str(&format_timestamp(ts))
        }
    }
}

impl Serialize for Value<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Str(v) => s.serialize_str(v),
            Value::Ts(v) => s.collect_str(&Timestamp(*v)),
            Value::Addr(v) => s.collect_str(v),
            Value::Bool(v) => s.serialize_bool(*v),
            Value::U64(v) => s.serialize_u64(*v),
        }
    }
}

/// Receives a record's fields in emission order.
trait FieldVisitor {
    fn field(&mut self, key: &'static str, value: Value<'_>);
}

/// The single definition of each record's field order, shared by both encoders.
trait Fields {
    fn visit_fields(&self, include_routing_fields: bool, v: &mut impl FieldVisitor);
}

impl Fields for HttpAccessRecord {
    fn visit_fields(&self, include_routing_fields: bool, v: &mut impl FieldVisitor) {
        v.field("ts", Value::Ts(self.ts));
        v.field("kind", Value::Str("http"));
        if let Some(dir) = self.direction {
            v.field("direction", Value::Str(dir.as_str()));
        }
        v.field("client", Value::Addr(self.client));
        v.field("proto", Value::Str(&self.proto));
        v.field("mitm", Value::Bool(self.mitm));
        v.field("method", Value::Str(&self.method));
        v.field("target", Value::Str(&self.target));
        if include_routing_fields {
            v.field("rule", Value::Str(&self.rule));
            v.field("profile", Value::Str(&self.profile));
            v.field("upstream", Value::Str(&self.upstream));
        }
        v.field("status", Value::U64(u64::from(self.status)));
        v.field("bytes_in", Value::U64(self.bytes_in));
        v.field("bytes_out", Value::U64(self.bytes_out));
        v.field("duration_ms", Value::U64(self.duration_ms));
    }
}

impl Fields for SocksLogRecord {
    fn visit_fields(&self, include_routing_fields: bool, v: &mut impl FieldVisitor) {
        v.field("ts", Value::Ts(self.ts));
        v.field("kind", Value::Str("socks"));
        if let Some(dir) = self.direction {
            v.field("direction", Value::Str(dir.as_str()));
        }
        v.field("client", Value::Addr(self.client));
        v.field("proto", Value::Str(&self.proto));
        v.field("cmd", Value::Str(&self.cmd));
        v.field("target", Value::Str(&self.target));
        if include_routing_fields {
            v.field("rule", Value::Str(&self.rule));
            v.field("profile", Value::Str(&self.profile));
            v.field("upstream", Value::Str(&self.upstream));
        }
        v.field("status", Value::Str(&self.status));
        v.field("bytes_in", Value::U64(self.bytes_in));
        v.field("bytes_out", Value::U64(self.bytes_out));
        v.field("duration_ms", Value::U64(self.duration_ms));
    }
}

/// Write `v` as a logfmt value, quoting it when it is empty or contains spaces, quotes, `=`,
/// or control chars.
fn write_logfmt_value(out: &mut String, v: &str) {
    let needs_quotes = v.is_empty()
        || v.chars()
            .any(|c| c == ' ' || c == '"' || c == '=' || c.is_control());
    if !needs_quotes {
        out.push_str(v);
        return;
    }
    out.reserve(v.len() + 2);
    out.push('"');
    for c in v.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// logfmt encoder writing straight into one pre-sized line buffer.
struct TextEncoder {
    line: String,
}

impl FieldVisitor for TextEncoder {
    fn field(&mut self, key: &'static str, value: Value<'_>) {
        if !self.line.is_empty() {
            self.line.push(' ');
        }
        self.line.push_str(key);
        self.line.push('=');
        match value {
            Value::Str(s) => write_logfmt_value(&mut self.line, s),
            // Timestamps and socket addresses never contain characters that need quoting.
            Value::Ts(ts) => {
                let _ = write!(self.line, "{}", Timestamp(ts));
            }
            Value::Addr(a) => {
                let _ = write!(self.line, "{a}");
            }
            Value::Bool(b) => self.line.push_str(if b { "true" } else { "false" }),
            Value::U64(n) => {
                let _ = write!(self.line, "{n}");
            }
        }
    }
}

/// JSON encoder feeding fields straight into a serde map (no intermediate field list).
struct MapVisitor<'m, M: SerializeMap> {
    map: &'m mut M,
    err: Option<M::Error>,
}

impl<M: SerializeMap> FieldVisitor for MapVisitor<'_, M> {
    fn field(&mut self, key: &'static str, value: Value<'_>) {
        if self.err.is_none() {
            if let Err(e) = self.map.serialize_entry(key, &value) {
                self.err = Some(e);
            }
        }
    }
}

/// Zero-allocation `Serialize` view of a record as one JSON object.
struct JsonRecord<'a, R> {
    record: &'a R,
    include_routing_fields: bool,
}

impl<R: Fields> Serialize for JsonRecord<'_, R> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(None)?;
        let mut visitor = MapVisitor {
            map: &mut map,
            err: None,
        };
        self.record
            .visit_fields(self.include_routing_fields, &mut visitor);
        if let Some(e) = visitor.err {
            return Err(e);
        }
        map.end()
    }
}

/// Initial capacity of an encoded line; typical records fit without regrowing.
const LINE_CAPACITY: usize = 256;

fn encode(record: &impl Fields, format: LogFormat, include_routing_fields: bool) -> String {
    match format {
        LogFormat::Text => {
            let mut enc = TextEncoder {
                line: String::with_capacity(LINE_CAPACITY),
            };
            record.visit_fields(include_routing_fields, &mut enc);
            enc.line
        }
        LogFormat::Json => {
            let mut buf = Vec::with_capacity(LINE_CAPACITY);
            let view = JsonRecord {
                record,
                include_routing_fields,
            };
            match serde_json::to_writer(&mut buf, &view) {
                // serde_json only ever writes valid UTF-8.
                Ok(()) => String::from_utf8(buf)
                    .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
                Err(e) => format!("{{\"log_encode_error\":{:?}}}", e.to_string()),
            }
        }
    }
}

impl HttpAccessRecord {
    /// Encode as one line (without trailing newline).
    pub fn format(&self, format: LogFormat) -> String {
        self.format_with(format, true)
    }

    /// Encode as one line, optionally omitting `rule`, `profile`, and `upstream`.
    pub fn format_with(&self, format: LogFormat, include_routing_fields: bool) -> String {
        encode(self, format, include_routing_fields)
    }
}

impl SocksLogRecord {
    /// Encode as one line (without trailing newline).
    pub fn format(&self, format: LogFormat) -> String {
        self.format_with(format, true)
    }

    /// Encode as one line, optionally omitting `rule`, `profile`, and `upstream`.
    pub fn format_with(&self, format: LogFormat, include_routing_fields: bool) -> String {
        encode(self, format, include_routing_fields)
    }
}

// ---------------------------------------------------------------------------
// Async access logger
// ---------------------------------------------------------------------------

/// Access-log sink configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessLogConfig {
    pub format: LogFormat,
    pub http_access_log: Option<PathBuf>,
    pub socks_log: Option<PathBuf>,
    /// Also dump every record to `stdout` (true at `DEBUG` / `VERBOSE`).
    pub dump_stdout: bool,
    /// Whether formatted lines include the routing fields (`rule`, `profile`, `upstream`).
    /// Defaults to `true`; consumers without user-visible routing rules (`rusty-conduit`,
    /// `rusty-conduit-ssh`) set this to `false` to keep their existing log schema.
    pub include_routing_fields: bool,
}

impl AccessLogConfig {
    pub fn new(
        level: LogLevel,
        format: LogFormat,
        http_access_log: Option<PathBuf>,
        socks_log: Option<PathBuf>,
    ) -> Self {
        Self {
            format,
            http_access_log,
            socks_log,
            dump_stdout: level.dumps_access_to_stdout(),
            include_routing_fields: true,
        }
    }

    /// Control whether formatted access lines include `rule`, `profile`, and `upstream`.
    pub fn with_routing_fields(mut self, include: bool) -> Self {
        self.include_routing_fields = include;
        self
    }
}

type BoxWriter = Box<dyn AsyncWrite + Send + Unpin>;

enum Command {
    Http(String),
    Socks(String),
    Flush(oneshot::Sender<io::Result<()>>),
}

struct Sinks {
    http: Option<BufWriter<tokio::fs::File>>,
    socks: Option<BufWriter<tokio::fs::File>>,
    stdout: Option<BufWriter<BoxWriter>>,
}

impl Sinks {
    async fn write(&mut self, http: bool, line: &str) -> io::Result<()> {
        let file = if http {
            self.http.as_mut()
        } else {
            self.socks.as_mut()
        };
        if let Some(f) = file {
            f.write_all(line.as_bytes()).await?;
            f.write_all(b"\n").await?;
        }
        if let Some(out) = self.stdout.as_mut() {
            out.write_all(line.as_bytes()).await?;
            out.write_all(b"\n").await?;
            // stdout dumps are interactive diagnostics: keep them prompt.
            out.flush().await?;
        }
        Ok(())
    }

    async fn handle(&mut self, cmd: Command, errors: &Logger) {
        match cmd {
            Command::Http(line) => {
                if let Err(e) = self.write(true, &line).await {
                    errors.error(format!("http access log write failed: {e}"));
                }
            }
            Command::Socks(line) => {
                if let Err(e) = self.write(false, &line).await {
                    errors.error(format!("socks log write failed: {e}"));
                }
            }
            Command::Flush(ack) => {
                let _ = ack.send(self.flush().await);
            }
        }
    }

    async fn flush(&mut self) -> io::Result<()> {
        if let Some(f) = self.http.as_mut() {
            f.flush().await?;
        }
        if let Some(f) = self.socks.as_mut() {
            f.flush().await?;
        }
        if let Some(f) = self.stdout.as_mut() {
            f.flush().await?;
        }
        Ok(())
    }
}

async fn open_append(path: &Path) -> io::Result<BufWriter<tokio::fs::File>> {
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
    Ok(BufWriter::new(file))
}

/// Async HTTP/SOCKS access logger backed by a single writer task.
///
/// Cheap to clone; all clones feed the same task. Call [`AccessLogger::shutdown`] (or at
/// least [`AccessLogger::flush`]) before exit so buffered file output is persisted.
#[derive(Clone)]
pub struct AccessLogger {
    format: LogFormat,
    include_routing_fields: bool,
    direction: Option<ProxyDirection>,
    http_active: bool,
    socks_active: bool,
    tx: Option<mpsc::Sender<Command>>,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
    errors: Logger,
    sink: Option<AccessRecordSink>,
}

impl fmt::Debug for AccessLogger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessLogger")
            .field("format", &self.format)
            .field("include_routing_fields", &self.include_routing_fields)
            .field("direction", &self.direction)
            .field("http_active", &self.http_active)
            .field("socks_active", &self.socks_active)
            .field("sink", &self.sink.is_some())
            .finish_non_exhaustive()
    }
}

const CHANNEL_CAPACITY: usize = 4096;

impl AccessLogger {
    /// Open the configured sinks, dumping to the process `stdout` when enabled.
    /// Must be called inside a Tokio runtime.
    pub async fn start(config: AccessLogConfig, errors: Logger) -> io::Result<Self> {
        Self::start_with_stdout(config, Box::new(tokio::io::stdout()), errors).await
    }

    /// Like [`AccessLogger::start`] but with an injected `stdout` writer (used by tests).
    pub async fn start_with_stdout(
        config: AccessLogConfig,
        stdout: BoxWriter,
        errors: Logger,
    ) -> io::Result<Self> {
        let http = match &config.http_access_log {
            Some(p) => Some(open_append(p).await?),
            None => None,
        };
        let socks = match &config.socks_log {
            Some(p) => Some(open_append(p).await?),
            None => None,
        };
        let stdout = config.dump_stdout.then(|| BufWriter::new(stdout));
        let http_active = http.is_some() || stdout.is_some();
        let socks_active = socks.is_some() || stdout.is_some();

        if !http_active && !socks_active {
            return Ok(Self {
                format: config.format,
                include_routing_fields: config.include_routing_fields,
                direction: None,
                http_active,
                socks_active,
                tx: None,
                task: Arc::new(Mutex::new(None)),
                errors,
                sink: None,
            });
        }

        let (tx, mut rx) = mpsc::channel::<Command>(CHANNEL_CAPACITY);
        let mut sinks = Sinks {
            http,
            socks,
            stdout,
        };
        let task_errors = errors.clone();
        let task = tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                sinks.handle(cmd, &task_errors).await;
                // Drain everything already queued, then flush once the queue is empty: busy
                // periods still write in large blocks, and a quiet proxy's records reach the
                // files immediately instead of waiting for the buffer to fill.
                while let Ok(cmd) = rx.try_recv() {
                    sinks.handle(cmd, &task_errors).await;
                }
                if let Err(e) = sinks.flush().await {
                    task_errors.error(format!("access log flush failed: {e}"));
                }
            }
            if let Err(e) = sinks.flush().await {
                task_errors.error(format!("access log flush failed: {e}"));
            }
        });

        Ok(Self {
            format: config.format,
            include_routing_fields: config.include_routing_fields,
            direction: None,
            http_active,
            socks_active,
            tx: Some(tx),
            task: Arc::new(Mutex::new(Some(task))),
            errors,
            sink: None,
        })
    }

    /// Tag every record emitted through this logger handle with `direction` (unless the
    /// record already carries one). Clones share the same underlying writer task.
    pub fn with_direction(mut self, direction: ProxyDirection) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Control whether formatted lines written through this handle include `rule`,
    /// `profile`, and `upstream`.
    pub fn with_routing_fields(mut self, include: bool) -> Self {
        self.include_routing_fields = include;
        self
    }

    /// Also deliver every record as a structured [`AccessRecord`] to `sink`, alongside the
    /// formatted lines (which are unchanged). Records reach the sink even when no file or
    /// `stdout` output is configured. Replaces any previously attached sink.
    pub fn with_sink(mut self, sink: AccessRecordSink) -> Self {
        self.sink = Some(sink);
        self
    }

    /// Like [`Self::with_sink`], forwarding each record to an unbounded channel. Records are
    /// dropped silently once the receiver is gone.
    pub fn with_channel(self, tx: mpsc::UnboundedSender<AccessRecord>) -> Self {
        self.with_sink(Arc::new(move |record: &AccessRecord| {
            let _ = tx.send(record.clone());
        }))
    }

    /// Whether a structured record sink is attached.
    pub fn has_sink(&self) -> bool {
        self.sink.is_some()
    }

    /// Whether HTTP records go anywhere (file, stdout, or structured sink).
    pub fn http_enabled(&self) -> bool {
        self.http_active || self.sink.is_some()
    }

    /// Whether SOCKS records go anywhere (file, stdout, or structured sink).
    pub fn socks_enabled(&self) -> bool {
        self.socks_active || self.sink.is_some()
    }

    pub fn format(&self) -> LogFormat {
        self.format
    }

    async fn send(&self, cmd: Command) {
        if let Some(tx) = &self.tx {
            if tx.send(cmd).await.is_err() {
                self.errors
                    .error("access logger is shut down; dropping record");
            }
        }
    }

    /// Record an HTTP/HTTPS access entry.
    pub async fn log_http(&self, record: &HttpAccessRecord) {
        let mut owned;
        let record = if record.direction.is_none() && self.direction.is_some() {
            owned = record.clone();
            owned.direction = self.direction;
            &owned
        } else {
            record
        };
        if let Some(sink) = &self.sink {
            sink(&AccessRecord::Http(record.clone()));
        }
        if self.http_active {
            self.send(Command::Http(
                record.format_with(self.format, self.include_routing_fields),
            ))
            .await;
        }
    }

    /// Non-blocking [`Self::log_http`]: delivers `record` to the sink (if any) and queues
    /// its formatted line with `try_send`. Returns the formatted line only when the writer
    /// queue is full, so the caller can finish delivery with [`Self::send_http_line`].
    pub(crate) fn try_log_http(&self, record: &HttpAccessRecord) -> Option<String> {
        let mut owned;
        let record = if record.direction.is_none() && self.direction.is_some() {
            owned = record.clone();
            owned.direction = self.direction;
            &owned
        } else {
            record
        };
        if let Some(sink) = &self.sink {
            sink(&AccessRecord::Http(record.clone()));
        }
        if !self.http_active {
            return None;
        }
        let tx = self.tx.as_ref()?;
        let line = record.format_with(self.format, self.include_routing_fields);
        match tx.try_send(Command::Http(line)) {
            Ok(()) => None,
            Err(mpsc::error::TrySendError::Full(Command::Http(line))) => Some(line),
            Err(mpsc::error::TrySendError::Full(_)) => None,
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.errors
                    .error("access logger is shut down; dropping record");
                None
            }
        }
    }

    /// Queue an already formatted HTTP line, waiting for room (see [`Self::try_log_http`]).
    pub(crate) async fn send_http_line(&self, line: String) {
        self.send(Command::Http(line)).await;
    }

    /// Record a SOCKS connection entry.
    pub async fn log_socks(&self, record: &SocksLogRecord) {
        let mut owned;
        let record = if record.direction.is_none() && self.direction.is_some() {
            owned = record.clone();
            owned.direction = self.direction;
            &owned
        } else {
            record
        };
        if let Some(sink) = &self.sink {
            sink(&AccessRecord::Socks(record.clone()));
        }
        if self.socks_active {
            self.send(Command::Socks(
                record.format_with(self.format, self.include_routing_fields),
            ))
            .await;
        }
    }

    /// Flush all buffered output to the underlying files / stdout.
    pub async fn flush(&self) -> io::Result<()> {
        let Some(tx) = &self.tx else {
            return Ok(());
        };
        let (ack_tx, ack_rx) = oneshot::channel();
        if tx.send(Command::Flush(ack_tx)).await.is_err() {
            return Ok(());
        }
        ack_rx.await.unwrap_or(Ok(()))
    }

    /// Flush and wait for the writer task to finish. Records sent by other clones after this
    /// returns are dropped with an `ERROR` diagnostic.
    pub async fn shutdown(&self) -> io::Result<()> {
        let result = self.flush().await;
        let task = self.task.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};
    use std::time::Duration;
    use time::macros::datetime;

    /// Shared in-memory buffer usable as both a sync and async writer.
    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl Buf {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl AsyncWrite for Buf {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            b: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.0.lock().unwrap().extend_from_slice(b);
            Poll::Ready(Ok(b.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    fn temp_path(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "rusty-conduit-log-test-{}-{}-{tag}.log",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ))
    }

    fn http_record() -> HttpAccessRecord {
        HttpAccessRecord {
            ts: datetime!(2026-11-22 13:30:01.123456 UTC),
            direction: None,
            client: "127.0.0.1:51234".parse().unwrap(),
            proto: "https".into(),
            mitm: true,
            method: "GET".into(),
            target: "https://example.com/api/v1".into(),
            rule: "internal-domains-with-failover".into(),
            profile: "bastion".into(),
            upstream: "socks5://127.0.0.1:1081".into(),
            status: 200,
            bytes_in: 142,
            bytes_out: 4096,
            duration_ms: 84,
        }
    }

    fn socks_record() -> SocksLogRecord {
        SocksLogRecord {
            ts: datetime!(2026-11-22 13:30:02.456 UTC),
            direction: None,
            client: "127.0.0.1:51240".parse().unwrap(),
            proto: "socks5".into(),
            cmd: "CONNECT".into(),
            target: "github.com:22".into(),
            rule: "rfc1918-to-corp".into(),
            profile: "corp".into(),
            upstream: "http://10.0.0.2:3128".into(),
            status: "ok".into(),
            bytes_in: 1280,
            bytes_out: 8192,
            duration_ms: 1520,
        }
    }

    #[test]
    fn level_parsing_is_case_insensitive_and_ordered() {
        for (s, l) in [
            ("quiet", LogLevel::Quiet),
            ("FATAL", LogLevel::Fatal),
            ("Error", LogLevel::Error),
            ("info", LogLevel::Info),
            ("DeBuG", LogLevel::Debug),
            ("VERBOSE", LogLevel::Verbose),
        ] {
            assert_eq!(s.parse::<LogLevel>().unwrap(), l);
            assert_eq!(l.to_string().parse::<LogLevel>().unwrap(), l);
        }
        assert!("trace".parse::<LogLevel>().is_err());
        assert!(LogLevel::Quiet < LogLevel::Fatal);
        assert!(LogLevel::Fatal < LogLevel::Error);
        assert!(LogLevel::Error < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Verbose);
        assert_eq!("TEXT".parse::<LogFormat>().unwrap(), LogFormat::Text);
        assert_eq!("json".parse::<LogFormat>().unwrap(), LogFormat::Json);
        assert!("xml".parse::<LogFormat>().is_err());
    }

    #[test]
    fn stdout_dump_only_at_debug_and_verbose() {
        for l in LogLevel::ALL {
            let expected = matches!(l, LogLevel::Debug | LogLevel::Verbose);
            assert_eq!(l.dumps_access_to_stdout(), expected, "{l}");
        }
    }

    #[test]
    fn timestamp_is_rfc3339_millis_utc() {
        assert_eq!(
            format_timestamp(datetime!(2026-11-22 13:30:01.123456 UTC)),
            "2026-11-22T13:30:01.123Z"
        );
        assert_eq!(
            format_timestamp(datetime!(2026-11-22 15:30:01 +02:00)),
            "2026-11-22T13:30:01.000Z"
        );
    }

    #[test]
    fn inline_timestamp_matches_format_timestamp() {
        for ts in [
            datetime!(2026-11-22 13:30:01.123456 UTC),
            datetime!(2026-11-22 15:30:01 +02:00),
            datetime!(2026-01-01 00:00:00.999999 -05:30),
            datetime!(0001-02-03 04:05:06.007 UTC),
            datetime!(9999-12-31 23:59:59.999 UTC),
        ] {
            assert_eq!(Timestamp(ts).to_string(), format_timestamp(ts));
        }
    }

    #[test]
    fn logfmt_escapes_control_chars_in_place() {
        let mut out = String::from("k=");
        write_logfmt_value(&mut out, "a\u{1}\"b\\\n");
        assert_eq!(out, "k=\"a\\u{1}\\\"b\\\\\\n\"");
        let mut out = String::new();
        write_logfmt_value(&mut out, "");
        assert_eq!(out, "\"\"");
    }

    #[test]
    fn http_record_text_format() {
        assert_eq!(
            http_record().format(LogFormat::Text),
            "ts=2026-11-22T13:30:01.123Z kind=http client=127.0.0.1:51234 proto=https mitm=true \
             method=GET target=https://example.com/api/v1 rule=internal-domains-with-failover \
             profile=bastion upstream=socks5://127.0.0.1:1081 status=200 bytes_in=142 \
             bytes_out=4096 duration_ms=84"
        );
    }

    #[test]
    fn socks_record_text_format() {
        assert_eq!(
            socks_record().format(LogFormat::Text),
            "ts=2026-11-22T13:30:02.456Z kind=socks client=127.0.0.1:51240 proto=socks5 \
             cmd=CONNECT target=github.com:22 rule=rfc1918-to-corp profile=corp \
             upstream=http://10.0.0.2:3128 status=ok bytes_in=1280 bytes_out=8192 \
             duration_ms=1520"
        );
    }

    #[test]
    fn text_format_quotes_special_values() {
        let mut r = http_record();
        r.target = "http://x/a b?q=\"1\"".into();
        r.method = String::new();
        let line = r.format(LogFormat::Text);
        assert!(line.contains(r#"target="http://x/a b?q=\"1\"""#), "{line}");
        assert!(line.contains(r#"method="""#), "{line}");
        assert!(!line.contains('\n'));
        r.target = "evil\nkind=forged".into();
        assert!(!r.format(LogFormat::Text).contains('\n'));
    }

    #[test]
    fn json_formats_have_identical_fields() {
        let v: serde_json::Value =
            serde_json::from_str(&http_record().format(LogFormat::Json)).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "ts": "2026-11-22T13:30:01.123Z", "kind": "http", "client": "127.0.0.1:51234",
                "proto": "https", "mitm": true, "method": "GET",
                "target": "https://example.com/api/v1", "rule": "internal-domains-with-failover",
                "profile": "bastion", "upstream": "socks5://127.0.0.1:1081", "status": 200,
                "bytes_in": 142, "bytes_out": 4096, "duration_ms": 84
            })
        );
        let line = socks_record().format(LogFormat::Json);
        assert!(!line.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "ts": "2026-11-22T13:30:02.456Z", "kind": "socks", "client": "127.0.0.1:51240",
                "proto": "socks5", "cmd": "CONNECT", "target": "github.com:22",
                "rule": "rfc1918-to-corp", "profile": "corp",
                "upstream": "http://10.0.0.2:3128", "status": "ok",
                "bytes_in": 1280, "bytes_out": 8192, "duration_ms": 1520
            })
        );
        assert!(
            line.starts_with(r#"{"ts":"#),
            "field order preserved: {line}"
        );
    }

    #[test]
    fn diagnostic_level_filtering() {
        let all = [
            LogLevel::Fatal,
            LogLevel::Error,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Verbose,
        ];
        for configured in LogLevel::ALL {
            let buf = Buf::default();
            let logger = Logger::with_writer(configured, Box::new(buf.clone()));
            for l in all {
                logger.log(l, format!("msg-{l}"));
            }
            logger.log(LogLevel::Quiet, "never");
            let out = buf.text();
            for l in all {
                assert_eq!(
                    out.contains(&format!("msg-{l}")),
                    l <= configured,
                    "configured={configured} msg={l}"
                );
            }
            assert!(!out.contains("never"));
            if configured == LogLevel::Quiet {
                assert!(out.is_empty());
            }
        }
    }

    #[test]
    fn diagnostic_line_shape() {
        let buf = Buf::default();
        let logger = Logger::with_writer(LogLevel::Info, Box::new(buf.clone()));
        logger.info("hello world");
        logger.fatal("boom");
        let out = buf.text();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(" INFO    hello world"), "{}", lines[0]);
        assert!(lines[1].ends_with(" FATAL   boom"), "{}", lines[1]);
        assert!(lines[0].as_bytes()[4] == b'-' && lines[0].contains('T'));
    }

    #[tokio::test]
    async fn routes_to_files_only_below_debug() {
        for level in [
            LogLevel::Quiet,
            LogLevel::Fatal,
            LogLevel::Error,
            LogLevel::Info,
        ] {
            let http = temp_path("http");
            let socks = temp_path("socks");
            let stdout = Buf::default();
            let cfg = AccessLogConfig::new(
                level,
                LogFormat::Text,
                Some(http.clone()),
                Some(socks.clone()),
            );
            assert!(!cfg.dump_stdout);
            let logger = AccessLogger::start_with_stdout(
                cfg,
                Box::new(stdout.clone()),
                Logger::with_writer(LogLevel::Quiet, Box::new(io::sink())),
            )
            .await
            .unwrap();
            logger.log_http(&http_record()).await;
            logger.log_socks(&socks_record()).await;
            logger.shutdown().await.unwrap();

            let h = std::fs::read_to_string(&http).unwrap();
            let s = std::fs::read_to_string(&socks).unwrap();
            assert_eq!(h, format!("{}\n", http_record().format(LogFormat::Text)));
            assert_eq!(s, format!("{}\n", socks_record().format(LogFormat::Text)));
            assert!(stdout.text().is_empty(), "no stdout dump at {level}");
            std::fs::remove_file(http).ok();
            std::fs::remove_file(socks).ok();
        }
    }

    #[tokio::test]
    async fn routes_to_files_and_stdout_at_debug_and_verbose() {
        for level in [LogLevel::Debug, LogLevel::Verbose] {
            let http = temp_path("http");
            let socks = temp_path("socks");
            let stdout = Buf::default();
            let cfg = AccessLogConfig::new(
                level,
                LogFormat::Json,
                Some(http.clone()),
                Some(socks.clone()),
            );
            assert!(cfg.dump_stdout);
            let logger = AccessLogger::start_with_stdout(
                cfg,
                Box::new(stdout.clone()),
                Logger::with_writer(LogLevel::Quiet, Box::new(io::sink())),
            )
            .await
            .unwrap();
            logger.log_http(&http_record()).await;
            logger.log_socks(&socks_record()).await;
            logger.shutdown().await.unwrap();

            let hline = http_record().format(LogFormat::Json);
            let sline = socks_record().format(LogFormat::Json);
            assert_eq!(
                std::fs::read_to_string(&http).unwrap(),
                format!("{hline}\n")
            );
            assert_eq!(
                std::fs::read_to_string(&socks).unwrap(),
                format!("{sline}\n")
            );
            assert_eq!(stdout.text(), format!("{hline}\n{sline}\n"), "at {level}");
            std::fs::remove_file(http).ok();
            std::fs::remove_file(socks).ok();
        }
    }

    #[tokio::test]
    async fn stdout_only_when_no_files_configured() {
        let stdout = Buf::default();
        let logger = AccessLogger::start_with_stdout(
            AccessLogConfig::new(LogLevel::Debug, LogFormat::Text, None, None),
            Box::new(stdout.clone()),
            Logger::with_writer(LogLevel::Quiet, Box::new(io::sink())),
        )
        .await
        .unwrap();
        assert!(logger.http_enabled() && logger.socks_enabled());
        logger.log_http(&http_record()).await;
        logger.flush().await.unwrap();
        assert_eq!(
            stdout.text(),
            format!("{}\n", http_record().format(LogFormat::Text))
        );
        logger.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn inactive_when_nothing_configured() {
        let stdout = Buf::default();
        let logger = AccessLogger::start_with_stdout(
            AccessLogConfig::new(LogLevel::Info, LogFormat::Text, None, None),
            Box::new(stdout.clone()),
            Logger::with_writer(LogLevel::Quiet, Box::new(io::sink())),
        )
        .await
        .unwrap();
        assert!(!logger.http_enabled() && !logger.socks_enabled());
        logger.log_http(&http_record()).await;
        logger.log_socks(&socks_record()).await;
        logger.flush().await.unwrap();
        logger.shutdown().await.unwrap();
        assert!(stdout.text().is_empty());
    }

    #[tokio::test]
    async fn file_sinks_flush_when_the_queue_drains() {
        let http = temp_path("http-idle");
        let socks = temp_path("socks-idle");
        let logger = AccessLogger::start_with_stdout(
            AccessLogConfig::new(
                LogLevel::Info,
                LogFormat::Text,
                Some(http.clone()),
                Some(socks.clone()),
            ),
            Box::new(Buf::default()),
            Logger::with_writer(LogLevel::Quiet, Box::new(io::sink())),
        )
        .await
        .unwrap();
        logger.log_http(&http_record()).await;
        logger.log_socks(&socks_record()).await;
        // No flush() and no shutdown(): the writer task flushes on its own once idle.
        let want_http = format!("{}\n", http_record().format(LogFormat::Text));
        let want_socks = format!("{}\n", socks_record().format(LogFormat::Text));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let got_http = std::fs::read_to_string(&http).unwrap_or_default();
            let got_socks = std::fs::read_to_string(&socks).unwrap_or_default();
            if got_http == want_http && got_socks == want_socks {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "records not flushed while idle: http={got_http:?} socks={got_socks:?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        logger.shutdown().await.unwrap();
        std::fs::remove_file(http).ok();
        std::fs::remove_file(socks).ok();
    }

    #[tokio::test]
    async fn file_sinks_append_and_flush_persists() {
        let http = temp_path("http");
        std::fs::write(&http, "existing\n").unwrap();
        let logger = AccessLogger::start_with_stdout(
            AccessLogConfig::new(LogLevel::Info, LogFormat::Text, Some(http.clone()), None),
            Box::new(Buf::default()),
            Logger::with_writer(LogLevel::Quiet, Box::new(io::sink())),
        )
        .await
        .unwrap();
        assert!(logger.http_enabled());
        assert!(!logger.socks_enabled());
        logger.log_http(&http_record()).await;
        logger.log_socks(&socks_record()).await; // no socks sink: dropped
        logger.flush().await.unwrap();
        let text = std::fs::read_to_string(&http).unwrap();
        assert_eq!(
            text,
            format!("existing\n{}\n", http_record().format(LogFormat::Text))
        );
        logger.shutdown().await.unwrap();
        std::fs::remove_file(http).ok();
    }

    #[tokio::test]
    async fn unopenable_log_path_is_an_error() {
        let err = AccessLogger::start_with_stdout(
            AccessLogConfig::new(
                LogLevel::Info,
                LogFormat::Text,
                Some(PathBuf::from("/definitely/not/a/dir/http.log")),
                None,
            ),
            Box::new(Buf::default()),
            Logger::with_writer(LogLevel::Quiet, Box::new(io::sink())),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("/definitely/not/a/dir/http.log"));
    }

    #[tokio::test]
    async fn structured_sink_receives_records_alongside_unchanged_lines() {
        let http = temp_path("http");
        let socks = temp_path("socks");
        let seen: Arc<Mutex<Vec<AccessRecord>>> = Arc::default();
        let sink_seen = Arc::clone(&seen);
        let logger = AccessLogger::start_with_stdout(
            AccessLogConfig::new(
                LogLevel::Info,
                LogFormat::Json,
                Some(http.clone()),
                Some(socks.clone()),
            ),
            Box::new(Buf::default()),
            Logger::with_writer(LogLevel::Quiet, Box::new(io::sink())),
        )
        .await
        .unwrap()
        .with_sink(Arc::new(move |r: &AccessRecord| {
            sink_seen.lock().unwrap().push(r.clone())
        }));
        assert!(logger.has_sink());
        logger.log_http(&http_record()).await;
        logger.log_socks(&socks_record()).await;
        logger.shutdown().await.unwrap();

        let seen = seen.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec![
                AccessRecord::Http(http_record()),
                AccessRecord::Socks(socks_record())
            ]
        );
        assert_eq!(seen[0].kind(), "http");
        assert_eq!(seen[1].kind(), "socks");
        assert_eq!(seen[0].rule(), "internal-domains-with-failover");
        assert_eq!(seen[0].profile(), "bastion");
        assert_eq!(seen[0].bytes_in(), http_record().bytes_in);
        assert_eq!(seen[1].bytes_out(), socks_record().bytes_out);
        // Formatted output is byte-identical to the logger without a sink.
        let hline = http_record().format(LogFormat::Json);
        assert_eq!(seen[0].format(LogFormat::Json), hline);
        assert_eq!(
            std::fs::read_to_string(&http).unwrap(),
            format!("{hline}\n")
        );
        assert_eq!(
            std::fs::read_to_string(&socks).unwrap(),
            format!("{}\n", socks_record().format(LogFormat::Json))
        );
        std::fs::remove_file(http).ok();
        std::fs::remove_file(socks).ok();
    }

    #[tokio::test]
    async fn sink_only_logger_is_enabled_and_writes_nothing() {
        let stdout = Buf::default();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let logger = AccessLogger::start_with_stdout(
            AccessLogConfig::new(LogLevel::Info, LogFormat::Text, None, None),
            Box::new(stdout.clone()),
            Logger::with_writer(LogLevel::Quiet, Box::new(io::sink())),
        )
        .await
        .unwrap();
        assert!(!logger.http_enabled() && !logger.socks_enabled());
        let logger = logger.with_channel(tx);
        assert!(logger.http_enabled() && logger.socks_enabled());
        logger.log_http(&http_record()).await;
        logger.log_socks(&socks_record()).await;
        logger.shutdown().await.unwrap();
        assert_eq!(rx.recv().await, Some(AccessRecord::Http(http_record())));
        assert_eq!(rx.recv().await, Some(AccessRecord::Socks(socks_record())));
        assert!(stdout.text().is_empty());

        // A dropped receiver does not break logging.
        drop(rx);
        logger.log_http(&http_record()).await;
    }

    #[tokio::test]
    async fn try_log_http_queues_without_blocking_and_hands_back_on_full() {
        let stdout = Buf::default();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let logger = AccessLogger::start_with_stdout(
            AccessLogConfig::new(LogLevel::Debug, LogFormat::Text, None, None),
            Box::new(stdout.clone()),
            Logger::with_writer(LogLevel::Quiet, Box::new(io::sink())),
        )
        .await
        .unwrap()
        .with_channel(tx);
        // On the current-thread test runtime the writer task cannot run until we yield, so
        // the queue fills after exactly CHANNEL_CAPACITY records.
        for _ in 0..CHANNEL_CAPACITY {
            assert_eq!(logger.try_log_http(&http_record()), None);
        }
        let line = logger.try_log_http(&http_record());
        assert_eq!(line, Some(http_record().format(LogFormat::Text)));
        logger.send_http_line(line.unwrap()).await;
        logger.shutdown().await.unwrap();
        let expected = format!("{}\n", http_record().format(LogFormat::Text));
        assert_eq!(stdout.text(), expected.repeat(CHANNEL_CAPACITY + 1));
        for _ in 0..=CHANNEL_CAPACITY {
            assert_eq!(rx.recv().await, Some(AccessRecord::Http(http_record())));
        }
    }

    #[tokio::test]
    async fn routing_fields_toggle_and_direction_tag_work_in_text_json_and_sink() {
        let stdout = Buf::default();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let base = AccessLogger::start_with_stdout(
            AccessLogConfig::new(LogLevel::Debug, LogFormat::Text, None, None)
                .with_routing_fields(false),
            Box::new(stdout.clone()),
            Logger::with_writer(LogLevel::Quiet, Box::new(io::sink())),
        )
        .await
        .unwrap()
        .with_channel(tx);

        let fwd = base.clone().with_direction(ProxyDirection::Forward);
        let rev = base.clone().with_direction(ProxyDirection::Reverse);

        fwd.log_http(&http_record()).await;
        rev.log_socks(&socks_record()).await;
        base.shutdown().await.unwrap();

        let text = stdout.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(
            lines[0].contains("kind=http direction=forward client="),
            "{}",
            lines[0]
        );
        assert!(
            lines[0].contains("target=https://example.com/api/v1 status=200"),
            "{}",
            lines[0]
        );
        assert!(
            !lines[0].contains("rule=")
                && !lines[0].contains("profile=")
                && !lines[0].contains("upstream="),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].contains("kind=socks direction=reverse client="),
            "{}",
            lines[1]
        );
        assert!(
            lines[1].contains("target=github.com:22 status=ok"),
            "{}",
            lines[1]
        );

        let first = rx.recv().await.unwrap();
        let second = rx.recv().await.unwrap();
        assert_eq!(first.direction(), Some(ProxyDirection::Forward));
        assert_eq!(second.direction(), Some(ProxyDirection::Reverse));

        let json: serde_json::Value =
            serde_json::from_str(&first.format_with(LogFormat::Json, false)).unwrap();
        let obj = json.as_object().unwrap();
        assert_eq!(obj.len(), 12);
        assert_eq!(obj["direction"], "forward");
        assert!(!obj.contains_key("rule"));
    }
}
