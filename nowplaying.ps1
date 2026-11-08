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

$tick = 0
while ($true) {
  $tick++
  if ($ParentPid -and ($tick % 8 -eq 0) -and -not (Get-Process -Id $ParentPid -ErrorAction SilentlyContinue)) { exit 0 }
  try {
    $session = $null
    foreach ($candidate in $manager.GetSessions()) {
      if ($candidate.GetPlaybackInfo().PlaybackStatus.ToString() -eq 'Playing') { $session = $candidate; break }
    }
    if (-not $session) { $session = $manager.GetCurrentSession() }

    if (-not $session) {
      $state = [ordered]@{ type = 'state'; active = $false }
    } else {
      $props = Await ($session.TryGetMediaPropertiesAsync()) ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionMediaProperties])
      $timeline = $session.GetTimelineProperties()
      $playback = $session.GetPlaybackInfo()
      $key = "$($session.SourceAppUserModelId)|$($props.Artist)|$($props.Title)"
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
  Start-Sleep -Milliseconds 250
}
