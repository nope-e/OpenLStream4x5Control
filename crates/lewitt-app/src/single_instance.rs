use std::env;
use std::hash::{Hash, Hasher};
use std::io::{self, Read, Write};
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError};
use iced::futures::channel::mpsc;
use iced::futures::stream::BoxStream;
use iced::futures::{SinkExt, StreamExt, executor};
use iced::{Subscription, stream};
#[cfg(target_os = "linux")]
use interprocess::local_socket::GenericFilePath;
#[cfg(target_os = "windows")]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{ListenerNonblockingMode, ListenerOptions, Name};
#[cfg(target_os = "linux")]
use interprocess::os::unix::local_socket::ListenerOptionsExt as _;
use thiserror::Error;

const SHOW_WINDOW_FRAME: &[u8] = b"show-window\n";
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(20);
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(250);
const IO_TIMEOUT: Duration = Duration::from_millis(500);
const CLAIM_RETRY_INTERVAL: Duration = Duration::from_millis(25);
const CLAIM_RETRY_COUNT: usize = 12;
const EVENT_CHANNEL_CAPACITY: usize = 1;
#[cfg(target_os = "windows")]
const WINDOWS_PIPE_PREFIX: &str = "lewitt-ctl-stream4x5-v1";
#[cfg(target_os = "linux")]
const LINUX_SOCKET_NAME: &str = "lewitt-ctl-stream4x5-v1.sock";

pub const MAX_INSTANCE_FRAME_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceCommand {
    ShowWindow,
}

impl InstanceCommand {
    #[must_use]
    pub const fn encode(self) -> &'static [u8] {
        match self {
            Self::ShowWindow => SHOW_WINDOW_FRAME,
        }
    }

    /// Decode one complete, bounded IPC frame.
    pub fn decode(frame: &[u8]) -> Result<Self, InstanceProtocolError> {
        if frame.len() > MAX_INSTANCE_FRAME_LEN {
            return Err(InstanceProtocolError::FrameTooLarge);
        }
        match frame {
            SHOW_WINDOW_FRAME => Ok(Self::ShowWindow),
            _ => Err(InstanceProtocolError::UnknownCommand),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum InstanceProtocolError {
    #[error("single-instance IPC frame exceeds the size limit")]
    FrameTooLarge,
    #[error("unknown single-instance IPC command")]
    UnknownCommand,
}

/// Result of trying to become the desktop application's primary instance.
pub enum InstanceLaunch {
    Primary(PrimaryInstance),
    ExistingNotified,
}

/// Owns the primary instance listener and stops it when the daemon exits.
#[derive(Clone)]
pub struct PrimaryInstance {
    inner: Arc<PrimaryInner>,
}

impl PrimaryInstance {
    fn start(listener: LocalSocketListener) -> Result<Self, InstanceError> {
        let (sender, receiver) = crossbeam_channel::bounded(EVENT_CHANNEL_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("lewitt-instance-listener".into())
            .spawn(move || listener_loop(&listener, &worker_stop, &sender))
            .map_err(InstanceError::ListenerThread)?;

        Ok(Self {
            inner: Arc::new(PrimaryInner {
                stop,
                worker: Mutex::new(Some(worker)),
                events: InstanceEventSource { receiver },
            }),
        })
    }

    pub(crate) fn subscription(&self) -> Subscription<InstanceCommand> {
        Subscription::run_with(self.inner.events.clone(), instance_event_stream)
    }

    #[cfg(test)]
    fn receive_for_test(&self, timeout: Duration) -> Option<InstanceCommand> {
        self.inner.events.receiver.recv_timeout(timeout).ok()
    }
}

struct PrimaryInner {
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
    events: InstanceEventSource,
}

impl Drop for PrimaryInner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Ok(mut worker) = self.worker.lock()
            && let Some(worker) = worker.take()
            && worker.join().is_err()
        {
            tracing::error!("single-instance listener thread panicked during shutdown");
        }
    }
}

#[derive(Clone)]
struct InstanceEventSource {
    receiver: Receiver<InstanceCommand>,
}

impl Hash for InstanceEventSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "lewitt-single-instance-events-v1".hash(state);
    }
}

/// Claims the per-user endpoint or tells the existing process to show itself.
pub fn claim_or_notify() -> Result<InstanceLaunch, InstanceError> {
    claim_endpoint(&InstanceEndpoint::current()?)
}

