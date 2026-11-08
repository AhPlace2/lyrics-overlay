# Lyrics Overlay

Synced lyrics on top of your Windows desktop for whatever is playing: Spotify, YouTube in Chrome, Firefox or Brave, and other players that show up in Windows' media controls.

- A now playing card with the cover, title and progress, plus the current lyric lines
- Big floating lyrics that type in word by word, styled by mixing a look (Depth, Glow, Outline, Plain), a colour or rainbow, and a motion (Wave, Bounce, Pulse)
- Lyrics from [LRCLIB](https://lrclib.net), with other versions from LRCLIB and NetEase when the timing is off
- Per-song timing fixes and lyric choices, saved for next time
- Lyrics found once are saved on disk, so a song loads instantly next time, even offline

## Run it

Needs Windows 10 or 11 and [Node.js](https://nodejs.org).

Double-click `Start Lyrics Overlay.cmd`. The first start installs Electron, which takes a minute.

Or from a terminal:

```
npm install
npm start
```

## Use it

Click the gear on the now playing card for settings, or right-click the tray icon.

| Shortcut | Does |
|---|---|
| Ctrl+Alt+L | Show or hide lyrics |
| Ctrl+Alt+S | Sync: press when the line on screen is being sung |
| Ctrl+Alt+Up / Down | Lyrics earlier / later by 0.25 s |
| Ctrl+Alt+O / P | Next / previous lyrics version |
| Ctrl+Alt+M | Move the card |

Settings are saved in `%APPDATA%\lyrics-overlay\settings.json`, and found lyrics in `%APPDATA%\lyrics-overlay\lyrics-cache`. "Search again" in the settings fetches fresh lyrics for the current song.

## How it works

- `nowplaying.ps1` reads Windows' media controls (title, artist, position, cover) and reports changes to the app.
- `lyrics.js` finds timed lyrics; `cover.js` looks up a cover on iTunes when the player shares none.
- `main.js` runs the click-through overlay window, tray icon, shortcuts and settings panel.
- `overlay.*` draws the card and big lyrics; `settings.*` is the settings panel.
