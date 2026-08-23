; The Windows installer.
;
; Built by the release workflow with `makensis`, alongside — not instead of —
; the portable `.exe`. The two are for different people: the portable one is
; one file you put where you like, and this one is for somebody who wants mbrd
; in the Start menu, `.mbrd` files to open when double-clicked, and an entry in
; Add/Remove Programs.
;
; ## Why it installs per-user
;
; `HKCU` and `$LOCALAPPDATA`, not `HKLM` and `Program Files`. Three reasons,
; and the third is the one that matters most here:
;
;   1. No UAC prompt, so installing is not an administrative act.
;   2. It works on a machine where somebody is not an administrator, which is
;      most managed laptops.
;   3. **The installed executable stays writable by the app itself**, which is
;      what lets the in-app updater replace it. A `Program Files` install would
;      put the binary somewhere the app cannot write, and every update would
;      fall back to "download it yourself" — see `update/eligible.rs`, which
;      would correctly refuse.
;
; ## What it does not do
;
; No bundled runtime, because `-C target-feature=+crt-static` already linked
; the Visual C++ runtime into the executable. No file-in-use handling beyond
; the check below, because there is one file.

Unicode true
ManifestDPIAware true

!define APP "mbrd"
!define PUBLISHER "omznc"
!define UNINST "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}"

; VERSION is passed in by the workflow: `makensis /DVERSION=0.3.0`.
!ifndef VERSION
  !define VERSION "0.0.0"
!endif

Name "${APP} ${VERSION}"
OutFile "..\..\dist\mbrd_${VERSION}_x64-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\${APP}"
InstallDirRegKey HKCU "Software\${APP}" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

!include "MUI2.nsh"
!define MUI_ICON "..\icons\mbrd.ico"
!define MUI_UNICON "..\icons\mbrd.ico"
!define MUI_ABORTWARNING

; No components page and no directory page by default — there is one component
; and the per-user location is not a choice worth making somebody read about.
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\mbrd.exe"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; Refuse rather than corrupt. Windows will not let a running image be
; overwritten, and an installer that ploughs on leaves a half-installed
; directory and a confusing error.
Function .onInit
  FindProcDLL::FindProc "mbrd.exe"
  ${If} $R0 == 1
    MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION \
      "mbrd is running. Close it and press OK to carry on." IDOK retry
    Abort
    retry:
  ${EndIf}
FunctionEnd

Section "Install"
  SetOutPath "$INSTDIR"
  File "..\..\dist\mbrd.exe"

  WriteRegStr HKCU "Software\${APP}" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\${APP}.lnk" "$INSTDIR\mbrd.exe"

  ; Add/Remove Programs. `NoModify` and `NoRepair` because neither is offered,
  ; and an entry that advertises buttons that do nothing is worse than one
  ; that does not.
  WriteRegStr HKCU "${UNINST}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${UNINST}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINST}" "DisplayIcon" "$INSTDIR\mbrd.exe"
  WriteRegStr HKCU "${UNINST}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegStr HKCU "${UNINST}" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKCU "${UNINST}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST}" "NoRepair" 1

  ; `.mbrd`, under HKCU so no elevation is needed. The ProgID is separate from
  ; the extension key on purpose: that is the arrangement that lets somebody
  ; choose a different default without this uninstall taking their choice with
  ; it.
  WriteRegStr HKCU "Software\Classes\.mbrd" "" "mbrd.board"
  WriteRegStr HKCU "Software\Classes\mbrd.board" "" "mbrd board"
  WriteRegStr HKCU "Software\Classes\mbrd.board\DefaultIcon" "" "$INSTDIR\mbrd.exe,0"
  WriteRegStr HKCU "Software\Classes\mbrd.board\shell\open\command" \
    "" "$\"$INSTDIR\mbrd.exe$\" $\"%1$\""

  ; Tell the shell the association changed, or the icon does not appear until
  ; the next sign-in.
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\mbrd.exe"
  Delete "$INSTDIR\uninstall.exe"
  ; What the in-app updater may have left beside the executable. Removed by
  ; name rather than with `RMDir /r`, which on a directory somebody may have
  ; put things in is not a risk worth taking for a tidier uninstall.
  Delete "$INSTDIR\mbrd.exe.old"
  RMDir /r "$INSTDIR\.mbrd-update"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${APP}.lnk"

  DeleteRegKey HKCU "${UNINST}"
  DeleteRegKey HKCU "Software\${APP}"
  DeleteRegKey HKCU "Software\Classes\mbrd.board"
  ; Only if it is still ours. Somebody who has since pointed `.mbrd` at
  ; another application should keep that choice.
  ReadRegStr $0 HKCU "Software\Classes\.mbrd" ""
  ${If} $0 == "mbrd.board"
    DeleteRegKey HKCU "Software\Classes\.mbrd"
  ${EndIf}

  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'
SectionEnd
