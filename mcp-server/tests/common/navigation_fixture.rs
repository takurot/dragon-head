use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

pub struct PrivateDestination {
    pub url: String,
    pub count: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl PrivateDestination {
    pub fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let count = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::clone(&count);
        let should_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !should_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if should_stop.load(Ordering::Acquire) {
                            break;
                        }
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                        let mut request = [0; 4096];
                        if stream.read(&mut request).is_ok_and(|bytes| bytes > 0) {
                            let request = String::from_utf8_lossy(&request);
                            let target = request
                                .lines()
                                .next()
                                .and_then(|line| line.split_whitespace().nth(1))
                                .unwrap_or("/");
                            let (content_type, body) = if target == "/deferred-page" {
                                ("text/html", "<html><head><script defer src='/deferred.js'></script></head><body>initializing</body></html>")
                            } else if target == "/deferred.js" {
                                thread::sleep(Duration::from_secs(4));
                                ("application/javascript", "document.body.insertAdjacentHTML('beforeend', '<button>DEFER INITIALIZATION COMPLETE</button>');")
                            } else if target == "/slow-page" {
                                ("text/html", "<html><head><script src='/slow.js'></script></head><body>loaded</body></html>")
                            } else if target == "/slow.js" {
                                thread::sleep(Duration::from_secs(4));
                                ("application/javascript", "window.slowScriptLoaded = true;")
                            } else if target.starts_with("/source") {
                                (
                                    "text/html",
                                    "<html><body><a href='/private'>go private</a></body></html>",
                                )
                            } else {
                                if target.starts_with("/private") {
                                    requests.fetch_add(1, Ordering::AcqRel);
                                }
                                ("text/html", "<html><body><button>PRIVATE DESTINATION</button></body></html>")
                            };
                            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            url: format!("http://{address}/private"),
            count,
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for PrivateDestination {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let address = self
            .url
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap();
        let _ = TcpStream::connect(address);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
