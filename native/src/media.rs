// Reports what Windows' media controls say is playing, and passes the card's playback controls to the player.
// Port of src/main/nowplaying.ps1, run as a thread instead of a PowerShell process.

use serde_json::{json, Value};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::OnceLock;
use std::time::Duration;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionMediaProperties as Props, GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;

static COMMANDS: OnceLock<Sender<Value>> = OnceLock::new();

/// A command for the player: {"cmd":"toggle"|"next"|"prev"|"seek","key":"<song key>","to":<seconds, for seek>}.
pub fn command(msg: Value) -> bool {
    COMMANDS.get().map(|tx| tx.send(msg).is_ok()).unwrap_or(false)
}

pub fn start() {
    let (tx, rx) = channel();
    let _ = COMMANDS.set(tx);
    std::thread::Builder::new()
        .name("now-playing".into())
        .spawn(move || {
            unsafe {
                let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
            }
            loop {
                if let Err(e) = run(&rx) {
                    eprintln!("[now playing] {e}, restarting");
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        })
        .expect("now-playing thread");
}

fn status_name(s: Status) -> &'static str {
    match s {
        Status::Closed => "Closed",
        Status::Opened => "Opened",
        Status::Changing => "Changing",
        Status::Stopped => "Stopped",
        Status::Playing => "Playing",
        Status::Paused => "Paused",
        _ => "Closed",
    }
}

fn is_playing(s: &Session) -> bool {
    s.GetPlaybackInfo().and_then(|p| p.PlaybackStatus()).map(|st| st == Status::Playing).unwrap_or(false)
}

fn cover_of(props: &Props) -> windows::core::Result<Option<String>> {
    let Ok(thumb) = props.Thumbnail() else { return Ok(None) };
    let stream = thumb.OpenReadAsync()?.join()?;
    let size = stream.Size()? as u32;
    if size == 0 {
        return Ok(None);
    }
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
    reader.LoadAsync(size)?.join()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes)?;
    let kind = stream.ContentType().map(|s| s.to_string()).unwrap_or_default();
    let kind = if kind.is_empty() { "image/jpeg".to_string() } else { kind };
    use base64::Engine;
    Ok(Some(format!("data:{kind};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))))
}

fn secs(t: windows::Foundation::TimeSpan) -> f64 {
    t.Duration as f64 / 1e7
}

fn run(rx: &Receiver<Value>) -> windows::core::Result<()> {
    let manager = Manager::RequestAsync()?.join()?;
    let mut last_line = String::new();
    let (mut cover_key, mut cover_tick, mut sent_cover): (Option<String>, u32, Option<String>) = (None, 0, None);
    // The session last reported, which commands act on.
    let (mut current, mut current_key, mut current_app): (Option<Session>, Option<String>, Option<String>) = (None, None, None);

    loop {
        // Wait for the next check, but wake straight away when a command comes in.
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(msg) => {
                let cmd = msg.get("cmd").and_then(Value::as_str).unwrap_or("").to_string();
                let mut ok = false;
                if let (Some(s), Some(k)) = (&current, &current_key) {
                    if msg.get("key").and_then(Value::as_str) == Some(k.as_str()) {
                        ok = control(s, &cmd, &msg).unwrap_or(false);
                    }
                }
                crate::core::on_media(json!({ "type": "control", "cmd": cmd, "ok": ok }));
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }

        let sessions: Vec<Session> = manager.GetSessions()?.into_iter().collect();
        let mut session = sessions.iter().find(|s| is_playing(s)).cloned();
        // Nothing playing (say, just paused from the card): stay with the same player rather than jumping to another.
        if session.is_none() {
            if let Some(app) = &current_app {
                session = sessions.iter().find(|s| s.SourceAppUserModelId().map(|a| a.to_string()).as_ref() == Ok(app)).cloned();
            }
        }
        if session.is_none() {
            session = manager.GetCurrentSession().ok();
        }

        let state = match &session {
            None => {
                current = None;
                current_key = None;
                json!({ "type": "state", "active": false })
            }
            Some(s) => {
                let props = s.TryGetMediaPropertiesAsync()?.join()?;
                let timeline = s.GetTimelineProperties()?;
                let playback = s.GetPlaybackInfo()?;
                let app = s.SourceAppUserModelId()?.to_string();
                let (title, artist) = (props.Title()?.to_string(), props.Artist()?.to_string());
                let key = format!("{app}|{artist}|{title}");
                current = Some(s.clone());
                current_key = Some(key.clone());
                current_app = Some(app.clone());
                let c = playback.Controls()?;
                let flag = |r: windows::core::Result<bool>| r.unwrap_or(false);
                // LastUpdatedTime counts 100 ns ticks since 1601; the page wants Unix milliseconds.
                let updated = (timeline.LastUpdatedTime()?.UniversalTime - 116_444_736_000_000_000) / 10_000;
                let state = json!({
                    "type": "state",
                    "active": true,
                    "key": key,
                    "app": app,
                    "title": title,
                    "artist": artist,
                    "album": props.AlbumTitle()?.to_string(),
                    "status": status_name(playback.PlaybackStatus()?),
                    "position": secs(timeline.Position()?),
                    "start": secs(timeline.StartTime()?),
                    "end": secs(timeline.EndTime()?),
                    "updated": updated,
                    // What this player lets others do; the card greys out the rest.
                    "controls": {
                        "toggle": flag(c.IsPlayPauseToggleEnabled()) || flag(c.IsPlayEnabled()) || flag(c.IsPauseEnabled()),
                        "next": flag(c.IsNextEnabled()),
                        "prev": flag(c.IsPreviousEnabled()),
                        "seek": flag(c.IsPlaybackPositionEnabled()),
                    },
                });
                if cover_key.as_deref() != Some(key.as_str()) {
                    cover_key = Some(key.clone());
                    cover_tick = 0;
                    sent_cover = None;
                }
                cover_tick += 1;
                // Players often show their own icon (Chrome's logo, say) before the real cover, so keep checking:
                // every second for the first 20 seconds of a song, then every 5 seconds.
                if (cover_tick <= 80 && cover_tick % 4 == 1) || cover_tick % 20 == 1 {
                    if let Ok(Some(cover)) = cover_of(&props) {
                        if sent_cover.as_ref() != Some(&cover) {
                            crate::core::on_media(json!({ "type": "cover", "key": key, "data": cover }));
                            sent_cover = Some(cover);
                        }
                    }
                }
                state
            }
        };
        let line = state.to_string();
        if line != last_line {
            crate::core::on_media(state);
            last_line = line;
        }
    }
}

fn control(s: &Session, cmd: &str, msg: &Value) -> windows::core::Result<bool> {
    Ok(match cmd {
        "toggle" => {
            // Some players only offer play and pause separately, so ask for the one that is needed.
            let ok = if is_playing(s) { s.TryPauseAsync()?.join()? } else { s.TryPlayAsync()?.join()? };
            ok || s.TryTogglePlayPauseAsync()?.join()?
        }
        "next" => s.TrySkipNextAsync()?.join()?,
        "prev" => s.TrySkipPreviousAsync()?.join()?,
        "seek" => {
            let start = secs(s.GetTimelineProperties()?.StartTime()?);
            let to = msg.get("to").and_then(Value::as_f64).unwrap_or(0.0);
            s.TryChangePlaybackPositionAsync(((to + start) * 1e7) as i64)?.join()?
        }
        _ => false,
    })
}
