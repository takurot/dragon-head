use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

const HTML: &str = "<html><body><button id='delete' onclick=\"document.body.dataset.deleted='yes'\">Delete</button><a id='next' href='#'>Next</a></body></html>";

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
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                        let mut request = [0; 4096];
                        if stream.read(&mut request).is_ok_and(|length| length > 0) {
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
