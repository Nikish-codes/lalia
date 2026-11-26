; Lalia installer for Windows (NSIS 3, Modern UI).
;
; Installs for the current user only (no administrator prompt):
;   %LOCALAPPDATA%\Programs\Lalia, a Start menu shortcut, an entry in
;   "Installed apps", and optionally start-on-login and a desktop shortcut.
;
; Build (from this folder):
;   makensis -DVERSION=0.4.0 -DEXE=../../target/release/lalia.exe installer.nsi

Unicode true
!include "MUI2.nsh"
!include "FileFunc.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef EXE
  !define EXE "../../target/release/lalia.exe"
!endif
!ifndef OUTFILE
  !define OUTFILE "lalia-${VERSION}-windows-x64-setup.exe"
!endif

!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lalia"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"

Name "Lalia"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\Lalia"
InstallDirRegKey HKCU "Software\Lalia" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
ManifestDPIAware true
BrandingText "Lalia ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "Lalia"
VIAddVersionKey "FileDescription" "Lalia installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "MIT License"

!define MUI_ICON "lalia.ico"
!define MUI_UNICON "lalia.ico"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "Welcome to Lalia"
!define MUI_WELCOMEPAGE_TEXT "Lalia types what you say into any app: hold F12, speak, let go.$\r$\n$\r$\nIt lives in the notification area next to the clock. The first time it starts, a short setup asks for your Gemini API key and checks your microphone.$\r$\n$\r$\nThis installs Lalia for you only; no administrator rights are needed."
!define MUI_FINISHPAGE_RUN "$INSTDIR\lalia.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start Lalia now"
!define MUI_FINISHPAGE_LINK "Lalia on GitHub"
!define MUI_FINISHPAGE_LINK_LOCATION "https://github.com/Nikish-codes/lalia"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; Ask a running Lalia to quit so its files can be replaced or removed.
!macro StopLalia
  IfFileExists "$INSTDIR\lalia.exe" 0 +3
    nsExec::Exec '"$INSTDIR\lalia.exe" quit'
    Sleep 1000
!macroend

Section "Lalia" SecMain
  SectionIn RO
  !insertmacro StopLalia
  SetOutPath "$INSTDIR"
  File "/oname=lalia.exe" "${EXE}"
  File "/oname=README.md" "../../README.md"
  File "/oname=LICENSE.txt" "../../LICENSE"
  File "/oname=Inter-LICENSE.txt" "../../assets/fonts/Inter-LICENSE.txt"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\Lalia.lnk" "$INSTDIR\lalia.exe"

  WriteRegStr HKCU "Software\Lalia" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "Lalia"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "Lalia contributors"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\lalia.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "URLInfoAbout" "https://github.com/Nikish-codes/lalia"
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" "$0"
SectionEnd

Section "Start Lalia when I sign in" SecAutostart
  WriteRegStr HKCU "${RUN_KEY}" "Lalia" '"$INSTDIR\lalia.exe"'
SectionEnd

Section /o "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\Lalia.lnk" "$INSTDIR\lalia.exe"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "The Lalia program and its Start menu shortcut."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecAutostart} "Lalia waits quietly in the notification area after you sign in. You can change this later in Lalia's settings."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "A shortcut on the desktop that opens Lalia."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  !insertmacro StopLalia
  Delete "$INSTDIR\lalia.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\Inter-LICENSE.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Lalia.lnk"
  Delete "$DESKTOP\Lalia.lnk"
  DeleteRegValue HKCU "${RUN_KEY}" "Lalia"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegKey HKCU "Software\Lalia"
  ; Settings, the API key and history in %APPDATA%\lalia and
  ; %LOCALAPPDATA%\lalia are the user's: they stay.
SectionEnd
