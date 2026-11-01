# Cabin browser

Open **Browser** from the cabin Home screen. Enter an HTTP/HTTPS address (a missing scheme defaults to
HTTPS), then press Go. Back, Forward, Reload/Stop and 50–200% zoom are available. The last address is
remembered; cookies and website storage live in a separate `site-browser` profile in WestPlay's data folder.
Restart recreates the control without clearing that profile. Leaving the app hides and mutes the site.

The browser uses a separate offscreen WebView2 composed into the existing game frame. Websites receive
neither the cabin's host commands nor its local folder mappings. Web messages and host objects are disabled.
Navigation accepts web URLs only, including redirects and new-window requests. New windows open in the same
view. File downloads and permission requests (camera, microphone, location, etc.) are disabled in this version.
Some sign-in flows requiring a separate popup window, protected media, extensions and native file dialogs may
not work as in a full desktop browser.

Click a website field to use the cabin keyboard. The game's existing stationary-truck rule also applies to
physical typing. Text is inserted by Chromium, including into cross-origin frames, without sending field
contents or passwords back to the cabin. Backspace, Ctrl+Backspace, Enter, Escape, Tab, Delete, Left/Right,
Home/End and Ctrl+A are supported. Ctrl+V uses the existing cabin paste bridge (up to 500 characters, one line).
The additional physical editing keys require rebuilding the game plugin as well as the host and UI.

## Development checks

`npm run build --prefix ui` and `npm test --prefix ui` check the frontend.
`cargo test -p westplay-host -p westplay-plugin -p westplay-shared` checks the native modules.
`cargo test -p westplay-host site_browser::tests::native_navigation_and_input -- --ignored` exercises a real
WebView2 against a local HTTP fixture, including password editing, cross-origin input, cookies and history.
With `WESTPLAY_UI` pointing to the absolute `ui/dist` directory, the ignored `built_browser_controls` test
checks the built React page and writes `browser-preview.png` into the process's temporary directory.

`tools/dev/portable-rust.ps1` optionally downloads checksum-verified official Rust archives into `.build-tools`.
It does not run an installer, change persistent PATH or register Rust. For an isolated build, set `CARGO_HOME`,
`CARGO_TARGET_DIR`, `TEMP` and `TMP` to directories under `.build-tools` in the current process and prepend
`.build-tools/toolchain/bin` to that process's PATH. Existing Windows linker/SDK tools are still needed.
