//! An isolated, lazily created web browser composed into the cabin frame.
use crate::webview::{Error, OffscreenWebView, Stage};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_PERMISSION_STATE_DENY, ICoreWebView2_4, ICoreWebView2Environment,
};
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, CoTaskMemPWSTR, DownloadStartingEventHandler,
    NavigationCompletedEventHandler, NavigationStartingEventHandler, NewWindowRequestedEventHandler,
    PermissionRequestedEventHandler,
};
use windows::core::BOOL;
use windows::core::Interface;
use windows::core::{HSTRING, PWSTR};

/// Stable identities also used by the cabin's fixed web-app registry.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BrowserId {
    #[default]
    Browser,
    YandexMusic,
}

impl BrowserId {
    pub fn profile(self) -> &'static str {
        match self {
            Self::Browser => "site-browser",
            Self::YandexMusic => "site-yandex-music",
        }
    }
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum Command {
    Navigate { url: String },
    Back,
    Forward,
    Reload,
    Stop,
    Hide,
    Close,
    Show { x: f32, y: f32, width: u32, height: u32 },
    Zoom { factor: f64 },
    Input { event: Input },
    Media { control: MediaControl },
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaControl {
    PlayPause,
    Next,
    Previous,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Media {
    title: String,
    artist: String,
    album: String,
    artwork: Option<String>,
    playing: bool,
    play_pause: bool,
    next: bool,
    previous: bool,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Input {
    Text { text: String },
    Key { key: String },
}

#[derive(Default, Serialize)]
struct State {
    url: String,
    title: String,
    loading: bool,
    back: bool,
    forward: bool,
    error: Option<String>,
    media: Option<Media>,
    #[serde(skip)]
    generation: u64,
    #[serde(skip)]
    polling: bool,
    #[serde(skip)]
    failed: bool,
}

/// Only web navigation, including redirects and popups. No local files, scripts, credentials or app origins.
pub fn address(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() || input.len() > 8192 {
        return None;
    }
    let candidate = if input.contains("://")
        || input.starts_with("about:")
        || input.starts_with("javascript:")
        || input.starts_with("data:")
        || input.starts_with("file:")
    {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    let parsed = url::Url::parse(&candidate).ok()?;
    let host = parsed.host_str()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || host == "westplay.cabin"
        || host == "westplay.maps"
        || host == "westplay.customize"
    {
        return None;
    }
    Some(parsed.to_string())
}

pub struct Browser {
    pub view: OffscreenWebView,
    pub area: Option<(f32, f32, u32, u32)>,
    state: Rc<RefCell<State>>,
    focus: Rc<RefCell<Option<bool>>>,
    pub check_focus: bool,
    last_poll: Instant,
    media_poll_started: Instant,
    sent: String,
}

impl Browser {
    pub fn resend(&mut self) {
        self.sent.clear();
    }

    pub fn new(stage: &Stage, environment: &ICoreWebView2Environment, id: BrowserId) -> Result<Self, Error> {
        let view = OffscreenWebView::with_profile(stage, 800, 600, environment, id.profile())?;
        let core = view.core();
        view.add_startup_script(include_str!("site_media.js"))?;
        let state = Rc::new(RefCell::new(State::default()));
        let focus = Rc::new(RefCell::new(None));
        // SAFETY: these COM objects are alive on their owning STA thread. No remote-page messaging or host objects.
        unsafe {
            let settings = core.Settings()?;
            settings.SetIsWebMessageEnabled(false)?;
            settings.SetAreHostObjectsAllowed(false)?;
            settings.SetAreDevToolsEnabled(false)?;
            let s = state.clone();
            let f = focus.clone();
            let starting = NavigationStartingEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut uri = PWSTR::null();
                    args.Uri(&mut uri)?;
                    let uri = CoTaskMemPWSTR::from(uri).to_string();
                    if address(&uri).is_none() {
                        args.SetCancel(true)?;
                        return Ok(());
                    }
                    let mut s = s.borrow_mut();
                    *f.borrow_mut() = Some(false);
                    s.loading = true;
                    s.generation += 1;
                    s.media = None;
                    s.polling = false;
                    s.failed = false;
                    s.error = None;
                    s.url = uri;
                }
                Ok(())
            }));
            let mut token = 0;
            core.add_NavigationStarting(&starting, &mut token)?;
            let s = state.clone();
            let completed = NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                let mut s = s.borrow_mut();
                s.loading = false;
                if let Some(args) = args {
                    let mut success = BOOL::default();
                    args.IsSuccess(&mut success)?;
                    if !success.as_bool() {
                        s.error = Some("Could not load this page. Check the address or connection and retry.".into());
                    }
                }
                Ok(())
            }));
            core.add_NavigationCompleted(&completed, &mut token)?;
            let popup = NewWindowRequestedEventHandler::create(Box::new(move |sender, args| {
                if let Some(args) = args {
                    args.SetHandled(true)?;
                    let mut uri = PWSTR::null();
                    args.Uri(&mut uri)?;
                    let uri = CoTaskMemPWSTR::from(uri).to_string();
                    if let (Some(core), Some(uri)) = (sender, address(&uri)) {
                        core.Navigate(&HSTRING::from(uri))?;
                    }
                }
                Ok(())
            }));
            core.add_NewWindowRequested(&popup, &mut token)?;
            let permission = PermissionRequestedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                }
                Ok(())
            }));
            core.add_PermissionRequested(&permission, &mut token)?;
            let s = state.clone();
            let download = DownloadStartingEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    args.SetCancel(true)?;
                    args.SetHandled(true)?;
                }
                s.borrow_mut().error = Some("File downloads are not available in the cabin browser.".into());
                Ok(())
            }));
            core.cast::<ICoreWebView2_4>()?.add_DownloadStarting(&download, &mut token)?;
        }
        stage.add(&view, true)?;
        view.show(false)?;
        Ok(Self {
            view,
            area: None,
            state,
            focus,
            check_focus: false,
            last_poll: Instant::now() - Duration::from_secs(1),
            media_poll_started: Instant::now(),
            sent: String::new(),
        })
    }

    pub fn command(&mut self, command: Command, stage: &Stage) -> Result<(), Error> {
        match command {
            Command::Navigate { url } => {
                if let Some(url) = address(&url) {
                    self.view.navigate(&url)?;
                } else {
                    self.state.borrow_mut().error = Some("Enter an HTTP or HTTPS address.".into());
                }
            }
            Command::Show { x, y, width, height } => {
                if self.area.is_none() {
                    self.sent.clear();
                }
                let (sw, sh) = stage.page_size().get();
                if !x.is_finite()
                    || !y.is_finite()
                    || x < 0.0
                    || y < 0.0
                    || width == 0
                    || height == 0
                    || x + width as f32 > sw as f32 + 1.0
                    || y + height as f32 > sh as f32 + 1.0
                {
                    return Ok(());
                }
                self.view.place(x, y, width, height)?;
                self.view.show(true)?;
                self.view.set_muted(false)?;
                self.area = Some((x, y, width, height));
            }
            Command::Hide => {
                // Hide only the composition visual. Keep the page and its audio running in the background.
                self.area = None;
                self.view.show(false)?;
            }
            Command::Close => {
                self.area = None;
                self.view.show(false)?;
                self.view.set_muted(true)?;
            }
            Command::Reload => {
                self.view.reload()?;
            }
            Command::Zoom { factor } if factor.is_finite() => {
                self.view.zoom(factor.clamp(0.5, 2.0))?;
            }
            // On a small screen the keyboard can cover the entire surface. The trusted cabin still owns
            // the selected field, and detaches it when leaving the browser.
            Command::Input { event } => {
                self.input(event)?;
            }
            Command::Media { control } => {
                let action = serde_json::to_string(&control).unwrap();
                self.view.run_script(&format!("window.westplaySiteMedia?.control({action})"), |_| {})?;
            }
            command => {
                // SAFETY: the control is alive on this STA; navigation commands have no pointer arguments.
                unsafe {
                    match command {
                        Command::Back => self.view.core().GoBack()?,
                        Command::Forward => self.view.core().GoForward()?,
                        Command::Stop => {
                            self.view.core().Stop()?;
                            self.state.borrow_mut().loading = false;
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    fn input(&self, event: Input) -> windows::core::Result<()> {
        match event {
            Input::Text { text } => self
                .devtools("Input.insertText", serde_json::json!({"text": text.chars().take(8192).collect::<String>()})),
            Input::Key { key } => {
                let (name, code, modifiers) = match key.as_str() {
                    "backspace" => ("Backspace", 8, 0),
                    "wordBackspace" => ("Backspace", 8, 2),
                    "enter" => ("Enter", 13, 0),
                    "escape" => ("Escape", 27, 0),
                    "tab" => ("Tab", 9, 0),
                    "delete" => ("Delete", 46, 0),
                    "left" => ("ArrowLeft", 37, 0),
                    "right" => ("ArrowRight", 39, 0),
                    "home" => ("Home", 36, 0),
                    "end" => ("End", 35, 0),
                    "selectAll" => ("a", 65, 2),
                    _ => return Ok(()),
                };
                for kind in ["keyDown", "keyUp"] {
                    let mut args =
                        serde_json::json!({"type":kind,"key":name,"windowsVirtualKeyCode":code,"modifiers":modifiers});
                    if code == 13 && kind == "keyDown" {
                        args["text"] = "\r".into();
                    }
                    self.devtools("Input.dispatchKeyEvent", args)?;
                }
                Ok(())
            }
        }
    }

    fn devtools(&self, method: &str, args: serde_json::Value) -> windows::core::Result<()> {
        let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(())));
        // SAFETY: fixed host-owned method names, JSON encoded arguments, and a retained completion handler.
        unsafe {
            self.view.core().CallDevToolsProtocolMethod(
                &HSTRING::from(method),
                &HSTRING::from(args.to_string()),
                &handler,
            )
        }
    }

    pub fn tick(&mut self, cabin: &OffscreenWebView, id: BrowserId) {
        if let Some(editable) = self.focus.borrow_mut().take() {
            let _ = cabin.post_json(
                &serde_json::json!({"type":"browserFocus","id":id,"editable":editable && self.area.is_some()})
                    .to_string(),
            );
        }
        if self.last_poll.elapsed() < Duration::from_millis(200) {
            return;
        }
        self.last_poll = Instant::now();
        if self.view.take_failure().is_some() {
            self.state.borrow_mut().generation += 1;
            self.state.borrow_mut().media = None;
            self.state.borrow_mut().failed = true;
            self.state.borrow_mut().error = Some("Browser stopped. Close and reopen it to try again.".into());
            self.state.borrow_mut().loading = false;
        }
        // ExecuteScript's error callback does not deliver a value. Recover instead of leaving polling stuck.
        if self.state.borrow().polling && self.media_poll_started.elapsed() > Duration::from_secs(5) {
            let mut s = self.state.borrow_mut();
            s.generation += 1;
            s.polling = false;
            s.media = None;
        }
        if !self.state.borrow().loading && !self.state.borrow().polling && !self.state.borrow().failed {
            self.media_poll_started = Instant::now();
            let state = self.state.clone();
            let generation = state.borrow().generation;
            state.borrow_mut().polling = true;
            if self
                .view
                .run_script("window.westplaySiteMedia?.read() ?? null", move |value| {
                    let mut s = state.borrow_mut();
                    if s.generation != generation {
                        return;
                    }
                    s.polling = false;
                    s.media = serde_json::from_str::<Option<Media>>(&value).ok().flatten().map(|mut media| {
                        media.title = media.title.chars().take(2048).collect();
                        media.artist = media.artist.chars().take(2048).collect();
                        media.album = media.album.chars().take(2048).collect();
                        media.artwork = media.artwork.and_then(|url| address(&url));
                        media
                    });
                })
                .is_err()
            {
                self.state.borrow_mut().polling = false;
                self.state.borrow_mut().media = None;
            }
        }
        // SAFETY: native properties from a live control; strings freed by CoTaskMemPWSTR.
        unsafe {
            let core = self.view.core();
            let mut s = self.state.borrow_mut();
            let mut title = PWSTR::null();
            if core.DocumentTitle(&mut title).is_ok() {
                s.title = CoTaskMemPWSTR::from(title).to_string();
            }
            let mut uri = PWSTR::null();
            if core.Source(&mut uri).is_ok() {
                let uri = CoTaskMemPWSTR::from(uri).to_string();
                if address(&uri).is_some() {
                    s.url = uri;
                }
            }
            let mut back = BOOL::default();
            let mut forward = BOOL::default();
            let _ = core.CanGoBack(&mut back);
            let _ = core.CanGoForward(&mut forward);
            s.back = back.as_bool();
            s.forward = forward.as_bool();
        }
        if self.check_focus && self.area.is_some() {
            self.check_focus = false;
            let focus = self.focus.clone();
            // Only a boolean crosses the boundary. No field values or page-supplied commands are consumed.
            let _ = self.view.run_script("(() => { let e = document.activeElement; while (e?.shadowRoot?.activeElement) e = e.shadowRoot.activeElement; return !!e && (e.isContentEditable || e.tagName === 'IFRAME' || ((e.tagName === 'INPUT' || e.tagName === 'TEXTAREA') && !e.disabled && !e.readOnly)); })()", move |value| {
                *focus.borrow_mut() = Some(value == "true");
            });
        }
        let mut value = serde_json::to_value(&*self.state.borrow()).unwrap_or_default();
        value["type"] = "browserState".into();
        value["id"] = serde_json::to_value(id).unwrap();
        let message = value.to_string();
        if message != self.sent {
            let _ = cabin.post_json(&message);
            self.sent = message;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    #[test]
    #[ignore = "renders the built browser controls; WESTPLAY_UI names the UI dist directory"]
    fn built_browser_controls() {
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
        // SAFETY: COM is initialized on this test's own thread before creating the view.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
        }
        let ui = std::env::var("WESTPLAY_UI").expect("WESTPLAY_UI");
        let stage = Stage::new(1280, 720).unwrap();
        let view = OffscreenWebView::new(
            &stage,
            1280,
            720,
            &std::env::temp_dir().join(format!("westplay-browser-ui-{}", std::process::id())),
        )
        .unwrap();
        view.map_folder("westplay.cabin", Path::new(&ui)).unwrap();
        stage.add(&view, true).unwrap();
        // A layout preview has no continuous frame capture to advance native view transitions.
        view.add_startup_script("document.startViewTransition = undefined; window.__errors=[]; addEventListener('error',e=>__errors.push(e.message)); addEventListener('unhandledrejection',e=>__errors.push(String(e.reason)))").unwrap();
        view.add_startup_script("localStorage.setItem('westplay.cabin', JSON.stringify({version:3,state:{navigation:{route:{kind:'home'},base:'home',recents:[]}}}))").unwrap();
        view.navigate("https://westplay.cabin/cabin.html").unwrap();
        let pump = || {
            let until = Instant::now() + Duration::from_secs(2);
            while Instant::now() < until {
                crate::pump_messages();
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        pump();
        let telemetry = westplay_shared::Telemetry { electric_enabled: true, ..Default::default() };
        let powered = serde_json::json!({"type":"telemetry","data":telemetry}).to_string();
        view.post_json(&powered).unwrap();
        for _ in 0..5 {
            if view.evaluate("!!document.querySelector('button.app[data-app=browser]')").unwrap() == "true" {
                break;
            }
            pump();
        }
        view.evaluate("document.querySelector('button.app[data-app=browser]').click()").unwrap();
        pump();
        view.capture_png(&std::env::temp_dir().join("browser-preview.png")).unwrap();
        assert_eq!(
            view.evaluate("document.querySelector('.browser-toolbar [role=textbox]').getAttribute('aria-readonly')")
                .unwrap(),
            "\"false\""
        );
        view.evaluate("document.querySelector('.home-button').click()").unwrap();
        pump();
        view.evaluate("document.querySelector('button.app[data-app=yandexMusic]').click()").unwrap();
        pump();
        assert_eq!(
            view.evaluate("document.querySelector('.browser-toolbar [role=textbox]').getAttribute('aria-readonly')")
                .unwrap(),
            "\"true\""
        );
        assert_eq!(
            view.evaluate("document.querySelector('.browser-toolbar [role=textbox]').textContent").unwrap(),
            "\"https://music.yandex.ru/\""
        );
        assert_eq!(
            view.evaluate(
                "document.querySelectorAll('.browser-app iframe, .browser-toolbar .text-field-clear').length"
            )
            .unwrap(),
            "0"
        );
        view.evaluate("document.querySelector('.browser-toolbar [role=textbox]').dispatchEvent(new PointerEvent('pointerdown', {bubbles:true, button:0}))").unwrap();
        pump();
        assert_eq!(view.evaluate("!!document.querySelector('.keyboard.shown')").unwrap(), "false");
        for (id, url) in
            [(BrowserId::Browser, "https://example.org/"), (BrowserId::YandexMusic, "https://music.yandex.ru/album/1")]
        {
            view.post_json(&serde_json::json!({"type":"browserState","id":id,"url":url,"title":"Test","loading":false,"back":false,"forward":false,"error":null}).to_string()).unwrap();
            pump();
            let expected = if id == BrowserId::Browser { "https://music.yandex.ru/" } else { url };
            assert_eq!(
                view.evaluate("document.querySelector('.browser-toolbar [role=textbox]').textContent").unwrap(),
                serde_json::to_string(expected).unwrap()
            );
        }
        assert_eq!(view.evaluate("window.__errors").unwrap(), "[]");
        println!(
            "browser preview: {}",
            view.evaluate("({errors:window.__errors,url:location.href, text:document.body.innerText.slice(0,600)})")
                .unwrap()
        );
        assert_eq!(view.evaluate("!!document.querySelector('.browser-toolbar')").unwrap(), "true");
        assert_eq!(
            view.evaluate("document.querySelector('.browser-surface').getBoundingClientRect().height > 100").unwrap(),
            "true"
        );
        view.capture_png(&std::env::temp_dir().join("browser-preview.png")).unwrap();
        // Closing releases the selected native browser and leaves its screen. Reopening must navigate
        // again, rather than trying to show the already-destroyed control (including fixed web apps).
        for id in ["yandexMusic", "browser"] {
            view.evaluate("document.querySelector('.home-button').click()").unwrap();
            pump();
            view.evaluate(&format!("document.querySelector('button.app[data-app={id}]').click()")).unwrap();
            pump();
            view.take_page_messages();
            view.evaluate(
                "[...document.querySelectorAll('.browser-info button')].find(b => b.textContent === 'Close').click()",
            )
            .unwrap();
            pump();
            assert_eq!(view.evaluate("!!document.querySelector('.browser-toolbar')").unwrap(), "false");
            let commands: Vec<serde_json::Value> = view
                .take_page_messages()
                .iter()
                .filter_map(|message| serde_json::from_str::<serde_json::Value>(message).ok())
                .filter(|message| message["type"] == "browser")
                .collect();
            assert!(commands.iter().any(|message| message["id"] == id && message["command"]["action"] == "close"));
            assert!(
                !commands
                    .iter()
                    .any(|message| matches!(message["command"]["action"].as_str(), Some("show" | "navigate")))
            );
            view.evaluate(&format!("document.querySelector('button.app[data-app={id}]').click()")).unwrap();
            pump();
            assert!(view.take_page_messages().iter().any(|message| {
                let message: serde_json::Value = serde_json::from_str(message).unwrap();
                message["type"] == "browser" && message["id"] == id && message["command"]["action"] == "navigate"
            }));
        }
        view.post_json(r#"{"type":"game","running":false}"#).unwrap();
        pump();
        assert_eq!(view.evaluate("!!document.querySelector('.browser-toolbar')").unwrap(), "false");
        view.take_page_messages();
        view.post_json(r#"{"type":"game","running":true}"#).unwrap();
        view.post_json(&powered).unwrap();
        for _ in 0..4 {
            pump();
        }
        assert!(view.take_page_messages().iter().any(|message| {
            let message: serde_json::Value = serde_json::from_str(message).unwrap();
            message["type"] == "browser" && message["id"] == "browser" && message["command"]["action"] == "navigate"
        }));
    }
    #[test]
    fn accepts_web_addresses_only() {
        assert_eq!(address(" example.org/a?q=1 ").unwrap(), "https://example.org/a?q=1");
        assert_eq!(address("http://localhost:8080").unwrap(), "http://localhost:8080/");
        for value in [
            "",
            "javascript:alert(1)",
            "file:///C:/secret",
            "data:text/html,x",
            "about:blank",
            "ftp://example.org",
            "https://user:pass@example.org",
            "https://westplay.cabin/cabin.html",
        ] {
            assert!(address(value).is_none(), "{value}");
        }
    }

    /// Exercises the real native control, including mouse focus and Chromium editing in a cross-origin frame.
    #[test]
    #[ignore = "runs WebView2 against a local HTTP fixture"]
    fn native_navigation_and_input() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            sync::{
                Arc,
                atomic::{AtomicBool, Ordering},
            },
        };
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN, COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP,
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON, COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
        };
        use windows::Win32::Foundation::POINT;
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
        // SAFETY: COM is initialized once on this test's own STA thread before creating any controls.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
        }
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        server.set_nonblocking(true).unwrap();
        struct Server(Arc<AtomicBool>);
        impl Drop for Server {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let _server = Server(stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Ok((mut stream, _)) = server.accept() {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                    let mut bytes = [0; 4096];
                    let count = stream.read(&mut bytes).unwrap_or(0);
                    let request = String::from_utf8_lossy(&bytes[..count]);
                    let page = if request.starts_with("GET /frame") {
                        "<input id=p style='position:absolute;left:10px;top:10px;width:200px;height:30px' oninput='parent.postMessage({testValue:this.value},\"*\")'>".to_string()
                    } else if request.starts_with("GET /second") {
                        "<title>Second</title><p>Second page</p>".to_string()
                    } else {
                        format!(
                            "<title>Fixture</title><script>addEventListener('message',e=>{{if(e.origin==='http://localhost:{port}')window.frameValue=e.data.testValue}})</script><input id=p type=password style='position:absolute;left:20px;top:20px;width:200px;height:30px'><iframe src='http://localhost:{port}/frame' style='position:absolute;left:20px;top:100px;width:300px;height:120px;border:0'></iframe>"
                        )
                    };
                    let body = format!("<!doctype html><meta charset=utf-8>{page}");
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        });
        let pump = |ms| {
            let end = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < end {
                crate::pump_messages();
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let profile = std::env::temp_dir().join(format!("westplay-browser-test-{}", std::process::id()));
        let stage = Stage::new(800, 600).unwrap();
        let cabin = OffscreenWebView::new(&stage, 800, 600, &profile).unwrap();
        let mut browser = Browser::new(&stage, cabin.environment(), BrowserId::Browser).unwrap();
        browser.command(Command::Show { x: 0.0, y: 0.0, width: 800, height: 600 }, &stage).unwrap();
        browser.command(Command::Navigate { url: format!("http://127.0.0.1:{port}/") }, &stage).unwrap();
        pump(2500);
        assert_eq!(browser.view.evaluate("document.title").unwrap(), "\"Fixture\"");
        let click = |x, y| {
            let point = POINT { x, y };
            browser
                .view
                .send_mouse(
                    COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN,
                    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON,
                    0,
                    point,
                )
                .unwrap();
            browser
                .view
                .send_mouse(
                    COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP,
                    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
                    0,
                    point,
                )
                .unwrap();
            pump(100);
        };
        click(50, 35);
        browser.input(Input::Text { text: "Привет".into() }).unwrap();
        pump(150);
        browser.input(Input::Key { key: "backspace".into() }).unwrap();
        pump(150);
        assert_eq!(browser.view.evaluate("document.getElementById('p').value").unwrap(), "\"Приве\"");
        // The local fixture reports its own input for this assertion; production pages have no bridge.
        click(60, 130);
        browser.input(Input::Text { text: "frame".into() }).unwrap();
        pump(150);
        assert_eq!(browser.view.evaluate("document.activeElement.tagName").unwrap(), "\"IFRAME\"");
        assert_eq!(browser.view.evaluate("window.frameValue").unwrap(), "\"frame\"");
        browser.input(Input::Key { key: "selectAll".into() }).unwrap();
        pump(100);
        browser.input(Input::Text { text: "replaced".into() }).unwrap();
        pump(100);
        assert_eq!(browser.view.evaluate("window.frameValue").unwrap(), "\"replaced\"");
        assert!(browser.view.take_page_messages().is_empty());
        browser.view.evaluate("document.cookie='browserTest=kept;path=/;max-age=3600'").unwrap();
        browser.command(Command::Navigate { url: format!("http://127.0.0.1:{port}/second") }, &stage).unwrap();
        pump(500);
        assert_eq!(browser.view.evaluate("document.title").unwrap(), "\"Second\"");
        assert!(browser.view.evaluate("document.cookie").unwrap().contains("browserTest=kept"));
        browser.command(Command::Back, &stage).unwrap();
        pump(500);
        assert_eq!(browser.view.evaluate("document.title").unwrap(), "\"Fixture\"");
        browser.command(Command::Forward, &stage).unwrap();
        pump(500);
        assert_eq!(browser.view.evaluate("document.title").unwrap(), "\"Second\"");
        browser.command(Command::Navigate { url: "file:///C:/Windows/win.ini".into() }, &stage).unwrap();
        pump(200);
        assert_eq!(browser.view.evaluate("document.title").unwrap(), "\"Second\"");
        browser.command(Command::Zoom { factor: 1.2 }, &stage).unwrap();
        let mut other = Browser::new(&stage, cabin.environment(), BrowserId::YandexMusic).unwrap();
        other.command(Command::Navigate { url: format!("http://127.0.0.1:{port}/") }, &stage).unwrap();
        pump(1000);
        assert_eq!(other.view.evaluate("document.cookie").unwrap(), "\"\"");
        other.view.evaluate("document.cookie='otherBrowser=kept;path=/'").unwrap();
        // One browser process, three isolated profiles, even on the same origin.
        cabin.navigate(&format!("http://127.0.0.1:{port}/")).unwrap();
        pump(500);
        let browser_pid = |view: &OffscreenWebView| {
            let mut pid = 0;
            // SAFETY: the test owns a live WebView on this STA; pid is an output value.
            unsafe {
                view.core().BrowserProcessId(&mut pid).unwrap();
            }
            pid
        };
        assert_ne!(browser_pid(&cabin), 0);
        assert_eq!(browser_pid(&browser.view), browser_pid(&cabin));
        assert_eq!(browser_pid(&other.view), browser_pid(&cabin));
        assert_eq!(cabin.evaluate("document.cookie").unwrap(), "\"\"");
        cabin.evaluate("document.cookie='cabinOnly=kept;path=/'; localStorage.setItem('owner','cabin')").unwrap();
        browser.view.evaluate("localStorage.setItem('owner','browser')").unwrap();
        assert_eq!(other.view.evaluate("localStorage.getItem('owner')").unwrap(), "null");
        assert_eq!(cabin.evaluate("localStorage.getItem('owner')").unwrap(), "\"cabin\"");
        assert!(!browser.view.evaluate("document.cookie").unwrap().contains("cabinOnly"));
        other.view.delete_cookies().unwrap();
        assert!(browser.view.evaluate("document.cookie").unwrap().contains("browserTest=kept"));
        assert!(cabin.evaluate("document.cookie").unwrap().contains("cabinOnly=kept"));
        other.view.evaluate("document.cookie='otherBrowser=kept;path=/'").unwrap();
        // Play a silent PCM WAV through the real media pipeline, without external sites or audible test noise.
        browser
            .view
            .evaluate(
                r#"(() => {
            const wav = new ArrayBuffer(44 + 8000 * 30);
            const header = new DataView(wav);
            const bytes = new Uint8Array(wav);
            const text = (offset, value) => [...value].forEach((c, i) => bytes[offset + i] = c.charCodeAt(0));
            text(0, 'RIFF'); header.setUint32(4, wav.byteLength - 8, true); text(8, 'WAVE');
            text(12, 'fmt '); header.setUint32(16, 16, true); header.setUint16(20, 1, true);
            header.setUint16(22, 1, true); header.setUint32(24, 8000, true); header.setUint32(28, 8000, true);
            header.setUint16(32, 1, true); header.setUint16(34, 8, true);
            text(36, 'data'); header.setUint32(40, wav.byteLength - 44, true); bytes.fill(128, 44);
            window.testAudio = new Audio(URL.createObjectURL(new Blob([wav], {type: 'audio/wav'})));
            testAudio.play();
        })()"#,
            )
            .unwrap();
        pump(500);
        browser.view.evaluate("navigator.mediaSession.metadata = new MediaMetadata({title:'Fixture song',artist:'Fixture artist'}); navigator.mediaSession.setActionHandler('nexttrack', () => navigator.mediaSession.metadata = new MediaMetadata({title:'Next song'}))").unwrap();
        browser.tick(&cabin, BrowserId::Browser);
        pump(300);
        assert_eq!(browser.state.borrow().media.as_ref().unwrap().title, "Fixture song");
        assert!(browser.state.borrow().media.as_ref().unwrap().playing);
        browser.command(Command::Hide, &stage).unwrap();
        browser.command(Command::Media { control: MediaControl::PlayPause }, &stage).unwrap();
        pump(300);
        assert_eq!(browser.view.evaluate("testAudio.paused").unwrap(), "true");
        browser.tick(&cabin, BrowserId::Browser);
        pump(300);
        assert!(!browser.state.borrow().media.as_ref().unwrap().playing);
        browser.command(Command::Media { control: MediaControl::Next }, &stage).unwrap();
        pump(300);
        browser.tick(&cabin, BrowserId::Browser);
        pump(300);
        assert_eq!(browser.state.borrow().media.as_ref().unwrap().title, "Next song");
        browser.command(Command::Media { control: MediaControl::PlayPause }, &stage).unwrap();
        pump(300);
        for _ in 0..2 {
            assert_eq!(browser.view.evaluate("!testAudio.paused && testAudio.currentTime > 0").unwrap(), "true");
            browser.view.evaluate("window.beforeHide = testAudio.currentTime").unwrap();
            browser.command(Command::Hide, &stage).unwrap();
            assert!(browser.area.is_none());
            other.command(Command::Show { x: 0.0, y: 0.0, width: 800, height: 600 }, &stage).unwrap();
            assert_eq!(other.view.evaluate("document.title").unwrap(), "\"Fixture\"");
            pump(500);
            let mut muted = BOOL::default();
            // SAFETY: the browser's control is alive on this test's STA thread; `muted` is an output value.
            unsafe {
                use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
                browser.view.core().cast::<ICoreWebView2_8>().unwrap().IsMuted(&mut muted).unwrap();
            }
            assert!(!muted.as_bool(), "hiding the browser must not mute background playback");
            assert_eq!(
                browser.view.evaluate("!testAudio.paused && testAudio.currentTime > beforeHide + 0.2").unwrap(),
                "true"
            );
            other.command(Command::Hide, &stage).unwrap();
            browser.command(Command::Show { x: 0.0, y: 0.0, width: 800, height: 600 }, &stage).unwrap();
            assert!(browser.area.is_some());
            assert_eq!(browser.view.evaluate("document.title").unwrap(), "\"Second\"");
            assert!(!browser.view.evaluate("document.cookie").unwrap().contains("otherBrowser"));
        }
        browser.view.evaluate("testAudio.pause(); URL.revokeObjectURL(testAudio.src)").unwrap();
        stage.remove(&browser.view).unwrap();
        drop(browser);
        let mut reopened = Browser::new(&stage, cabin.environment(), BrowserId::Browser).unwrap();
        reopened.command(Command::Navigate { url: format!("http://127.0.0.1:{port}/") }, &stage).unwrap();
        pump(1000);
        assert_eq!(browser_pid(&reopened.view), browser_pid(&cabin));
        assert_eq!(reopened.view.evaluate("document.title").unwrap(), "\"Fixture\"");
        assert!(reopened.view.evaluate("document.cookie").unwrap().contains("browserTest=kept"));
        assert_eq!(reopened.view.evaluate("localStorage.getItem('owner')").unwrap(), "\"browser\"");
        assert!(other.view.evaluate("document.cookie").unwrap().contains("otherBrowser=kept"));
        assert_eq!(cabin.evaluate("document.title").unwrap(), "\"Fixture\"");
    }
}
