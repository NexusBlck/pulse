# Synapse

A living island at the top of your screen, for Windows 10/11. Native Rust (Win32 + Direct2D), about 640 KB, and it uses no CPU at rest. You control everything through one JSON file.

## What it does

- **Rests** as the time, **two eyes** (they blink, glance about, and watch your pointer when it's on them), a small dot, or nothing at all (a strip you can find with the pointer).
- **Music**: when anything plays (Spotify, browsers, Media Player...) it becomes a pill with the album art and a dancing equalizer, and shows the new track's title for a few seconds when the song changes.
- **Synced lyrics** from [LRCLIB](https://lrclib.net): the closed pill sings along, growing and shrinking to fit each line, and the Now Playing page scrolls through the lyrics. Each song is looked up once and cached in `%LOCALAPPDATA%\Synapse\lyrics`.
- **Pop-ups**:
  - **Volume** when you change it.
  - **Charging** when you plug in, and **Low battery** warnings at 20% and 10%.
  - **Bluetooth** when a paired device (earbuds, headphones, a mouse...) connects or disconnects, with its name and its battery when Windows knows it.
  - **Unlocked**: a padlock springs open when you unlock Windows (Hello, PIN or password).
  - **Mute**: muting and unmuting get their own pop-up, like a phone's silent switch.
  - **Do Not Disturb**: when Windows' Do Not Disturb turns on or off.
  - **Phone drop**: files arriving from your phone show their progress and a tick on the closed island, like AirDrop.
  - **Privacy dots**: while an app uses the camera (green), microphone (orange) or location (blue), a small bubble beside the island shows the dot, and a pop-up names the app when it starts. Read from Windows' own usage records, which it signals on change, so nothing polls.
  - Charging is green above 20% and red below; the low-battery warnings throb.
  
  All of them come from Windows' own notifications; nothing polls.
- **Opens** when you hover over it (or click it) into pages:
  - **Now Playing**: art, title, artist, a progress bar you can click to seek, and previous / play-pause / next.
  - **Shelf**: drag files onto the island (it opens by itself) and they wait there, with thumbnails, until you drag them out to wherever you want them. Click one to open it; hover for ✕ to remove it. Up to 16 files; the list survives restarts (`%APPDATA%\Synapse\shelf.json`). It only keeps paths and never copies your files.
  - **Phone drop** (the Shelf's **From phone** tile): scan the QR code with your phone's camera, pick photos, PDFs or any files, and they arrive on the Shelf over Wi-Fi, saved in `Downloads\Synapse`. The phone and PC need to be on the same Wi-Fi. The QR holds a secret one-time link, so nobody else on the network can send you files. Nothing listens until you open the panel, and it stops when you press Done (or after 10 idle minutes). The first time, Windows asks to let Synapse through the firewall: allow **private** networks.
  - **Tasks**: a to-do list. Click the field and type (the island takes the keyboard only while you type; Esc or a click elsewhere gives it back), Enter adds. Tick to finish (done ones sink and get struck through), ✕ to delete, Clear done, and the wheel scrolls a long list. Kept in `%APPDATA%\Synapse\tasks.json`.
  - **Focus**: a Pomodoro timer. Focus sessions, short breaks, and a long break every few sessions, with a chime and a notice when each one ends. While it runs, the closed island shows the countdown; click it to open the timer.
  - **Clock**: big time, date, battery, and a line for the passing minute.
  - **Commands**: your own buttons that open anything (apps, folders, URLs, `ms-settings:` pages).
- **Add-ons** (the last tab; each stays off until you switch it on, then gets a tab of its own):
  - **System**: CPU, GPU, memory and network with a minute of history, the busiest apps, free disk space and uptime. Reads once a second, only while its page is open.
  - **Caffeine**: tap what you drink; see how much is still in you, the day's curve, what's left at bedtime, and the last call (the latest your usual drink still keeps bedtime under your limit). Kept in `%APPDATA%\Synapse\caffeine.json`.
  - **Prayer times**: worked out on the PC for where you are (Windows' location the first time), with the method and Asr rule your country follows unless you choose. Shows the next prayer, today's six times and the qibla, counts down on the closed island before each prayer, and chimes at its time.
  - **Downloader**: copy a link, then **Paste → Video** or **Paste → Audio**. Files go to your Downloads folder and onto the Shelf, with progress, Stop and Open, and a progress pill on the closed island. Uses [yt-dlp](https://github.com/yt-dlp/yt-dlp), which Synapse fetches from its official GitHub release when you switch the add-on on (about 18 MB), checks against the release's SHA-256, and deletes when you switch it off. YouTube needs a JavaScript runtime: Synapse uses Node if it's installed. With FFmpeg on the PC, video comes in up to 1080p; otherwise yt-dlp takes the best single file.
  - **Script reader**: a teleprompter in the island, right under your webcam. Paste your script or edit `%APPDATA%\Synapse\script.txt`; **Read** scrolls it (also **Ctrl+Alt+S** from anywhere), with speed and text size. While its page shows, screen shares and recordings leave the island out (switchable).
  - Coming next: Stocks, AI usage, Japanese readings.
- **Ambient light**: the island takes its colour and light from what you're looking at: the album art while music plays, otherwise what's on screen (sampled once a second as a tiny 64×32 picture, below the top strip so the glow never samples itself). You get a palette of two colours, left and right.
  - The island wears it: a tint in its body, a halo around it, and the accent.
  - **Glow on the screen edge**: light blooms out from the island and runs along the top of the screen in the palette's colours, through a click-through window that never gets in the way.
  - **Breathes with music**: the glow rises and falls with the loudness of what's playing.
  - **RGB keyboard**: with [OpenRGB](https://openrgb.org) running and its SDK server started (OpenRGB > SDK Server > Start Server), turn on *RGB keyboard (OpenRGB)* and every device OpenRGB controls glows in the same colour.
  - Screen sampling stops while you're locked away or a full-screen app hides the island; the glow only redraws when the colour changes or the music swells.
- **Themes**: the palette button on the open island shows every theme as a live preview. Click one to switch.
  - **Midnight**: the black notch.
  - **Capsule**: a floating black pill with a soft shadow.
  - **Glass**: real frosted glass. Windows itself blurs what's behind the island (a small window under it, shaped to its outline), with a light rim and sheen on top. Windows' compositor does the blur, so Synapse never copies the screen.
  - **Neon**: a black notch with a glowing edge in the album art's colour.
  - **Light**: a white pill with dark text.
  - **Aurora**: a floating pill with a purple-to-blue gradient.
  - **Pastel**: a soft pill running from candy pink into baby blue, with plum text and a lavender accent.
  - **Coffee**: espresso into mocha, with cream text and a caramel accent.
  
  Picking one writes its `style` and `colors` into your config, so you can fine-tune from there.
- **Hotkey** (default `Ctrl+Alt+P`) opens and pins it; press again to close. Clicking the tray icon does the same.
- Gets out of the way of full-screen games and videos.
- Springs drive all the motion: the outline stretches open with a little bounce, and the content cross-fades as it morphs.

## Run it

```bash
cargo build --release
```

Then run `target\release\synapse.exe`. Right-click the island or the tray icon for **Settings / Open config file / Reload config / Start with Windows / Quit**.

## Make it yours

**Settings page**: the gear on the open island. Theme, what it shows at rest, shape, the everyday switches and which tabs show, right in the island. **Advanced settings ↗** opens the window.

**Settings window**: Advanced settings on that page (or **Settings...** in the right-click menu). A sidebar of sections (General, Appearance, Island, Music & lyrics, Pop-ups, Focus timer, Tabs, About), and every option is a click: switches, pickers, steppers, colour swatches. Changes apply at once and save to the config. Drag it by any empty part; Esc closes it.

Everything also lives in one file, `%APPDATA%\Synapse\config.json` (your commands are edited there). Synapse creates it on first run and reloads it **as soon as you save it**. If you make a mistake, the island tells you which line it's on and keeps your last good settings.

| Key | What it does | Default |
|---|---|---|
| `theme` | `"midnight"`, `"capsule"`, `"glass"`, `"neon"`, `"light"`, `"aurora"`, `"pastel"`, `"coffee"`: the finish (shadow, glow, glass, gradient). Easiest to pick on the island | `"midnight"` |
| `style` | `"notch"` hangs from the top edge; `"pill"` floats below it | `"notch"` |
| `idle` | what shows at rest: `"clock"`, `"dot"`, `"hidden"` | `"clock"` |
| `clock_24h`, `show_seconds` | clock format | `false`, `false` |
| `font` | any installed font family | `"Segoe UI Variable Text"` |
| `width` | width of the open island (360 to 900) | `560` |
| `offset_x` | shift left (negative) or right of centre, in DIPs | `0` |
| `open_on_hover` | open when the pointer rests on it (otherwise click) | `true` |
| `hover_delay_ms` | how long the pointer must rest before it opens | `220` |
| `collapse_delay_ms` | grace before it closes after the pointer leaves | `0` |
| `hide_in_fullscreen` | hide over full-screen apps | `true` |
| `show_media`, `volume_hud`, `hud_seconds` | music pill and volume pop-up | `true`, `true`, `1.8` |
| `ambient.enabled` | ambient light on or off | `true` |
| `ambient.source` | `"auto"` (art while music plays, else screen), `"art"`, `"screen"` | `"auto"` |
| `ambient.edge_glow`, `tint_island`, `breathe` | the edge glow, the island's tint and halo, breathing with music | `true` |
| `ambient.intensity` | glow brightness, 0.1 to 1 | `0.7` |
| `ambient.openrgb`, `openrgb_port` | send the colour to OpenRGB's SDK server | `false`, `6742` |
| `addons.system`, `caffeine`, `prayer`, `downloader`, `script` | add-ons on or off | `false` |
| `script.speed`, `size`, `hotkey`, `hide_from_capture` | Script reader: scroll speed, text size, start/pause hotkey, leave it out of screen shares | `32`, `22`, `"Ctrl+Alt+S"`, `true` |
| `caffeine.bedtime_min`, `half_life_h`, `bedtime_limit_mg`, `drinks` | bedtime (minutes after midnight), half-life, what's fine at bedtime, the quick-add buttons | `1380`, `5`, `35`, six drinks |
| `prayer.method`, `prayer.asr` | `"auto"` or a method (`"karachi"`, `"mwl"`, `"isna"`, `"makkah"`...); `"auto"`, `"standard"`, `"hanafi"` | `"auto"` |
| `prayer.lat`, `lon`, `place` | where you are (filled in for you) | |
| `prayer.remind_min`, `prayer.sound` | countdown minutes on the island, chime at prayer time | `10`, `true` |
| `power_hud` | charging and low-battery pop-ups | `true` |
| `unlock_hud`, `mute_hud`, `dnd_hud` | the unlocked, mute and Do Not Disturb pop-ups | `true` |
| `privacy` | privacy dots and "X is using your camera" pop-ups | `true` |
| `bluetooth_hud` | Bluetooth connect / disconnect pop-ups | `true` |
| `lyrics` | synced lyrics on the Now Playing page | `true` |
| `lyrics_in_pill` | the line being sung in the closed pill | `true` |
| `lyrics_offset_ms` | lyrics running early or late? positive shows lines later, negative earlier | `0` |
| `hotkey` | e.g. `"Ctrl+Alt+P"`, `"Win+Shift+Space"`, `"Alt+F2"`; `""` turns it off | `"Ctrl+Alt+P"` |
| `editor` | program the gear button opens the config with | `"notepad.exe"` |
| `start_with_windows` | run at sign-in | `false` |
| `pages` | which pages, in order: `"media"`, `"shelf"`, `"focus"`, `"clock"`, `"commands"` | all five |
| `focus.focus_minutes`, `break_minutes`, `long_break_minutes` | timer lengths | `25`, `5`, `15` |
| `focus.long_break_every` | a long break after this many focus sessions | `4` |
| `focus.auto_start_breaks`, `auto_start_focus` | start the next phase by itself | `true`, `false` |
| `focus.sound` | chime when a phase ends | `true` |
| `colors.island / text / dim / accent / border` | `"#RRGGBB"` or `"#RRGGBBAA"` | black island, white text, green accent |
| `colors.accent_from_art` | take the accent colour from the album art | `true` |
| `motion.speed` | 1 = normal, 2 = twice as fast (0.3 to 3) | `1.0` |
| `motion.bounce` | 0 = no overshoot, 0.6 = very springy | `0.27` |
| `commands` | your buttons, see below | a few examples |

### Commands

```json
"commands": [
  { "label": "Terminal", "target": "wt.exe" },
  { "label": "Projects", "target": "C:\\Projects" },
  { "label": "YouTube", "target": "https://youtube.com" },
  { "label": "Bluetooth", "target": "ms-settings:bluetooth" },
  { "label": "VS Code here", "target": "code", "args": "\"C:\\Projects\\Synapse\"" }
]
```

`target` and `args` expand `%VARIABLES%` such as `%USERPROFILE%`.

### Example: a purple, bouncy, floating island

```json
{
  "style": "pill",
  "idle": "dot",
  "colors": { "accent": "#BF5AF2", "accent_from_art": false },
  "motion": { "speed": 1.2, "bounce": 0.45 }
}
```

Any key you leave out keeps its default.

## How it's built

| File | What |
|---|---|
| `src/app.rs` | the window, the state machine (rest / music / pop-up / notice / open), input, the frame loop and all drawing |
| `src/island.rs` | the outline's width, height and corners on springs; grow and shrink feel different |
| `src/spring.rs` | the damped spring behind every motion |
| `src/gfx.rs` | Direct2D + DirectWrite into a layered window (transparent pixels let clicks through) |
| `src/clock.rs` | the vblank frame clock; parked (zero CPU) unless something moves |
| `src/media.rs` | now playing via Windows' media controls, album art via WIC |
| `src/lyrics.rs` | LRCLIB lookups, the lyrics cache and the LRC parser |
| `src/shelf.rs` | the Shelf's list, dropping files in, dragging them out, thumbnails |
| `src/phone.rs` | Phone drop: the local web server, the phone's page, QR codes, safe file names |
| `src/focus.rs` | the Focus timer's phases and timing |
| `src/volume.rs` | volume change callbacks, following the default output device |
| `src/system.rs` | the System add-on's readings |
| `src/downloader.rs` | the Downloader: fetching and checking yt-dlp, running it, reading its progress |
| `src/caffeine.rs` | the Caffeine add-on: decay, the log, last call |
| `src/prayer.rs` | the Prayer add-on: the sun's maths, methods, cities, qibla, location |
| `src/ambient.rs` | the ambient palette: colours from album art and the screen sampler |
| `src/edge.rs` | the glow along the screen's top edge |
| `src/openrgb.rs` | OpenRGB SDK client: RGB devices in the island's colour |
| `src/privacy.rs` | which apps are using the camera, microphone or location |
| `src/bluetooth.rs` | paired Bluetooth devices connecting and disconnecting, and their battery |
| `src/watch.rs` | reloads the config the moment you save it |
| `src/sys.rs` | tray, hotkey, launching commands, start with Windows, clock, battery |
| `src/config.rs` | every setting and its default |
| `src/settings.rs` | the Settings window: its sections, controls and drawing |

Logs: `%LOCALAPPDATA%\Synapse\synapse.log`. Tests: `cargo test`.

## License

MIT. See [LICENSE](LICENSE).
