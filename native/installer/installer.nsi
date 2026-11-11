; Lyrics Overlay installer. Build it with installer\build.ps1, which passes VERSION, EXE and OUTFILE.
;
; Installs for the current user only, into %LOCALAPPDATA%\Programs\Lyrics Overlay, so no admin rights are needed and
; the app's self-updater can swap its own exe there. Settings (%APPDATA%\lyrics-overlay) are left alone, both on
; install and on uninstall. An older Electron install (0.1.x) is removed first.

Unicode true
!include MUI2.nsh
!include LogicLib.nsh
!include FileFunc.nsh

!ifndef VERSION | EXE | OUTFILE
  !error "Run installer\build.ps1, or pass /DVERSION=x.y.z /DEXE=path\to\lyrics-overlay.exe /DOUTFILE=path"
!endif

!define NAME "Lyrics Overlay"
!define APP_EXE "lyrics-overlay.exe"
!define PUBLISHER "AhPlace2"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\LyricsOverlay"

Name "${NAME}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\Lyrics Overlay"
InstallDirRegKey HKCU "${UNINST_KEY}" "InstallLocation"
RequestExecutionLevel user
SetCompressor /SOLID lzma
BrandingText "${NAME} ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${NAME}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "${NAME} installer"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "AGPL-3.0-or-later"

!define MUI_ICON "lyrics-overlay.ico"
!define MUI_UNICON "lyrics-overlay.ico"
!define MUI_ABORTWARNING
!define MUI_COMPONENTSPAGE_NODESC
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Start Lyrics Overlay"

!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; The app can't be replaced or deleted while it runs, and it has no quit message, so end it.
!macro CloseApp
  nsExec::Exec 'taskkill /F /IM "${APP_EXE}"'
  Pop $0
  Sleep 300
!macroend

; The Electron build (0.1.x) registered itself under a GUID key named "Lyrics Overlay <version>". Run its own
; uninstaller quietly, so the two don't run side by side. Its settings folder is the same one this build uses.
Function RemoveElectron
  StrCpy $R0 0
  ${Do}
    EnumRegKey $R1 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall" $R0
    ${If} $R1 == ""
      ${Break}
    ${EndIf}
    IntOp $R0 $R0 + 1
    ${If} $R1 == "LyricsOverlay"
      ${Continue}
    ${EndIf}
    ReadRegStr $R2 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\$R1" "DisplayName"
    StrCpy $R3 $R2 15
    ReadRegStr $R4 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\$R1" "QuietUninstallString"
    ${If} $R3 == "Lyrics Overlay "
    ${AndIf} $R4 != ""
      DetailPrint "Removing the old version ($R2)"
      nsExec::Exec 'taskkill /F /IM "Lyrics Overlay.exe"'
      Pop $0
      ExecWait '$R4'
      ; Its uninstaller hands over to a copy of itself and returns at once. That copy deletes the install folder,
      ; which may be this one, so wait until it is done (its registry key goes last), up to 30 seconds.
      StrCpy $R5 0
      ${Do}
        ClearErrors
        ReadRegStr $R2 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\$R1" "DisplayName"
        ${If} ${Errors}
          ${Break}
        ${EndIf}
        Sleep 250
        IntOp $R5 $R5 + 1
      ${LoopUntil} $R5 >= 120
      Sleep 500
      ${Break}
    ${EndIf}
  ${Loop}
FunctionEnd

Section "Lyrics Overlay" SecApp
  SectionIn RO
  Call RemoveElectron
  !insertmacro CloseApp
  SetOutPath "$INSTDIR"
  ; Leftovers from the self-updater.
  Delete "$INSTDIR\lyrics-overlay.old.exe"
  Delete "$INSTDIR\lyrics-overlay.new.exe"
  File "/oname=${APP_EXE}" "${EXE}"
  File "lyrics-overlay.ico"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortCut "$SMPROGRAMS\${NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\lyrics-overlay.ico"

  ; Settings > Apps.
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${NAME}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\lyrics-overlay.ico"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "https://github.com/AhPlace2/lyrics-overlay"
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" $0
SectionEnd

Section "Desktop shortcut" SecDesktop
  CreateShortCut "$DESKTOP\${NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\lyrics-overlay.ico"
SectionEnd

Section "Uninstall"
  !insertmacro CloseApp
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\lyrics-overlay.old.exe"
  Delete "$INSTDIR\lyrics-overlay.new.exe"
  Delete "$INSTDIR\lyrics-overlay.ico"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${NAME}.lnk"
  Delete "$DESKTOP\${NAME}.lnk"
  DeleteRegKey HKCU "${UNINST_KEY}"
SectionEnd
