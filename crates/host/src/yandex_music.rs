//! Scoped frame-ancestor permission for the cabin's full-site Yandex Music iframe.

use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2, ICoreWebView2NavigationStartingEventArgs2};
use webview2_com::{CoTaskMemPWSTR, NavigationStartingEventHandler};
use windows::core::{HSTRING, Interface, PWSTR};

const CABIN_ORIGIN: &str = "https://westplay.cabin";
const MUSIC_ORIGIN: &str = "https://music.yandex.ru";

/// Register only on the cabin WebView, before navigating it. WebView2 retains the handler until close.
pub fn install(webview: &ICoreWebView2) -> windows::core::Result<()> {
    let handler = NavigationStartingEventHandler::create(Box::new(move |webview, args| {
        let (Some(webview), Some(args)) = (webview, args) else { return Ok(()) };
        let mut source = PWSTR::null();
        // SAFETY: the event's WebView is alive; the output is freed by CoTaskMemPWSTR.
        unsafe { webview.Source(&mut source)? };
        let source = CoTaskMemPWSTR::from(source).to_string();
        let mut target = PWSTR::null();
        // SAFETY: args lives through this callback; the output is freed by CoTaskMemPWSTR.
        unsafe { args.Uri(&mut target)? };
        let target = CoTaskMemPWSTR::from(target).to_string();
        let Some(ancestors) = allowed_ancestors(&source, &target) else { return Ok(()) };
        let Ok(args) = args.cast::<ICoreWebView2NavigationStartingEventArgs2>() else {
            log!("[yandex music] this WebView2 runtime does not support AdditionalAllowedFrameAncestors");
            return Ok(());
        };
        // SAFETY: the event args and string live through the call. Only exact trusted origins are added.
        if let Err(error) = unsafe { args.SetAdditionalAllowedFrameAncestors(&HSTRING::from(ancestors)) } {
            log!("[yandex music] could not allow the cabin frame ancestor: {error}");
        }
        Ok(())
    }));
    let mut token = 0;
    // SAFETY: the WebView retains the handler. FrameNavigationStarting also covers nested frames.
    unsafe { webview.add_FrameNavigationStarting(&handler, &mut token) }
}

fn allowed_ancestors(source: &str, target: &str) -> Option<String> {
    let cabin = if at_origin(source, CABIN_ORIGIN) {
        CABIN_ORIGIN
    } else if cfg!(debug_assertions) && at_origin(source, "http://localhost:5173") {
        "http://localhost:5173"
    } else if cfg!(debug_assertions) && at_origin(source, "http://127.0.0.1:5173") {
        "http://127.0.0.1:5173"
    } else {
        return None;
    };
    if at_origin(target, MUSIC_ORIGIN) {
        return Some(cabin.to_string());
    }
    // Sign-in may be nested inside Music: every ancestor must be allowed, including Music itself.
    if ["https://passport.yandex.ru", "https://sso.passport.yandex.ru", "https://sso.ya.ru"]
        .iter()
        .any(|origin| at_origin(target, origin))
    {
        return Some(format!("{cabin} {MUSIC_ORIGIN}"));
    }
    None
}

/// WebView2 supplies absolute URLs. Require the whole origin, never a hostname prefix or substring.
fn at_origin(url: &str, origin: &str) -> bool {
    url.strip_prefix(origin).is_some_and(|rest| {
        rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') || rest.starts_with('#')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_music_and_nested_sign_in_only_from_the_cabin() {
        let cabin = "https://westplay.cabin/cabin.html";
        assert_eq!(allowed_ancestors(cabin, "https://music.yandex.ru/"), Some(CABIN_ORIGIN.into()));
        assert_eq!(
            allowed_ancestors(cabin, "https://passport.yandex.ru/auth"),
            Some(format!("{CABIN_ORIGIN} {MUSIC_ORIGIN}")),
        );
        assert_eq!(allowed_ancestors("https://example.com/", "https://music.yandex.ru/"), None);
        assert_eq!(allowed_ancestors(cabin, "https://example.com/"), None);
    }

    #[test]
    fn rejects_similar_hosts_credentials_other_ports_and_schemes() {
        for target in [
            "https://music.yandex.ru.evil.example/",
            "https://music.yandex.ru@evil.example/",
            "https://music.yandex.ru:8443/",
            "http://music.yandex.ru/",
            "https://example.com/?next=https://music.yandex.ru/",
        ] {
            assert_eq!(allowed_ancestors(CABIN_ORIGIN, target), None, "{target}");
        }
        for source in [
            "https://westplay.cabin.evil.example/",
            "https://westplay.cabin@evil.example/",
            "http://westplay.cabin/",
            "about:blank",
        ] {
            assert_eq!(allowed_ancestors(source, MUSIC_ORIGIN), None, "{source}");
        }
    }

    #[test]
    fn local_development_origins_are_debug_only() {
        for origin in ["http://localhost:5173", "http://127.0.0.1:5173"] {
            assert_eq!(
                allowed_ancestors(&format!("{origin}/cabin.html"), MUSIC_ORIGIN),
                cfg!(debug_assertions).then(|| origin.to_string()),
            );
        }
        assert_eq!(allowed_ancestors("http://localhost:5174/", MUSIC_ORIGIN), None);
    }
}
