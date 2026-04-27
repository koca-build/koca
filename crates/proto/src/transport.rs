use std::path::PathBuf;

use interprocess::local_socket::{
    prelude::*, tokio::prelude::*, GenericFilePath, GenericNamespaced, ListenerOptions,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::mpsc,
};

use crate::{
    error::ProtoError,
    types::{Command, Message, MessageBody, Request, ResultPayload},
};

/// Generate a unique socket name for this process.
pub fn socket_name() -> String {
    format!("koca-backend-{}", std::process::id())
}

fn make_name(name: &str) -> Result<interprocess::local_socket::Name<'static>, ProtoError> {
    if GenericNamespaced::is_supported() {
        name.to_owned()
            .to_ns_name::<GenericNamespaced>()
            .map_err(ProtoError::Socket)
    } else {
        PathBuf::from(format!("/tmp/{name}"))
            .to_fs_name::<GenericFilePath>()
            .map_err(ProtoError::Socket)
    }
}

type TokioStream = interprocess::local_socket::tokio::Stream;

// ── KocaSession (parent / koca side) ─────────────────────────────────────

/// koca's session handle to a running backend process.
///
/// A background tokio task continuously reads from the socket and buffers
/// messages into an internal channel. This means:
/// - `try_recv()` is always non-blocking — returns `None` if no message ready.
/// - The TUI render loop ticks at a fixed interval, drains all buffered events,
///   then renders once. No `tokio::select!` needed.
///
/// ```rust,ignore
/// session.send(Command::Confirm).await?;
/// let mut ticker = tokio::time::interval(Duration::from_millis(80));
/// let result = 'outer: loop {
///     ticker.tick().await;
///     state.tick += 1;
///     loop {
///         match session.try_recv()? {
///             None => break,
///             Some(MessageBody::Event { event })   => state.apply(event),
///             Some(MessageBody::Result { result })  => break 'outer result,
///             Some(MessageBody::Error { error })    => return Err(error.into()),
///         }
///     }
///     vp.draw(height, |f| render(f, &state));
/// };
/// ```
pub struct KocaSession {
    /// Write half of the socket — for sending requests to the backend.
    writer: tokio::io::WriteHalf<TokioStream>,
    /// Buffered messages from the background reader task.
    receiver: mpsc::Receiver<Result<MessageBody, ProtoError>>,
    next_id: u64,
}

