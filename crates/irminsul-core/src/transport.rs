use crate::{CaptureMode, CaptureState, Engine, Snapshot, process, session::random_id};
use anyhow::{Result, bail, ensure};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

// Full IPv4 datagram + Ethernet header + message type.
const MAX_FRAME: usize = 65_550;
const MAX_PENDING: usize = 1_048_576;

/// The parent owns decoding and snapshots. The elevated child sends packets only;
/// it exposes no file access, inventory export, shell, or optimizer commands.
pub struct CaptureController {
    engine: Arc<Mutex<Engine>>,
    stop: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<TcpStream>>>,
    worker: Option<JoinHandle<Result<()>>>,
}

impl CaptureController {
    pub fn new() -> Result<Self> {
        Ok(Self {
            engine: Arc::new(Mutex::new(Engine::new()?)),
            stop: Arc::new(AtomicBool::new(false)),
            socket: Arc::new(Mutex::new(None)),
            worker: None,
        })
    }

    pub fn state(&self) -> Result<CaptureState> {
        Ok(self
            .engine
            .lock()
            .map_err(|_| anyhow::anyhow!("Capture worker stopped unexpectedly."))?
            .state())
    }

    pub fn snapshot(&self, uid: &str, capture_id: &str) -> Result<Snapshot> {
        self.engine
            .lock()
            .map_err(|_| anyhow::anyhow!("Capture worker stopped unexpectedly."))?
            .snapshot(uid, capture_id)
    }

    pub fn start(&mut self) -> Result<()> {
        self.start_with_mode(CaptureMode::Auto)
    }

    pub fn start_with_mode(&mut self, mode: CaptureMode) -> Result<()> {
        ensure!(
            self.worker.as_ref().is_none_or(JoinHandle::is_finished),
            "Capture is already running or stopping."
        );
        let process = process::game_process()?;
        self.engine.lock().unwrap().observe_process(process)?;
        self.engine.lock().unwrap().start()?;
        self.stop.store(false, Ordering::Release);
        let (engine, stop, socket) = (self.engine.clone(), self.stop.clone(), self.socket.clone());
        self.worker = Some(thread::spawn(move || {
            let result = capture_parent(&engine, &stop, &socket, mode);
            socket.lock().unwrap().take();
            let mut engine = engine.lock().unwrap();
            engine.stop();
            if let Err(error) = &result {
                engine.fail(error.to_string());
            }
            result
        }));
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Ok(mut engine) = self.engine.lock() {
            if engine.state().capturing {
                engine.status("stopping", "Stopping capture…");
            }
        }
    }

    pub fn stop_and_wait(&mut self) -> Result<()> {
        self.stop();
        let deadline = Instant::now() + Duration::from_secs(8);
        while self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            ensure!(
                Instant::now() < deadline,
                "Capture is still stopping. Finish the Windows permission prompt and try again."
            );
            thread::sleep(Duration::from_millis(50));
        }
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("Capture worker stopped unexpectedly."))??;
        }
        Ok(())
    }
}

impl Drop for CaptureController {
    fn drop(&mut self) {
        self.stop();
    }
}

fn capture_parent(
    engine: &Mutex<Engine>,
    stop: &AtomicBool,
    shared_socket: &Mutex<Option<TcpStream>>,
    mode: CaptureMode,
) -> Result<()> {
    let mut stream = match connect_helper(engine, stop, mode) {
        Ok(stream) => stream,
        Err(_) if stop.load(Ordering::Acquire) => return Ok(()),
        Err(error) => return Err(error),
    };
    *shared_socket.lock().unwrap() = Some(stream.try_clone()?);
    stream.set_nonblocking(true)?;
    engine
        .lock()
        .unwrap()
        .status("initializing", "Connected. Starting account capture...");
    receive_frames(engine, stop, &mut stream)
}

fn receive_frames(engine: &Mutex<Engine>, stop: &AtomicBool, stream: &mut TcpStream) -> Result<()> {
    let mut pending = Vec::new();
    let mut process_check = Instant::now();
    loop {
        if stop.load(Ordering::Acquire) {
            return stop_helper(stream, &mut pending);
        }
        if process_check.elapsed() >= Duration::from_millis(500) {
            engine
                .lock()
                .unwrap()
                .observe_process(process::game_process()?)?;
            process_check = Instant::now();
        }
        read_available(stream, &mut pending)?;
        if drain_frames(engine, &mut pending)? {
            return Ok(());
        }
    }
}

fn connect_helper(
    engine: &Mutex<Engine>,
    stop: &AtomicBool,
    mode: CaptureMode,
) -> Result<TcpStream> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let token = random_id()?;
    let port = listener.local_addr()?.port();
    let child_token = token.clone();
    let (launch_tx, launch_rx) = std::sync::mpsc::sync_channel(1);
    // The Windows consent UI can outlive cancellation. Keep its blocking shell
    // call separate so the listener can close; a late helper then cannot capture.
    thread::spawn(move || {
        let _ = launch_tx.send(process::launch_helper(port, &child_token, mode));
    });
    listener.set_nonblocking(true)?;
    engine.lock().unwrap().status(
        "starting",
        "Allow the Windows capture permission prompt, or stop to cancel.",
    );
    accept_helper(&listener, &token, stop, &launch_rx)
}

