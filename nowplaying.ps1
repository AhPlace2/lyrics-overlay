# Reports what Windows' media controls say is playing, as one JSON object per line.
# Runs under Windows PowerShell 5.1, which can call the WinRT media APIs directly.
# Exits on its own once the overlay process (-ParentPid) is gone, even if the overlay crashed.
param([int]$ParentPid = 0)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]
$null = [Windows.Storage.Streams.IRandomAccessStreamWithContentType, Windows.Storage.Streams, ContentType = WindowsRuntime]
$null = [Windows.Storage.Streams.IInputStream, Windows.Storage.Streams, ContentType = WindowsRuntime]
# PowerShell can't pick these methods for a raw WinRT stream on its own, so call them through reflection.
$asStreamForRead = [System.IO.WindowsRuntimeStreamExtensions].GetMethod('AsStreamForRead', [Type[]]@([Windows.Storage.Streams.IInputStream]))
$contentTypeOf = [Windows.Storage.Streams.IContentTypeProvider].GetProperty('ContentType')

$asTask = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
  $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
} | Select-Object -First 1

function Await($operation, [Type]$resultType) {
  $task = $asTask.MakeGenericMethod($resultType).Invoke($null, @($operation))
  if (-not $task.Wait(5000)) { throw 'Timed out waiting for Windows media info' }
  $task.Result
}

function Get-Cover($props) {
  if (-not $props.Thumbnail) { return $null }
  $stream = Await ($props.Thumbnail.OpenReadAsync()) ([Windows.Storage.Streams.IRandomAccessStreamWithContentType])
  $reader = $asStreamForRead.Invoke($null, @($stream))
  $memory = New-Object System.IO.MemoryStream
  $reader.CopyTo($memory)
  $reader.Dispose()
  if ($memory.Length -eq 0) { return $null }
  $type = $null
  try { $type = $contentTypeOf.GetValue($stream) } catch { }
  if (-not $type) { $type = 'image/jpeg' }
  "data:$type;base64," + [Convert]::ToBase64String($memory.ToArray())
}

function Emit($object) {
  [Console]::Out.WriteLine(($object | ConvertTo-Json -Compress))
  [Console]::Out.Flush()
}

$manager = Await ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager]::RequestAsync()) ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager])
$lastLine = $null
$coverKey = $null
$coverTick = 0
$sentCover = $null
$current = $null      # the session last reported, which commands act on
$currentKey = $null
$currentApp = $null

# Commands from the overlay arrive on stdin, one JSON object per line:
# {"cmd":"toggle"|"next"|"prev"|"seek","key":"<song key>","to":<seconds into the song, for seek>}
# [Console]::In would block here (its ReadLineAsync isn't really async), so read the raw input stream.
$stdin = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), [System.Text.Encoding]::UTF8)
$pending = $stdin.ReadLineAsync()

function Invoke-Control($line) {
  $msg = $line | ConvertFrom-Json
  $ok = $false
  if ($current -and $msg.key -eq $currentKey) {
    switch ($msg.cmd) {
      'toggle' {
        # Some players only offer play and pause separately, so ask for the one that is needed.
        $playingNow = $current.GetPlaybackInfo().PlaybackStatus.ToString() -eq 'Playing'
        $ok = if ($playingNow) { Await ($current.TryPauseAsync()) ([bool]) } else { Await ($current.TryPlayAsync()) ([bool]) }
        if (-not $ok) { $ok = Await ($current.TryTogglePlayPauseAsync()) ([bool]) }
      }
      'next' { $ok = Await ($current.TrySkipNextAsync()) ([bool]) }
      'prev' { $ok = Await ($current.TrySkipPreviousAsync()) ([bool]) }
      'seek' {
        $start = $current.GetTimelineProperties().StartTime.TotalSeconds
        $ticks = [long](([double]$msg.to + $start) * 10000000)
        $ok = Await ($current.TryChangePlaybackPositionAsync($ticks)) ([bool])
      }
    }
  }
  Emit ([ordered]@{ type = 'control'; cmd = $msg.cmd; ok = [bool]$ok })
}