impl KocaSession {
    fn new(stream: TokioStream) -> Self {
        let (read, write) = tokio::io::split(stream);
        let (tx, rx) = mpsc::channel(64);

        // Background task: drain socket → channel.
        // Exits when the channel is dropped (KocaSession dropped) or on error.
        tokio::spawn(async move {
            let mut reader = BufReader::new(read);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Err(e) => {
                        let _ = tx.send(Err(ProtoError::Io(e))).await;
                        break;
                    }
                    Ok(0) => {
                        // EOF — connection closed by backend
                        let _ = tx.send(Err(ProtoError::ConnectionClosed)).await;
                        break;
                    }
                    Ok(_) => {
                        let result = serde_json::from_str::<Message>(&line)
                            .map(|m| m.body)
                            .map_err(ProtoError::Json);
                        if tx.send(result).await.is_err() {
                            // KocaSession was dropped; stop reading
                            break;
                        }
                    }
                }
            }
        });

        Self {
            writer: write,
            receiver: rx,
            next_id: 1,
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    async fn write_request(&mut self, req: &Request) -> Result<(), ProtoError> {
        let mut line = serde_json::to_string(req).map_err(ProtoError::Json)?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .await
            .map_err(ProtoError::Io)?;
        self.writer.flush().await.map_err(ProtoError::Io)?;
        Ok(())
    }

    // ── Public API ────────────────────────────────────────────────────────

    /// Non-blocking poll.
    ///
    /// - `Ok(None)` — no message buffered yet; keep going.
    /// - `Ok(Some(body))` — a message is ready.
    /// - `Err(e)` — the connection died.
    pub fn try_recv(&mut self) -> Result<Option<MessageBody>, ProtoError> {
        match self.receiver.try_recv() {
            Ok(Ok(body)) => Ok(Some(body)),
            Ok(Err(e)) => Err(e),
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => Err(ProtoError::ConnectionClosed),
        }
    }

    /// Blocking async recv. Awaits until the next message arrives.
    pub async fn recv(&mut self) -> Result<MessageBody, ProtoError> {
        match self.receiver.recv().await {
            Some(Ok(body)) => Ok(body),
            Some(Err(e)) => Err(e),
            None => Err(ProtoError::ConnectionClosed),
        }
    }

    /// Simple request/response for commands that don't stream events
    /// (e.g. `check-installed`, `abort`). Awaits until the result arrives.
    pub async fn call(&mut self, cmd: Command) -> Result<ResultPayload, ProtoError> {
        let id = self.next_id();
        self.write_request(&Request { id, cmd }).await?;
        loop {
            match self.recv().await? {
                MessageBody::Result { result } => return Ok(result),
                MessageBody::Error { error } => return Err(ProtoError::Backend(error)),
                MessageBody::Event { .. } => {} // unexpected; ignore
            }
        }
    }

    /// Send a command without waiting for a response.
    ///
    /// Use for streaming commands (`confirm`, `remove`) where the TUI loop
    /// drives consumption via `try_recv()`.
    pub async fn send(&mut self, cmd: Command) -> Result<u64, ProtoError> {
        let id = self.next_id();
        self.write_request(&Request { id, cmd }).await?;
        Ok(id)
    }

    /// Send a `Shutdown` command. The caller is responsible for waiting on the
    /// child process afterwards (`child.wait().await`).
    pub async fn shutdown(mut self) -> Result<(), ProtoError> {
        let id = self.next_id();
        // Best-effort during shutdown — ignore send errors
        let _ = self
            .write_request(&Request {
                id,
                cmd: Command::Shutdown,
            })
            .await;
        Ok(())
    }
}

/// Listens for the backend to connect. Create this **before** spawning the
/// backend process, then pass the socket name to the backend via `--socket`.
pub struct KocaListener {
    listener: interprocess::local_socket::tokio::Listener,
}

impl KocaListener {
    pub fn listen(name: &str) -> Result<Self, ProtoError> {
        let socket_name = make_name(name)?;
        let listener = ListenerOptions::new()
            .name(socket_name)
            .create_tokio()
            .map_err(ProtoError::Io)?;
        Ok(Self { listener })
    }

    pub async fn accept(self) -> Result<KocaSession, ProtoError> {
        let stream = self.listener.accept().await.map_err(ProtoError::Io)?;
        Ok(KocaSession::new(stream))
    }
}

// ── BackendSession (child / backend side) ─────────────────────────────────

/// The backend's session handle back to koca.
///
/// The backend processes one request at a time and may freely block on `recv()`.
pub struct BackendSession {
    reader: BufReader<tokio::io::ReadHalf<TokioStream>>,
    writer: tokio::io::WriteHalf<TokioStream>,
}

impl BackendSession {
    /// Connect to the socket that koca is listening on.
    /// Pass the value of the `--socket` argument here.
    pub async fn connect(name: &str) -> Result<Self, ProtoError> {
        let socket_name = make_name(name)?;
        let stream = TokioStream::connect(socket_name)
            .await
            .map_err(ProtoError::Io)?;
        let (read, write) = tokio::io::split(stream);
        Ok(Self {
            reader: BufReader::new(read),
            writer: write,
        })
    }

    /// Wait for the next request from koca. The backend can block here freely.
    pub async fn recv(&mut self) -> Result<Request, ProtoError> {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .await
            .map_err(ProtoError::Io)?;
        if line.is_empty() {
            return Err(ProtoError::ConnectionClosed);
        }
        serde_json::from_str(&line).map_err(ProtoError::Json)
    }

    /// Send any message (result, event, or error) to koca.
    pub async fn send(&mut self, msg: &Message) -> Result<(), ProtoError> {
        let mut line = serde_json::to_string(msg).map_err(ProtoError::Json)?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .await
            .map_err(ProtoError::Io)?;
        self.writer.flush().await.map_err(ProtoError::Io)?;
        Ok(())
    }
}