fn claim_endpoint(endpoint: &InstanceEndpoint) -> Result<InstanceLaunch, InstanceError> {
    match notify_once(endpoint, InstanceCommand::ShowWindow) {
        Ok(()) => return Ok(InstanceLaunch::ExistingNotified),
        Err(error) if endpoint_is_absent(&error) => {}
        Err(error) if endpoint_may_be_starting(&error) => {
            notify_with_retries(endpoint, InstanceCommand::ShowWindow)
                .map_err(InstanceError::NotifyExisting)?;
            return Ok(InstanceLaunch::ExistingNotified);
        }
        Err(error) => return Err(InstanceError::NotifyExisting(error)),
    }

    match create_listener(endpoint, false) {
        Ok(listener) => PrimaryInstance::start(listener).map(InstanceLaunch::Primary),
        Err(error) if endpoint_is_claimed(&error) => {
            match notify_with_retries(endpoint, InstanceCommand::ShowWindow) {
                Ok(()) => Ok(InstanceLaunch::ExistingNotified),
                #[cfg(target_os = "linux")]
                Err(notify_error) if stale_socket_error(&notify_error) => {
                    let listener =
                        create_listener(endpoint, true).map_err(InstanceError::CreateListener)?;
                    PrimaryInstance::start(listener).map(InstanceLaunch::Primary)
                }
                Err(error) => Err(InstanceError::NotifyExisting(error)),
            }
        }
        Err(error) => Err(InstanceError::CreateListener(error)),
    }
}

fn create_listener(
    endpoint: &InstanceEndpoint,
    overwrite_stale: bool,
) -> io::Result<LocalSocketListener> {
    let options = ListenerOptions::new()
        .name(endpoint.name()?)
        .nonblocking(ListenerNonblockingMode::Accept)
        .try_overwrite(overwrite_stale)
        .max_spin_time(IO_TIMEOUT);

    #[cfg(target_os = "linux")]
    let options = options.mode(0o600);

    options.create_sync()
}

fn notify_with_retries(endpoint: &InstanceEndpoint, command: InstanceCommand) -> io::Result<()> {
    let mut last_error = None;
    for _ in 0..CLAIM_RETRY_COUNT {
        match notify_once(endpoint, command) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        thread::sleep(CLAIM_RETRY_INTERVAL);
    }
    Err(last_error.unwrap_or_else(|| io::Error::other("IPC notification retry failed")))
}

fn notify_once(endpoint: &InstanceEndpoint, command: InstanceCommand) -> io::Result<()> {
    let mut stream = LocalSocketStream::connect(endpoint.name()?)?;
    stream.write_all(command.encode())?;
    stream.flush()
}

fn listener_loop(
    listener: &LocalSocketListener,
    stop: &AtomicBool,
    events: &Sender<InstanceCommand>,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok(mut stream) => match receive_command(&mut stream) {
                Ok(command) => match events.try_send(command) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        tracing::debug!("coalesced a duplicate show-window request");
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        tracing::error!("single-instance UI event channel disconnected");
                        break;
                    }
                },
                Err(error) => {
                    tracing::warn!(error = %error, "rejected a single-instance IPC command");
                }
            },
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                tracing::error!(error = %error, "single-instance listener stopped");
                break;
            }
        }
    }
}

fn receive_command(
    stream: &mut LocalSocketStream,
) -> Result<InstanceCommand, InstanceReceiveError> {
    stream.set_nonblocking(true)?;
    let deadline = Instant::now() + IO_TIMEOUT;
    let mut frame = Vec::with_capacity(MAX_INSTANCE_FRAME_LEN + 1);
    let mut chunk = [0_u8; 16];

    loop {
        let remaining = MAX_INSTANCE_FRAME_LEN + 1 - frame.len();
        let read_len = remaining.min(chunk.len());
        let count = match stream.read(&mut chunk[..read_len]) {
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(InstanceReceiveError::Io(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "single-instance command timed out",
                    )));
                }
                thread::sleep(ACCEPT_POLL_INTERVAL);
                continue;
            }
            Err(error) => return Err(InstanceReceiveError::Io(error)),
        };
        if count == 0 {
            break;
        }
        frame.extend_from_slice(&chunk[..count]);

        if frame.len() > MAX_INSTANCE_FRAME_LEN || frame.contains(&b'\n') {
            break;
        }
    }

    InstanceCommand::decode(&frame).map_err(InstanceReceiveError::Protocol)
}

fn instance_event_stream(source: &InstanceEventSource) -> BoxStream<'static, InstanceCommand> {
    let receiver = source.receiver.clone();
    stream::channel(EVENT_CHANNEL_CAPACITY, async |output| {
        let _worker = thread::Builder::new()
            .name("lewitt-instance-events".into())
            .spawn(move || forward_instance_events(&receiver, output));
    })
    .boxed()
}

