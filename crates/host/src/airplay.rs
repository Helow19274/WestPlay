//! Video-only MPEG-TS relay. UxPlay owns AirPlay and audio; the cabin plays video over loopback HTTP.
//! All process/socket work runs off the STA/render thread. Only our child process is ever stopped.
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const IO_WAIT: Duration = Duration::from_millis(250);

#[derive(Debug, PartialEq, serde::Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum Command {
    Start,
    Stop,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub running: bool,
    pub url: Option<String>,
    pub error: Option<String>,
}

impl State {
    pub fn message(&self) -> String {
        serde_json::json!({"type": "airplayState", "running": self.running, "url": self.url, "error": self.error})
            .to_string()
    }
}

pub struct AirPlay {
    commands: Option<mpsc::Sender<Command>>,
    updates: mpsc::Receiver<State>,
    worker: Option<JoinHandle<()>>,
}

impl AirPlay {
    pub fn new(data: PathBuf) -> io::Result<Self> {
        let (commands, requests) = mpsc::channel();
        let (reports, updates) = mpsc::channel();
        let worker = thread::Builder::new().name("westplay-airplay".into()).spawn(move || {
            let mut session: Option<Session> = None;
            loop {
                match requests.recv_timeout(IO_WAIT) {
                    Ok(Command::Start) if session.is_none() => {
                        match Session::start(&data) {
                            Ok(started) => {
                                let _ = reports.send(State { running: true, url: Some(started.url.clone()), error: None });
                                session = Some(started);
                            }
                            Err(error) => {
                                log!("[airplay] start failed: {error}");
                                let _ = reports.send(State { error: Some(error.to_string()), ..State::default() });
                            }
                        }
                    }
                    Ok(Command::Start) => {}
                    Ok(Command::Stop) => {
                        drop(session.take());
                        let _ = reports.send(State::default());
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if let Some(active) = &mut session {
                    let problem = match active.child.try_wait() {
                        Ok(Some(code)) => Some(format!("UxPlay exited ({code}). Close any other UxPlay instance and check the AirPlay log and GStreamer plugins.")),
                        Err(error) => Some(format!("Cannot monitor UxPlay: {error}")),
                        Ok(None) if active.relay.as_ref().is_some_and(|thread| thread.is_finished()) => Some("AirPlay video relay stopped. Restart AirPlay.".into()),
                        Ok(None) => None,
                    };
                    if let Some(error) = problem {
                        log!("[airplay] {error}");
                        drop(session.take());
                        let _ = reports.send(State { error: Some(error), ..State::default() });
                    }
                }
            }
        })?;
        Ok(Self { commands: Some(commands), updates, worker: Some(worker) })
    }

    pub fn command(&self, command: Command) {
        if let Some(sender) = &self.commands {
            let _ = sender.send(command);
        }
    }

    pub fn updates(&self) -> impl Iterator<Item = State> + '_ {
        self.updates.try_iter()
    }
}

impl Drop for AirPlay {
    fn drop(&mut self) {
        self.commands.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn engine(folder: &Path) -> io::Result<(PathBuf, bool)> {
    // The GUI embeds libuxplay statically and does not forward command-line engine arguments.
    for (name, gui) in [("uxplay.exe", false), ("uxplay-windows.exe", true)] {
        let path = folder.join(name);
        if path.is_file() {
            return Ok((path, gui));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("Place the complete uxplay-windows folder beside WestPlay.exe (expected {}).", folder.display()),
    ))
}

fn arguments(port: u16) -> Vec<String> {
    // RTP pay/depay is in-process: no extra receiver, decoding or video encoding. Keep normal audio on.
    let pipeline = format!(
        "config-interval=1 ! rtph264depay ! h264parse ! mpegtsmux alignment=7 ! tcpserversink host=127.0.0.1 port={port}"
    );
    ["-n", "WestPlay", "-nh", "-s", "1280x720", "-fps", "30", "-nofreeze", "-vrtp", &pipeline]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn gui_arguments(args: &[String]) -> String {
    // These are fixed engine options and a numeric port, never user input or paths.
    args.iter()
        .map(|arg| if arg.contains(' ') { format!("\"{arg}\"") } else { arg.clone() })
        .collect::<Vec<_>>()
        .join(" ")
}

struct Session {
    child: Child,
    job: Job,
    stop: Arc<AtomicBool>,
    relay: Option<JoinHandle<()>>,
    url: String,
}

impl Session {
    fn start(data: &Path) -> io::Result<Self> {
        let exe = std::env::current_exe()?;
        let folder = exe.parent().ok_or_else(|| io::Error::other("No executable directory"))?.join("uxplay-windows");
        let (binary, gui) = engine(&folder)?;
        let http = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        http.set_nonblocking(true)?;
        // UxPlay must bind its own socket. Reserve a free port until immediately before launch.
        let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let video = reservation.local_addr()?;
        // A fresh unguessable URL prevents unrelated local pages from reading the phone's screen.
        // SAFETY: CoCreateGuid writes/returns a GUID and does not require COM initialization.
        let token = unsafe { windows::Win32::System::Com::CoCreateGuid() }.map_err(io::Error::other)?;
        let route = format!("/{token:?}/airplay.ts");
        let url = format!("http://127.0.0.1:{}{route}", http.local_addr()?.port());
        let runtime = data.join("airplay");
        std::fs::create_dir_all(runtime.join("uxplay-windows"))?;
        let args = arguments(video.port());
        // GUI's machine configuration wins over its Qt user settings. Override ProgramData ONLY in
        // this child, so no installed UxPlay settings or real machine-wide files are overwritten.
        std::fs::write(runtime.join("uxplay-windows/arguments.txt"), gui_arguments(&args))?;
        let rc = runtime.join("uxplayrc");
        std::fs::write(&rc, "# WestPlay owns the engine options for this session.\n")?;
        let log_path = runtime.join("receiver.log");
        let log = std::fs::File::create(&log_path)?;
        let mut command = Process::new(binary);
        command
            .current_dir(&folder)
            .env("UXPLAYRC", &rc)
            .creation_flags(0x08000000) // CREATE_NO_WINDOW: no console window.
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        if gui {
            command.env("ProgramData", &runtime);
        } else {
            command.args(&args);
        }
        let plugins = folder.join("lib/gstreamer-1.0");
        if plugins.is_dir() {
            command.env("GST_PLUGIN_PATH", &plugins).env("GST_PLUGIN_PATH_1_0", &plugins);
        }
        let scanner = folder.join("libexec/gstreamer-1.0/gst-plugin-scanner.exe");
        if scanner.is_file() {
            command.env("GST_PLUGIN_SCANNER", scanner);
        }
        drop(reservation);
        let job = Job::new()?;
        let mut child = command.spawn()?;
        if let Err(error) = job.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        log!("[airplay] receiver started, pid {}; diagnostics: {}", child.id(), log_path.display());
        let stop = Arc::new(AtomicBool::new(false));
        let mut session = Self { child, job, stop: stop.clone(), relay: None, url };
        session.relay = Some(thread::Builder::new().name("westplay-airplay-http".into()).spawn(move || {
            serve(http, video, &route, &stop);
        })?);
        Ok(session)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.job.terminate();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(relay) = self.relay.take() {
            let _ = relay.join();
        }
        log!("[airplay] receiver stopped");
    }
}

/// Closing the job also stops the receiver if WestPlay crashes; descendants cannot outlive the session.
struct Job(windows::Win32::Foundation::HANDLE);

impl Job {
    fn new() -> io::Result<Self> {
        use windows::Win32::System::JobObjects::*;
        // SAFETY: unnamed, non-inheritable job owned by this value; the limits buffer is valid for the call.
        unsafe {
            let job = Self(CreateJobObjectW(None, None).map_err(io::Error::other)?);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            )
            .map_err(io::Error::other)?;
            Ok(job)
        }
    }
    fn assign(&self, child: &Child) -> io::Result<()> {
        // SAFETY: the job and child process handles remain alive throughout the call.
        unsafe {
            windows::Win32::System::JobObjects::AssignProcessToJobObject(
                self.0,
                windows::Win32::Foundation::HANDLE(child.as_raw_handle()),
            )
        }
        .map_err(io::Error::other)
    }
    fn terminate(&self) {
        // SAFETY: the job belongs exclusively to this session; unrelated UxPlay instances aren't members.
        unsafe {
            let _ = windows::Win32::System::JobObjects::TerminateJobObject(self.0, 0);
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: this is the one owning handle, closed once; KILL_ON_JOB_CLOSE handles parent crashes too.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

fn allowed_origin(origin: &str) -> bool {
    if origin == "https://westplay.cabin" {
        return true;
    }
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && matches!(url.host_str(), Some("localhost" | "127.0.0.1"))
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
}

fn request(stream: &mut TcpStream, route: &str) -> io::Result<Option<bool>> {
    // One small request per connection. Timeouts bound shutdown even for a partial/malicious request.
    let mut header = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while header.len() < 8192 && !header.ends_with(b"\r\n\r\n") {
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::other("HTTP header timeout"));
        }
        let mut byte = [0];
        stream.read_exact(&mut byte)?;
        header.push(byte[0]);
    }
    if !header.ends_with(b"\r\n\r\n") {
        return Err(io::Error::other("HTTP headers too large"));
    }
    let header = String::from_utf8_lossy(&header);
    let mut lines = header.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let method = first.next().unwrap_or_default();
    let path = first.next().unwrap_or_default();
    let origin = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("origin"))
        .map(|(_, value)| value.trim());
    if path != route || !matches!(method, "GET" | "OPTIONS") || origin.is_some_and(|o| !allowed_origin(o)) {
        stream.write_all(b"HTTP/1.0 403 Forbidden\r\nContent-Length: 0\r\n\r\n")?;
        return Ok(None);
    }
    let cors = origin.map(|o| format!("Access-Control-Allow-Origin: {o}\r\nVary: Origin\r\n")).unwrap_or_default();
    if method == "OPTIONS" {
        write!(
            stream,
            "HTTP/1.0 204 No Content\r\n{cors}Access-Control-Allow-Methods: GET, OPTIONS\r\nAccess-Control-Allow-Headers: Range\r\nAccess-Control-Allow-Private-Network: true\r\nContent-Length: 0\r\n\r\n"
        )?;
        return Ok(Some(false));
    }
    write!(
        stream,
        "HTTP/1.0 200 OK\r\n{cors}Content-Type: video/mp2t\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
    )?;
    Ok(Some(true))
}

fn serve(listener: TcpListener, video: SocketAddr, route: &str, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut browser, _)) => {
                let _ = browser.set_read_timeout(Some(IO_WAIT));
                let _ = browser.set_write_timeout(Some(IO_WAIT));
                if request(&mut browser, route).ok() != Some(Some(true)) {
                    continue;
                }
                // There may be no video socket until the phone starts mirroring. Reconnect after EOS.
                'connection: while !stop.load(Ordering::Relaxed) {
                    match TcpStream::connect_timeout(&video, IO_WAIT) {
                        Ok(mut source) => {
                            let _ = source.set_read_timeout(Some(IO_WAIT));
                            let mut buffer = [0; 32 * 1024];
                            while !stop.load(Ordering::Relaxed) {
                                match source.read(&mut buffer) {
                                    Ok(0) => break 'connection,
                                    Ok(n) => {
                                        if browser.write_all(&buffer[..n]).is_err() {
                                            break 'connection;
                                        }
                                    }
                                    Err(e)
                                        if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) =>
                                    {
                                        // Detect a browser that went away even while the phone is idle.
                                        if matches!(browser.peek(&mut [0]), Ok(0)) {
                                            break 'connection;
                                        }
                                    }
                                    Err(_) => break 'connection,
                                }
                            }
                            break;
                        }
                        Err(_) => {
                            if matches!(browser.peek(&mut [0]), Ok(0)) {
                                break;
                            }
                            thread::sleep(Duration::from_millis(50));
                        }
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(20)),
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_job_terminates_owned_receiver() {
        let job = Job::new().unwrap();
        let mut child = Process::new("cmd.exe")
            .args(["/c", "ping -n 60 127.0.0.1 > nul"])
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        job.assign(&child).unwrap();
        drop(job);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("receiver survived job closure");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn http_rejects_wrong_tokens_and_origins_and_answers_preflight() {
        for (path, origin, method, expected) in [
            ("/wrong", "https://westplay.cabin", "GET", "403 Forbidden"),
            ("/token", "https://evil.example", "GET", "403 Forbidden"),
            ("/token", "https://westplay.cabin", "OPTIONS", "204 No Content"),
        ] {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let address = listener.local_addr().unwrap();
            let worker = thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket.set_read_timeout(Some(IO_WAIT)).unwrap();
                request(&mut socket, "/token").unwrap();
            });
            let mut browser = TcpStream::connect(address).unwrap();
            browser.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            write!(browser, "{method} {path} HTTP/1.1\r\nOrigin: {origin}\r\n\r\n").unwrap();
            let mut response = String::new();
            browser.read_to_string(&mut response).unwrap();
            assert!(response.contains(expected), "{response}");
            worker.join().unwrap();
        }
    }