fn read_available(stream: &mut TcpStream, pending: &mut Vec<u8>) -> Result<()> {
    let mut buffer = [0u8; 32_768];
    match stream.read(&mut buffer) {
        Ok(0) => bail!("Capture helper closed. Completed snapshots remain available."),
        Ok(size) => pending.extend_from_slice(&buffer[..size]),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
            thread::sleep(Duration::from_millis(20))
        }
        Err(e) => return Err(e.into()),
    }
    ensure!(
        pending.len() <= MAX_PENDING,
        "Capture queue exceeded its limit. Restart capture."
    );
    Ok(())
}

fn drain_frames(engine: &Mutex<Engine>, pending: &mut Vec<u8>) -> Result<bool> {
    while let Some(frame) = take_frame(pending)? {
        if frame[0] == b'D' {
            return Ok(true);
        }
        handle_frame(engine, frame)?;
    }
    Ok(false)
}

fn stop_helper(stream: &mut TcpStream, pending: &mut Vec<u8>) -> Result<()> {
    if shutdown_confirmed(pending)? {
        return Ok(());
    }
    stream.set_nonblocking(false)?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    // A completed helper may already have sent D. Read its acknowledgement even
    // when the stop write races its graceful socket close.
    let _ = stream.write_all(b"S");
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut buffer = [0u8; 32_768];
    loop {
        ensure!(
            Instant::now() < deadline,
            "Capture helper did not confirm shutdown."
        );
        match stream.read(&mut buffer) {
            Ok(0) => bail!("Capture helper closed without confirming shutdown."),
            Ok(size) => {
                pending.extend_from_slice(&buffer[..size]);
                if shutdown_confirmed(pending)? {
                    return Ok(());
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                ()
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn shutdown_confirmed(pending: &mut Vec<u8>) -> Result<bool> {
    while let Some(frame) = take_frame(pending)? {
        match frame[0] {
            b'D' => return Ok(true),
            b'E' => bail!("{}", String::from_utf8_lossy(&frame[1..])),
            _ => (),
        }
    }
    Ok(false)
}

fn accept_helper(
    listener: &TcpListener,
    token: &str,
    stop: &AtomicBool,
    launch: &std::sync::mpsc::Receiver<Result<()>>,
) -> Result<TcpStream> {
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline && !stop.load(Ordering::Acquire) {
        if let Ok(Err(error)) = launch.try_recv() {
            return Err(error);
        }
        match listener.accept() {
            Ok((mut stream, address)) if address.ip().is_loopback() => {
                stream.set_read_timeout(Some(Duration::from_secs(1)))?;
                let mut received = [0u8; 64];
                if stream.read_exact(&mut received).is_ok() && token.as_bytes() == received {
                    return Ok(stream);
                }
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50))
            }
            Err(e) => return Err(e.into()),
        }
    }
    bail!(
        "Capture permission was cancelled or timed out. Dismiss any old Windows prompt before starting again."
    )
}

fn take_frame(pending: &mut Vec<u8>) -> Result<Option<Vec<u8>>> {
    if pending.len() < 4 {
        return Ok(None);
    }
    let len = u32::from_be_bytes(pending[..4].try_into()?) as usize;
    ensure!(
        (1..=MAX_FRAME).contains(&len),
        "Invalid capture frame length."
    );
    if pending.len() < len + 4 {
        return Ok(None);
    }
    let frame = pending[4..4 + len].to_vec();
    pending.drain(..4 + len);
    Ok(Some(frame))
}

fn handle_frame(engine: &Mutex<Engine>, frame: Vec<u8>) -> Result<()> {
    match frame[0] {
        b'R' => engine.lock().unwrap().status(
            "waiting",
            if &frame[1..] == b"compatibility" {
                "Compatibility capture is running. Log into an account and enter the door."
            } else {
                "Capture is running. Log into an account and enter the door."
            },
        ),
        b'E' => bail!("{}", String::from_utf8_lossy(&frame[1..])),
        b'P' => {
            let packet = frame[1..].to_vec();
            let mut engine = engine.lock().unwrap();
            // A new login cannot race the slower idle process watcher.
            if crate::session::is_login_packet(&packet) {
                engine.observe_process(process::game_process()?)?;
            }
            if let Err(error) = engine.receive(packet) {
                engine.fail(error.to_string());
            }
        }
        _ => bail!("Unknown capture message."),
    }
    Ok(())
}

/// Call before argument parsing or GUI startup in either native executable.
pub fn run_helper_if_requested() -> Option<Result<()>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("--irminsul-capture-helper") {
        return None;
    }
    Some((|| {
        ensure!((3..=4).contains(&args.len()), "Invalid helper arguments.");
        let port = args[1].parse::<u16>()?;
        let mode = args
            .get(3)
            .map(|value| value.parse())
            .transpose()?
            .unwrap_or_default();
        ensure!(
            args[2].len() == 64 && args[2].bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid helper identity."
        );
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(helper(port, &args[2], mode))
    })())
}

#[cfg(windows)]
async fn helper(port: u16, token: &str, mode: CaptureMode) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut socket = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    socket.write_all(token.as_bytes()).await?;
    let result = capture_packets(&mut socket, mode).await;
    if let Err(error) = &result {
        let _ = send_frame(&mut socket, b'E', error.to_string().as_bytes()).await;
    } else {
        // Sent only after capture_packets has released its stream and session.
        send_frame(&mut socket, b'D', &[]).await?;
    }
    result
}

#[cfg(not(windows))]
async fn helper(_: u16, _: &str, _: CaptureMode) -> Result<()> {
    bail!("Windows capture helper unavailable.")
}

#[cfg(windows)]
async fn send_frame(socket: &mut tokio::net::TcpStream, kind: u8, data: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    ensure!(data.len() < MAX_FRAME, "Oversized packet.");
    let mut frame = Vec::with_capacity(data.len() + 5);
    frame.extend_from_slice(&((data.len() + 1) as u32).to_be_bytes());
    frame.push(kind);
    frame.extend_from_slice(data);
    tokio::time::timeout(Duration::from_secs(5), socket.write_all(&frame)).await??;
    Ok(())
}

#[cfg(windows)]
async fn capture_packets(socket: &mut tokio::net::TcpStream, mode: CaptureMode) -> Result<()> {
    use tokio::io::AsyncReadExt;
    let mut packets = crate::native_capture::PacketSource::open(mode)?;
    send_frame(socket, b'R', packets.ready_label()).await?;
    // Allow one capture session to cover dailies across several accounts.
    let deadline = tokio::time::sleep(Duration::from_secs(4 * 60 * 60));
    tokio::pin!(deadline);
    let mut control = [0u8; 1];
    loop {
        let packet = tokio::select! {
            _ = socket.read(&mut control) => break,
            _ = &mut deadline => break,
            packet = packets.next_packet() => packet?,
        };
        send_frame(socket, b'P', &packet).await?;
    }
    drop(packets);
    // Dropping the source releases its private session or receive sockets.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compatibility_ready_message_is_visible() -> Result<()> {
        let engine = Mutex::new(Engine::new()?);
        handle_frame(&engine, b"Rcompatibility".to_vec())?;
        let state = engine.lock().unwrap().state();
        assert_eq!(state.phase, "waiting");
        assert!(state.message.contains("Compatibility capture"));
        Ok(())
    }
    #[test]
    fn stopping_preserves_a_partially_received_frame() -> Result<()> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let mut client = TcpStream::connect(listener.local_addr()?)?;
        let (mut helper, _) = listener.accept()?;
        let helper = thread::spawn(move || -> Result<()> {
            let mut control = [0];
            helper.read_exact(&mut control)?;
            ensure!(control == [b'S']);
            // Tail of an in-flight packet, followed by the shutdown acknowledgement.
            helper.write_all(&[2, 3, 0, 0, 0, 1, b'D'])?;
            Ok(())
        });
        let mut pending = vec![0, 0, 0, 4, b'P', 1];
        stop_helper(&mut client, &mut pending)?;
        helper.join().unwrap()?;
        Ok(())
    }
    #[test]
    fn shutdown_requires_ack_after_pending_packets() -> Result<()> {
        let mut pending = vec![0, 0, 0, 2, b'P', 1, 0, 0, 0, 1];
        assert!(!shutdown_confirmed(&mut pending)?);
        pending.push(b'D');
        assert!(shutdown_confirmed(&mut pending)?);
        assert!(shutdown_confirmed(&mut vec![0, 0, 0, 2, b'E', b'x']).is_err());
        let engine = Mutex::new(Engine::new()?);
        assert!(drain_frames(&engine, &mut vec![0, 0, 0, 1, b'D'])?);
        assert_ne!(engine.lock().unwrap().state().phase, "error");
        Ok(())
    }
    #[test]
    fn fragmented_frames_preserve_bytes_and_reject_unbounded_input() -> Result<()> {
        let mut pending = vec![0, 0, 0, 3, b'P'];
        assert!(take_frame(&mut pending)?.is_none());
        pending.extend_from_slice(&[1, 2]);
        assert_eq!(take_frame(&mut pending)?, Some(vec![b'P', 1, 2]));
        assert!(pending.is_empty());
        assert!(take_frame(&mut vec![255; 4]).is_err());
        Ok(())
    }
}
