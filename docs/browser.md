# Cabin browser

Open **Browser** from the cabin Home screen. Enter an HTTP/HTTPS address (a missing scheme defaults to
HTTPS), then press Go. Back, Forward, Reload/Stop and 50–200% zoom are available. The last address is
remembered; cookies and website storage live in a separate `site-browser` profile in WestPlay's data folder.
Restart recreates the control without clearing that profile. Leaving the app hides the site while keeping
the page and its audio running in the background. Pause playback on the website to stop its sound.

## Fixed web apps

The existing **Yandex Music** Home tile opens `https://music.yandex.ru/` in its own native browser.
Its address field is read-only; **Start page** returns to the configured URL. Links, redirects and sign-in
pages can navigate to other domains. Back, Forward, Reload/Stop, zoom and Restart work as in Browser.
Switching apps preserves each page and background audio. Restart affects only the selected app.

The fixed list is in `ui/src/shared/webApps.ts`; native identities/profile directories are in
`crates/host/src/site_browser.rs` (`BrowserId`). There is no runtime editor. The existing Home tile and icon
are reused. Each opened app gets its own WebView and persistent profile, created lazily; Yandex Music uses
`site-yandex-music`, separate from the ordinary browser's `site-browser`. The previous iframe and its
frame-ancestor exception have been removed. The old iframe's login is not migrated: sign in once in the new
browser. Rebuild both the UI and host together because browser messages now include the instance ID.

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

## Adding another site

Use one stable camelCase ID everywhere, for example `exampleRadio`:

1. Add `exampleRadio: { url: 'https://example.org/', name: 'Example Radio' }` to `WEB_APPS` in
   `ui/src/shared/webApps.ts`. This defines the start page and browser accessibility label, not the Home icon.
2. In `ui/src/cabin/state/apps.ts`, add `'exampleRadio'` to `APP_IDS` at the desired Home position and
   `exampleRadio: { id: 'exampleRadio', icon: 'browser' }` to `APPS`. Add its Home name to each language's
   `apps` dictionary in `ui/src/cabin/state/strings.ts`.
3. To use a custom icon instead of `'browser'`, add a role and filename mapping to `AppIconRole` and `FILES`
   in `ui/src/shared/icons.ts`, then add matching `<filename>.light.svg` and `<filename>.dark.svg` files
   under `ui/public/icons/westplay/`. Set the `APPS` entry's `icon` to that role. `WEB_APPS` does not load icons.
4. In `crates/host/src/site_browser.rs`, add `ExampleRadio` to `BrowserId` (Serde serializes it as
   `exampleRadio`) and `Self::ExampleRadio => "site-example-radio"` to `profile()`. Use a unique, stable
   profile directory so the site's cookies stay separate and survive restarts.
5. Update the expected Home order in `ui/src/cabin/state/apps.test.ts` and any icon tests affected by a new
   role. Run the frontend build/tests and native checks below, then rebuild the UI and host together.

`AppScreen` automatically opens registered `WEB_APPS` entries through `BrowserApp`; no new component or
switch branch is needed. The new site inherits the read-only address field, navigation controls and
background playback. Adding only a `WEB_APPS` entry does not create a Home tile or native browser identity.

## Development checks

`npm run build --prefix ui` and `npm test --prefix ui` check the frontend.
`cargo test -p westplay-host -p westplay-plugin -p westplay-shared` checks the native modules.
`cargo test -p westplay-host site_browser::tests::native_navigation_and_input -- --ignored` exercises a real
WebView2 against a local HTTP fixture, including password editing, cross-origin input, cookies, history and
continued, unmuted media playback when the browser is hidden and shown again.
With `WESTPLAY_UI` pointing to the absolute `ui/dist` directory, the ignored `built_browser_controls` test
checks the built React page and writes `browser-preview.png` into the process's temporary directory.

`tools/dev/portable-rust.ps1` optionally downloads checksum-verified official Rust archives into `.build-tools`.
It does not run an installer, change persistent PATH or register Rust. For an isolated build, set `CARGO_HOME`,
`CARGO_TARGET_DIR`, `TEMP` and `TMP` to directories under `.build-tools` in the current process and prepend
`.build-tools/toolchain/bin` to that process's PATH. Existing Windows linker/SDK tools are still needed.