    /// Requires a built UI and a short H.264 MPEG-TS fixture. Exercises the real secure-origin WebView2
    /// page, CORS, mpegts.js decoding, and Stop/Home commands without an iOS device or UxPlay installation.
    #[test]
    #[ignore = "runs WebView2; set WESTPLAY_UI and WESTPLAY_AIRPLAY_FIXTURE"]
    fn built_airplay_video_and_controls() {
        use crate::webview::{OffscreenWebView, Stage};
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
        // SAFETY: this test initializes COM on its own thread before constructing the native view.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
        }
        let ui = std::env::var("WESTPLAY_UI").unwrap();
        let bytes = std::fs::read(std::env::var("WESTPLAY_AIRPLAY_FIXTURE").unwrap()).unwrap();
        let source = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        source.set_nonblocking(true).unwrap();
        let video_address = source.local_addr().unwrap();
        let http = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        http.set_nonblocking(true).unwrap();
        let url = format!("http://{}/fixture/airplay.ts", http.local_addr().unwrap());
        struct Fixture {
            stop: Arc<AtomicBool>,
            threads: Vec<JoinHandle<()>>,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                self.stop.store(true, Ordering::Relaxed);
                for worker in self.threads.drain(..) {
                    let _ = worker.join();
                }
            }
        }
        let mut fixture = Fixture { stop: Arc::new(AtomicBool::new(false)), threads: Vec::new() };
        let stop = fixture.stop.clone();
        fixture.threads.push(thread::spawn(move || serve(http, video_address, "/fixture/airplay.ts", &stop)));
        let stop = fixture.stop.clone();
        let connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let connected = connections.clone();
        fixture.threads.push(thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Ok((mut socket, _)) = source.accept() {
                    connected.fetch_add(1, Ordering::Relaxed);
                    socket.set_write_timeout(Some(IO_WAIT)).unwrap();
                    for chunk in bytes.chunks(1316) {
                        if stop.load(Ordering::Relaxed) || socket.write_all(chunk).is_err() {
                            break;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                } else {
                    thread::sleep(Duration::from_millis(20));
                }
            }
        }));
        let stage = Stage::new(1280, 720).unwrap();
        let view = OffscreenWebView::new(
            &stage,
            1280,
            720,
            &std::env::temp_dir().join(format!("westplay-airplay-ui-{}", std::process::id())),
        )
        .unwrap();
        view.map_folder("westplay.cabin", Path::new(&ui)).unwrap();
        stage.add(&view, true).unwrap();
        view.add_startup_script("document.startViewTransition=undefined; window.__errors=[]; addEventListener('error',e=>__errors.push(e.message)); addEventListener('unhandledrejection',e=>__errors.push(String(e.reason))); localStorage.setItem('westplay.cabin',JSON.stringify({version:3,state:{navigation:{route:{kind:'home'},base:'home',recents:[]}}}))").unwrap();
        view.navigate("https://westplay.cabin/cabin.html").unwrap();
        let pump = || {
            let end = std::time::Instant::now() + Duration::from_millis(500);
            while std::time::Instant::now() < end {
                crate::pump_messages();
                thread::sleep(Duration::from_millis(10));
            }
        };
        for _ in 0..4 {
            pump();
        }
        let telemetry = westplay_shared::Telemetry { electric_enabled: true, ..Default::default() };
        view.post_json(&serde_json::json!({"type":"telemetry", "data":telemetry}).to_string()).unwrap();
        for _ in 0..20 {
            pump();
            if view.evaluate("!!document.querySelector('button.app[data-app=airplay]')").unwrap() == "true" {
                break;
            }
        }
        view.evaluate("document.querySelector('button.app[data-app=airplay]').click()").unwrap();
        for _ in 0..4 {
            pump();
        }
        view.capture_png(&std::env::temp_dir().join("airplay-waiting.png")).unwrap();
        assert_eq!(view.evaluate("document.querySelector('.airplay-toolbar button').disabled").unwrap(), "false");
        view.take_page_messages();
        view.evaluate("document.querySelector('.airplay-toolbar button').click()").unwrap();
        pump();
        assert!(view.take_page_messages().iter().any(|message| message.contains("\"action\":\"start\"")));
        view.post_json(&State { running: true, url: Some(url), error: None }.message()).unwrap();
        let mut decoded = false;
        for _ in 0..24 {
            pump();
            decoded = view.evaluate("document.querySelector('.airplay-picture video').videoWidth === 320 && document.querySelector('.airplay-picture video').currentTime > 0.1").unwrap() == "true";
            if decoded {
                break;
            }
        }
        view.capture_png(&std::env::temp_dir().join("airplay-video.png")).unwrap();
        assert!(decoded, "video did not decode; {}", view.evaluate("JSON.stringify({errors:__errors,video:document.querySelector('video')?.error?.message,state:document.querySelector('[role=status]')?.textContent})").unwrap());
        view.take_page_messages();
        // Each fixture stream ends after one clip: reconnect and decode the next one.
        for _ in 0..30 {
            pump();
            if connections.load(Ordering::Relaxed) >= 2
                && view.evaluate("document.querySelector('.airplay-picture video').currentTime > 0.1 && document.querySelector('.airplay-picture video').readyState >= 2").unwrap() == "true" {
                break;
            }
        }
        assert!(connections.load(Ordering::Relaxed) >= 2, "player did not reconnect after EOF");
        assert_eq!(view.evaluate("document.querySelector('.airplay-picture video').readyState >= 2").unwrap(), "true");
        view.evaluate("document.querySelector('.airplay-toolbar button').click()").unwrap();
        pump();
        assert!(view.take_page_messages().iter().any(|message| message.contains("\"action\":\"stop\"")));
        view.post_json(&State::default().message()).unwrap();
        pump();
        view.take_page_messages();
        view.evaluate("document.querySelector('.home-button').click()").unwrap();
        pump();
        assert!(view.take_page_messages().iter().any(|message| message.contains("\"action\":\"stop\"")));
        assert_eq!(view.evaluate("JSON.stringify(__errors)").unwrap(), "\"[]\"");
    }

    #[test]
    fn video_configuration_keeps_native_audio_and_uses_loopback() {
        let args = arguments(43210);
        assert!(!args.iter().any(|s| s == "-a" || s == "-as" || s == "-h265"));
        let config = gui_arguments(&args);
        assert!(config.contains("-vrtp \"config-interval=1 ! rtph264depay"));
        assert!(config.ends_with("host=127.0.0.1 port=43210\""));
    }

    #[test]
    fn only_cabin_and_local_development_origins_can_read_video() {
        for origin in ["https://westplay.cabin", "http://localhost:5173", "http://127.0.0.1:5173"] {
            assert!(allowed_origin(origin));
        }
        for origin in [
            "null",
            "https://evil.example",
            "https://westplay.cabin.evil",
            "http://localhost@evil.example",
            "http://127.0.0.1/evil",
        ] {
            assert!(!allowed_origin(origin));
        }
    }

    #[test]
    fn relay_streams_bytes_and_stops_while_phone_is_idle() {
        let source = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let video = source.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::spawn(move || serve(listener, video, "/secret/airplay.ts", &worker_stop));
        let mut browser = TcpStream::connect(address).unwrap();
        browser.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        browser.write_all(b"GET /secret/airplay.ts HTTP/1.1\r\nOrigin: https://westplay.cabin\r\n\r\n").unwrap();
        source.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut producer = loop {
            if let Ok((client, _)) = source.accept() {
                break client;
            }
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        };
        producer.write_all(b"test-ts-payload").unwrap();
        let mut received = Vec::new();
        while !received.ends_with(b"test-ts-payload") {
            let mut bytes = [0; 512];
            let n = browser.read(&mut bytes).unwrap();
            assert!(n > 0);
            received.extend_from_slice(&bytes[..n]);
        }
        assert!(String::from_utf8_lossy(&received).contains("Access-Control-Allow-Origin: https://westplay.cabin"));
        stop.store(true, Ordering::Relaxed);
        worker.join().unwrap();
    }
}
