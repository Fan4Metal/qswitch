; qswitch installer (Inno Setup 6).
; Built by tools\make_release.py, which passes the version and the icon path:
;   ISCC /DMyAppVersion=0.1.0 /DVersionInfoVersion=0.1.0.0 /DAppIcon=..\target\app.ico tools\setup.iss

#define MyAppName "qswitch"
#define MyAppFileStem "qswitch"
#define MyAppExeName "qswitch.exe"
#define MyAppPublisher "Fan4_Metal"
#ifndef MyAppVersion
  #define MyAppVersion "0.0.0"
#endif
#ifndef VersionInfoVersion
  #define VersionInfoVersion "0.0.0.0"
#endif
#ifndef AppIcon
  #define AppIcon "..\target\app.ico"
#endif

[Setup]
; A new GUID for every app (the same one in tools\make_release.py).
AppId={{D0714DDC-AA47-43C1-AD18-942035492AAD}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
VersionInfoVersion={#VersionInfoVersion}
AppCopyright=Copyright (C) 2026 {#MyAppPublisher}
AppPublisher={#MyAppPublisher}
; Per-user installation without administrator rights:
; {autopf} points to %LOCALAPPDATA%\Programs.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
OutputDir=..\dist
OutputBaseFilename={#MyAppFileStem}_{#MyAppVersion}_Setup
SetupIconFile={#AppIcon}
UninstallDisplayIcon={app}\{#MyAppExeName}
LicenseFile=..\LICENSE
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; A running copy is closed (Restart Manager) before its exe is replaced.
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
english.AutoStart=Start with Windows (in the notification area)
russian.AutoStart=Запускать вместе с Windows (в области уведомлений)

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "autostart"; Description: "{cm:AutoStart}"; GroupDescription: "{cm:AdditionalIcons}"

[Registry]
; The same value the program writes for its "Start with Windows" setting
; (autostart.rs): the exe's path and --hidden, so it starts in the tray.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "qswitch"; ValueData: """{app}\{#MyAppExeName}"" --hidden"; Flags: uninsdeletevalue; Tasks: autostart

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.ru.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent
; A silent installation (make_release.py --install) over a running copy:
; the copy was closed by CloseApplications; start it again in the tray.
Filename: "{app}\{#MyAppExeName}"; Parameters: "--hidden"; Flags: nowait; Check: WizardSilent

[UninstallRun]
Filename: "{app}\{#MyAppExeName}"; Parameters: "--quit"; Flags: runhidden waituntilterminated; RunOnceId: "quit"

; Optional blocks, taken from the sibling projects when needed:
; - Explorer context menu for folders and drives ([Registry] under
;   HKCU\Software\Classes\{Directory,Drive}\shell\<Verb>, task "contextmenu"):
;   disk_flashlight/tools/setup.iss.
; - File associations through the registry (ProgID, OpenWithProgids,
;   Capabilities, RegisteredApplications, a [Run] entry that opens
;   ms-settings:defaultapps, ChangesAssociations=yes): metalmd/tools/setup.iss.
;   qview registers them from the exe instead (qview --register).
; - Asking a running copy to quit through its own --quit ([Code]
;   PrepareToInstall and [UninstallRun]) and offering to delete the settings
;   on uninstallation (CurUninstallStepChanged): qrec/tools/setup.iss.
; - MinVersion=10.0.19041 when an API needs Windows 10 2004: qrec.
