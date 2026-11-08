@echo off
rem Starts the lyrics overlay without leaving a console window open.
cd /d "%~dp0"
if not exist "node_modules\electron\dist\electron.exe" (
  echo Installing Electron the first time, this takes a minute...
  call npm install
)
rem npm sometimes skips Electron's own download step, so run it directly if the app is still missing.
if not exist "node_modules\electron\dist\electron.exe" call node "node_modules\electron\install.js"
start "" "node_modules\electron\dist\electron.exe" .
