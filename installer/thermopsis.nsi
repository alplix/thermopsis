; Thermopsis installer (per user, no administrator rights needed). Build: installer/build.sh
!define NAME "Thermopsis"
!define VER "0.1.0"
Name "${NAME} ${VER}"
OutFile "..\dist\Thermopsis-Setup-${VER}.exe"
InstallDir "$LOCALAPPDATA\Programs\Thermopsis"
RequestExecutionLevel user
SetCompressor /SOLID lzma
!include "MUI2.nsh"
!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\thermopsis.exe"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Thermopsis" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  File /r "..\dist\app\*.*"
  File "..\LICENSE"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\Thermopsis"
  CreateShortcut "$SMPROGRAMS\Thermopsis\Thermopsis.lnk" "$INSTDIR\thermopsis.exe"
  CreateShortcut "$SMPROGRAMS\Thermopsis\Uninstall.lnk" "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Thermopsis" "DisplayName" "${NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Thermopsis" "DisplayVersion" "${VER}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Thermopsis" "Publisher" "bitboinc"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Thermopsis" "UninstallString" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Start with Windows" SecAuto
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Thermopsis" "$INSTDIR\thermopsis.exe"
SectionEnd

Section "Uninstall"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Thermopsis"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Thermopsis"
  Delete "$SMPROGRAMS\Thermopsis\*.lnk"
  RMDir "$SMPROGRAMS\Thermopsis"
  RMDir /r "$INSTDIR"
SectionEnd
