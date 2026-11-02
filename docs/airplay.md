# AirPlay (first version)

Place the **complete** UxPlay Windows runtime next to the WestPlay application executable:

```text
WestPlay.exe
uxplay-windows/
  uxplay-windows.exe
  ...DLLs and other files from its distribution...
  lib/gstreamer-1.0/
  libexec/gstreamer-1.0/
```

The standalone development host resolves the folder beside `westplay-host.exe` instead.
Nothing is downloaded or installed automatically. Keep the runtime's original directory structure.
If `uxplay.exe` is present, WestPlay uses that CLI engine instead of the GUI executable.

The engine must support `-vrtp` (upstream UxPlay 1.73+) and include `rtph264pay`, `rtph264depay`,
`h264parse`, `mpegtsmux`, and `tcpserversink`. Some Windows distributions prune plugins: a working
desktop mirroring window alone does not establish that MPEG-TS output is available.
GUI builds must support the documented `%ProgramData%/uxplay-windows/arguments.txt` configuration.
Older GUI builds without that override need an updated build or `uxplay.exe`.
See https://github.com/leapbtw/uxplay-windows and https://github.com/FDH2/UxPlay.

Close any independently running uxplay-windows first. Open **AirPlay** on the cabin Home screen and
press **Start**. On an iPhone connected to the same LAN, choose Control Center → Screen Mirroring →
**WestPlay**. Discovery and Windows Firewall requirements are the same as standalone UxPlay;
its Bonjour/mDNS setup must work. Its tray icon may remain visible while running.

This version mirrors **H.264 video only** into WebView2. UxPlay plays audio through the normal Windows
output; adjust its volume on the iPhone. Video buffering can introduce an offset relative to that audio.
WestPlay's equalizer, cabin spatial audio, volume shortcuts and automatic pause-with-game do not control
this external audio. The four built-in music players pause when Start is pressed.
Leaving the AirPlay app, pressing Stop, reloading the cabin page, exiting the game or closing WestPlay
stops its owned receiver. Switching to the truck's original display does not leave the app or stop audio.
The iPhone remains the controller; clicks on the mirrored picture do not control iOS.
DRM video and AirPlay HLS are outside this first version.

UxPlay's GStreamer pipeline packs and unpacks RTP **inside its own process**, muxes H.264 into MPEG-TS
without decoding or encoding video, and sends it to a loopback TCP port. A Rust worker relays those bytes
over loopback HTTP to `mpegts.js` in the existing cabin page. No WebRTC, extra GStreamer process, or native
texture transport is added. HTTP requests require a per-session unguessable path; CORS allows the cabin
origin and localhost development pages. Ports are selected dynamically. The browser retries when a stream
ends or stalls; reconnecting a phone does not restart the entire WestPlay host.

WestPlay writes its configuration and `receiver.log` below its data directory's `airplay` folder.
For the GUI, it sets `ProgramData` **only on its child process** to that folder, so the GUI reads a private
`uxplay-windows/arguments.txt`. It also supplies a private `UXPLAYRC`. Existing UxPlay configuration files
are not overwritten. The GUI may also write its own session log in its usual user-data location.

## Checks

Automated: host AirPlay tests exercise configuration, origin filtering and a real TCP/HTTP byte relay;
the frontend build checks the player API and app registration. The ignored native test
`airplay::tests::built_airplay_video_and_controls` loads the built page in WebView2, decodes a test
MPEG-TS through the real relay, checks Start/Stop/Home and captures preview PNGs in the temp directory.
Set `WESTPLAY_UI` to the built `ui/dist` directory and `WESTPLAY_AIRPLAY_FIXTURE` to a short H.264-only
MPEG-TS file (320×180), then run:

```text
cargo test -p westplay-host built_airplay_video_and_controls -- --ignored
```

For device acceptance, check:

- Portrait/landscape mirroring, video colors/aspect ratio and actual sound/video delay.
- Phone disconnect/reconnect, Wi-Fi interruption, Stop/Start and returning to Home.
- Missing executable/plugins, an already running GUI, game exit and application exit.
- A sustained session (bounded browser buffers), and changing trucks/GPU.

These hardware checks require the user's runtime and an iOS device on the LAN.