fn forward_instance_events(
    receiver: &Receiver<InstanceCommand>,
    mut output: mpsc::Sender<InstanceCommand>,
) {
    loop {
        match receiver.recv_timeout(EVENT_POLL_INTERVAL) {
            Ok(command) => {
                if executor::block_on(output.send(command)).is_err() {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if output.is_closed() {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn endpoint_is_absent(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

fn endpoint_may_be_starting(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

fn endpoint_is_claimed(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::AddrInUse | io::ErrorKind::PermissionDenied
    )
}

#[cfg(target_os = "linux")]
fn stale_socket_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionReset
    )
}

#[derive(Clone)]
struct InstanceEndpoint {
    #[cfg(target_os = "windows")]
    name: String,
    #[cfg(target_os = "linux")]
    path: PathBuf,
}

impl InstanceEndpoint {
    fn current() -> Result<Self, InstanceError> {
        #[cfg(target_os = "windows")]
        {
            let user_scope = env::var_os("LOCALAPPDATA")
                .or_else(|| env::var_os("USERPROFILE"))
                .ok_or(InstanceError::MissingUserScope)?;
            let hash = stable_scope_hash(user_scope.as_encoded_bytes());
            Ok(Self {
                name: format!("{WINDOWS_PIPE_PREFIX}-{hash:016x}"),
            })
        }

        #[cfg(target_os = "linux")]
        {
            let runtime_dir = env::var_os("XDG_RUNTIME_DIR")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .ok_or(InstanceError::MissingRuntimeDirectory)?;
            if !runtime_dir.is_absolute() || !runtime_dir.is_dir() {
                return Err(InstanceError::InvalidRuntimeDirectory(runtime_dir));
            }
            Ok(Self {
                path: runtime_dir.join(LINUX_SOCKET_NAME),
            })
        }

        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            Err(InstanceError::UnsupportedPlatform)
        }
    }

    fn name(&self) -> io::Result<Name<'_>> {
        #[cfg(target_os = "windows")]
        {
            self.name.as_str().to_ns_name::<GenericNamespaced>()
        }

        #[cfg(target_os = "linux")]
        {
            self.path.as_path().to_fs_name::<GenericFilePath>()
        }

        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "single-instance IPC is unsupported on this platform",
            ))
        }
    }

    #[cfg(test)]
    fn unique_for_test() -> Self {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

        #[cfg(target_os = "windows")]
        {
            Self {
                name: format!("lewitt-ctl-test-{}-{id}", std::process::id()),
            }
        }

        #[cfg(target_os = "linux")]
        {
            Self {
                path: env::temp_dir()
                    .join(format!("lewitt-ctl-test-{}-{id}.sock", std::process::id())),
            }
        }
    }
}

#[cfg(any(target_os = "windows", test))]
fn stable_scope_hash(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

#[derive(Debug, Error)]
enum InstanceReceiveError {
    #[error("could not read the single-instance command: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Protocol(#[from] InstanceProtocolError),
}

#[derive(Debug, Error)]
pub enum InstanceError {
    #[cfg(target_os = "windows")]
    #[error("neither LOCALAPPDATA nor USERPROFILE is available for per-user IPC")]
    MissingUserScope,
    #[cfg(target_os = "linux")]
    #[error("XDG_RUNTIME_DIR is not set; refusing to place the IPC socket in a public directory")]
    MissingRuntimeDirectory,
    #[cfg(target_os = "linux")]
    #[error("XDG_RUNTIME_DIR is not an existing absolute directory: {0}")]
    InvalidRuntimeDirectory(PathBuf),
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    #[error("single-instance IPC is unsupported on this platform")]
    UnsupportedPlatform,
    #[error("could not create the single-instance listener: {0}")]
    CreateListener(#[source] io::Error),
    #[error("could not notify the existing application instance: {0}")]
    NotifyExisting(#[source] io::Error),
    #[error("could not start the single-instance listener thread: {0}")]
    ListenerThread(#[source] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_command_round_trips() {
        let encoded = InstanceCommand::ShowWindow.encode();
        assert_eq!(
            InstanceCommand::decode(encoded),
            Ok(InstanceCommand::ShowWindow)
        );
    }

    #[test]
    fn oversized_and_unknown_frames_are_rejected() {
        assert_eq!(
            InstanceCommand::decode(&[b'x'; MAX_INSTANCE_FRAME_LEN + 1]),
            Err(InstanceProtocolError::FrameTooLarge)
        );
        assert_eq!(
            InstanceCommand::decode(b"quit\n"),
            Err(InstanceProtocolError::UnknownCommand)
        );
    }

    #[test]
    fn second_claim_notifies_primary_and_endpoint_is_reusable() {
        let endpoint = InstanceEndpoint::unique_for_test();
        let primary = match claim_endpoint(&endpoint).expect("first claim must succeed") {
            InstanceLaunch::Primary(primary) => primary,
            InstanceLaunch::ExistingNotified => panic!("test endpoint was unexpectedly occupied"),
        };

        assert!(matches!(
            claim_endpoint(&endpoint).expect("second claim must notify"),
            InstanceLaunch::ExistingNotified
        ));
        assert_eq!(
            primary.receive_for_test(Duration::from_secs(2)),
            Some(InstanceCommand::ShowWindow)
        );

        drop(primary);
        assert!(matches!(
            claim_endpoint(&endpoint).expect("endpoint must be reusable after shutdown"),
            InstanceLaunch::Primary(_)
        ));
    }

    #[test]
    fn user_scope_hash_is_deterministic_and_distinct() {
        assert_eq!(stable_scope_hash(b"same"), stable_scope_hash(b"same"));
        assert_ne!(stable_scope_hash(b"one"), stable_scope_hash(b"two"));
    }
}