$tick = 0
while ($true) {
  $tick++
  if ($ParentPid -and ($tick % 8 -eq 0) -and -not (Get-Process -Id $ParentPid -ErrorAction SilentlyContinue)) { exit 0 }
  while ($pending -and $pending.IsCompleted) {
    $line = $null
    try { $line = $pending.Result } catch { }
    if ($null -eq $line) { $pending = $null; break } # the overlay closed our input
    $pending = $stdin.ReadLineAsync()
    try { Invoke-Control $line } catch { [Console]::Error.WriteLine("Control failed: $($_.Exception.Message)") }
  }
  try {
    $session = $null
    $sessions = @($manager.GetSessions())
    foreach ($candidate in $sessions) {
      if ($candidate.GetPlaybackInfo().PlaybackStatus.ToString() -eq 'Playing') { $session = $candidate; break }
    }
    # Nothing playing (say, just paused from the card): stay with the same player rather than jumping to another.
    if (-not $session -and $currentApp) {
      foreach ($candidate in $sessions) { if ($candidate.SourceAppUserModelId -eq $currentApp) { $session = $candidate; break } }
    }
    if (-not $session) { $session = $manager.GetCurrentSession() }

    if (-not $session) {
      $state = [ordered]@{ type = 'state'; active = $false }
      $current = $null; $currentKey = $null
    } else {
      $props = Await ($session.TryGetMediaPropertiesAsync()) ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionMediaProperties])
      $timeline = $session.GetTimelineProperties()
      $playback = $session.GetPlaybackInfo()
      $key = "$($session.SourceAppUserModelId)|$($props.Artist)|$($props.Title)"
      $current = $session; $currentKey = $key; $currentApp = $session.SourceAppUserModelId
      $c = $playback.Controls
      $state = [ordered]@{
        type     = 'state'
        active   = $true
        key      = $key
        app      = $session.SourceAppUserModelId
        title    = $props.Title
        artist   = $props.Artist
        album    = $props.AlbumTitle
        status   = $playback.PlaybackStatus.ToString()
        position = $timeline.Position.TotalSeconds
        start    = $timeline.StartTime.TotalSeconds
        end      = $timeline.EndTime.TotalSeconds
        updated  = $timeline.LastUpdatedTime.ToUnixTimeMilliseconds()
        # What this player lets others do; the card greys out the rest.
        controls = [ordered]@{
          toggle = [bool]($c.IsPlayPauseToggleEnabled -or $c.IsPlayEnabled -or $c.IsPauseEnabled)
          next   = [bool]$c.IsNextEnabled
          prev   = [bool]$c.IsPreviousEnabled
          seek   = [bool]$c.IsPlaybackPositionEnabled
        }
      }
      if ($key -ne $coverKey) { $coverKey = $key; $coverTick = 0; $sentCover = $null }
      $coverTick++
      # Players often show their own icon (Chrome's logo, say) before the real cover, so keep checking:
      # every second for the first 20 seconds of a song, then every 5 seconds.
      if (($coverTick -le 80 -and $coverTick % 4 -eq 1) -or $coverTick % 20 -eq 1) {
        $cover = $null
        try { $cover = Get-Cover $props } catch { }
        if ($cover -and $cover -ne $sentCover) { Emit ([ordered]@{ type = 'cover'; key = $key; data = $cover }); $sentCover = $cover }
      }
    }

    $line = $state | ConvertTo-Json -Compress
    if ($line -ne $lastLine) {
      [Console]::Out.WriteLine($line)
      [Console]::Out.Flush()
      $lastLine = $line
    }
  } catch {
    [Console]::Error.WriteLine($_.Exception.Message)
  }
  # Wait for the next check, but wake straight away when a command comes in.
  if ($pending) { [void]$pending.Wait(250) } else { Start-Sleep -Milliseconds 250 }
}
