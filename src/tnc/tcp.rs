//! Raw TCP byte pipe for soft TNCs (Soundmodem / Direwolf KISS TCP).
//!
//! Same TAPR KISS framing as serial; the socket already speaks KISS — no
//! KISS.INI / DTR / exit sequence.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use socket2::{SockRef, TcpKeepalive};
use tokio::sync::mpsc;
use tracing::info;

use super::{TncError, TncEvent};

const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// Nonblocking TCP reads distinguish quiet links from fatal keepalive timeouts.
pub struct TcpKissLink {
    stream: TcpStream,
    pub addr: String,
    activity: Option<mpsc::Sender<TncEvent>>,
    last_tx_note: Option<Instant>,
    last_rx_note: Option<Instant>,
}

impl TcpKissLink {
    pub fn connect(host: &str, port: u16) -> Result<Self, TncError> {
        let host = host.trim();
        let addr = format!("{host}:{port}");
        info!(%addr, "opening KISS TCP");
        let stream = TcpStream::connect(&addr)
            .map_err(|e| TncError::Protocol(format!("KISS TCP connect {addr}: {e}")))?;
        let keepalive = TcpKeepalive::new()
            .with_time(Duration::from_secs(120))
            .with_interval(Duration::from_secs(30))
            .with_retries(5);
        SockRef::from(&stream)
            .set_tcp_keepalive(&keepalive)
            .map_err(|e| TncError::Protocol(format!("KISS TCP keepalive setup {addr}: {e}")))?;
        stream.set_nodelay(true)?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            addr,
            activity: None,
            last_tx_note: None,
            last_rx_note: None,
        })
    }

    pub fn watch(&mut self, events: mpsc::Sender<TncEvent>) {
        self.activity = Some(events);
    }

    fn note_link(&mut self, tx: bool) {
        let now = Instant::now();
        let recent = if tx {
            self.last_tx_note
        } else {
            self.last_rx_note
        };
        if recent.is_some_and(|t| now.saturating_duration_since(t) < Duration::from_millis(40)) {
            return;
        }
        if tx {
            self.last_tx_note = Some(now);
        } else {
            self.last_rx_note = Some(now);
        }
        if let Some(events) = &self.activity {
            let _ = events.try_send(TncEvent::LinkIo { tx, index: 0 });
        }
    }

    pub fn write_all(&mut self, buf: &[u8]) -> Result<(), TncError> {
        let deadline = Instant::now() + WRITE_TIMEOUT;
        let mut remaining = buf;
        while !remaining.is_empty() {
            match self.stream.write(remaining) {
                Ok(0) => {
                    return Err(TncError::Io(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "KISS TCP write returned zero",
                    )));
                }
                Ok(written) => remaining = &remaining[written..],
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(TncError::Io(e)),
            }
            if !remaining.is_empty() {
                if Instant::now() >= deadline {
                    return Err(TncError::Io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "KISS TCP write stalled; acceptance unknown, block not replayed",
                    )));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        self.stream.flush()?;
        if !buf.is_empty() {
            self.note_link(true);
        }
        Ok(())
    }

    pub fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, TncError> {
        match self.stream.read(buf) {
            Ok(0) => Err(TncError::Protocol(format!(
                "KISS TCP peer closed ({})",
                self.addr
            ))),
            Ok(n) => {
                self.note_link(false);
                Ok(n)
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                Ok(0)
            }
            Err(e) => Err(TncError::Io(e)),
        }
    }

    pub fn flush_input(&mut self) {
        let mut buf = [0u8; 256];
        for _ in 0..32 {
            match self.try_read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => continue,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn pair() -> (TcpKissLink, TcpStream) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let link =
            TcpKissLink::connect("127.0.0.1", listener.local_addr().unwrap().port()).unwrap();
        let (peer, _) = listener.accept().unwrap();
        (link, peer)
    }

    #[test]
    fn kiss_tcp_idle_keepalive_does_not_send_application_bytes() {
        let (mut link, mut peer) = pair();
        let socket = SockRef::from(&link.stream);
        assert!(socket.keepalive().unwrap());
        assert_eq!(socket.tcp_keepalive_retries().unwrap(), 5);
        #[cfg(not(windows))]
        {
            assert_eq!(
                socket.tcp_keepalive_time().unwrap(),
                Duration::from_secs(120)
            );
            assert_eq!(
                socket.tcp_keepalive_interval().unwrap(),
                Duration::from_secs(30)
            );
        }
        socket
            .set_tcp_keepalive(
                &TcpKeepalive::new()
                    .with_time(Duration::from_secs(1))
                    .with_interval(Duration::from_secs(1))
                    .with_retries(2),
            )
            .unwrap();
        peer.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut byte = [0];
        while Instant::now() < deadline {
            assert_eq!(link.try_read(&mut byte).unwrap(), 0);
            assert_eq!(
                peer.read(&mut byte).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        peer.write_all(b"\xc0\x00ok\xc0").unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut received = Vec::new();
        while received.len() < 5 && Instant::now() < deadline {
            let length = link.try_read(&mut byte).unwrap();
            received.extend_from_slice(&byte[..length]);
        }
        assert_eq!(received, b"\xc0\x00ok\xc0");
    }

    #[test]
    fn kiss_tcp_peer_reset_is_not_an_idle_read() {
        let (mut link, peer) = pair();
        SockRef::from(&peer)
            .set_linger(Some(Duration::ZERO))
            .unwrap();
        drop(peer);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match link.try_read(&mut [0; 16]) {
                Err(_) => break,
                Ok(0) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                result => panic!("Reset peer was not reported as lost: {result:?}"),
            }
        }
    }

    #[test]
    fn kiss_tcp_stalled_write_is_bounded_and_not_reported_as_success() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        SockRef::from(&listener).set_recv_buffer_size(4096).unwrap();
        let mut link =
            TcpKissLink::connect("127.0.0.1", listener.local_addr().unwrap().port()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        SockRef::from(&link.stream)
            .set_send_buffer_size(4096)
            .unwrap();
        // Fill with small writes: Windows may accept one large write despite SO_SNDBUF.
        let fill_deadline = Instant::now() + Duration::from_secs(5);
        let mut last_progress = Instant::now();
        loop {
            assert!(Instant::now() < fill_deadline, "TCP queue did not stall");
            match link.stream.write(&[0x55; 4096]) {
                Ok(written) if written > 0 => last_progress = Instant::now(),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if last_progress.elapsed() >= Duration::from_millis(100) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                result => panic!("Unexpected TCP queue fill result: {result:?}"),
            }
        }
        let (events, mut rx) = mpsc::channel(8);
        link.watch(events);
        let start = Instant::now();
        let error = link.write_all(&[0x55; 4096]).unwrap_err();
        assert!(matches!(error, TncError::Io(ref e) if e.kind() == std::io::ErrorKind::TimedOut));
        assert!(start.elapsed() >= WRITE_TIMEOUT);
        assert!(start.elapsed() < WRITE_TIMEOUT + Duration::from_secs(2));
        assert!(rx.try_recv().is_err());
    }
}
