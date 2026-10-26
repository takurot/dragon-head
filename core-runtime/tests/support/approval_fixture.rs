use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const HTML: &str = "<html><body><button id='delete' onclick=\"document.body.dataset.deleted='yes'\">Delete</button><a id='next' href='#'>Next</a></body></html>";

fn read_request_headers(
    mut read: impl FnMut(&mut [u8], Duration) -> io::Result<usize>,
) -> io::Result<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut request = [0; 4096];
    let mut length = 0;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let count = read(&mut request[length..], remaining)?;
        if count == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        length += count;
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        if let Some(end) = request[..length]
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
        {
            return Ok(request[..end + 4].to_vec());
        }
        if length == request.len() {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
}

pub struct Fixture {
    pub url: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    pub fn new(script_redirect: bool) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}/start", listener.local_addr()?);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Accepted sockets can inherit the listener's nonblocking mode.
                        if stream.set_nonblocking(false).is_err() {
                            continue;
                        }
                        if let Ok(request) = read_request_headers(|buffer, remaining| {
                            stream.set_read_timeout(Some(remaining))?;
                            stream.read(buffer)
                        }) {
                            let request = String::from_utf8_lossy(&request);
                            let body = if script_redirect && request.starts_with("GET /start ") {
                                "<html><script>location.href='/approved'</script></html>"
                            } else {
                                HTML
                            };
                            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            url,
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(
            self.url
                .trim_start_matches("http://")
                .trim_end_matches("/start"),
        );
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn fragmented_request_waits_for_the_complete_headers() {
        let mut reader =
            Cursor::new(b"GET /sta").chain(Cursor::new(b"rt HTTP/1.1\r\nHost: localhost\r\n\r\n"));
        let request = read_request_headers(|buffer, _| reader.read(buffer)).unwrap();
        assert_eq!(request, b"GET /start HTTP/1.1\r\nHost: localhost\r\n\r\n");
    }

    #[test]
    fn incomplete_headers_are_rejected_at_eof() {
        let mut reader = Cursor::new(b"GET /start HTTP/1.1\r\nHost: localhost\r\n");
        let error = read_request_headers(|buffer, _| reader.read(buffer))
            .expect_err("incomplete headers must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn oversized_headers_are_rejected() {
        let mut reader = Cursor::new(vec![b'x'; 4097]);
        let error = read_request_headers(|buffer, _| reader.read(buffer))
            .expect_err("oversized headers must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
