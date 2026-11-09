; Strata installer for Windows (Inno Setup 6, https://jrsoftware.org/isinfo.php).
;
; scripts/release.ps1 builds it; by hand:
;   ISCC /DAppVersion=0.1.1 /DSourceDir=<folder with strata.exe and strata-mcp.exe> installer\strata.iss
;
; Installs for the current user only, without admin rights, into
; %LOCALAPPDATA%\Programs\Strata. The in-app updater replaces strata.exe in
; place, which needs a folder the user can write to, so all-users installs
; (Program Files) are not offered.
;
; What it sets up:
; - Start menu shortcut (and an optional desktop shortcut), an entry in
;   Settings > Apps with an uninstaller.
; - .SC2Map files: Strata is listed in "Open with" and in Settings >
;   Default apps. Windows doesn't let installers take over a file type
;   the user already chose an app for (the StarCraft II editor), so the
;   finish page offers to open Strata's Default apps page instead.
; - Map folders (folders named *.SC2Map): "Open in Strata" in their context
;   menu (Windows 11: under "Show more options").
; - App Paths, so Win+R "strata" starts it.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\target\x86_64-pc-windows-msvc\release"
#endif
#ifndef OutputDir
  #define OutputDir "..\target\installer"
#endif

[Setup]
; Never change AppId: it is how Windows and later installers recognize Strata.
AppId={{B8FEE9B6-88C3-4D61-902C-7B6031E45F70}
AppName=Strata
AppVersion={#AppVersion}
AppVerName=Strata {#AppVersion}
AppPublisher=Strata
AppPublisherURL=https://github.com/erivgout/strata-sc2-editor
AppSupportURL=https://github.com/erivgout/strata-sc2-editor/issues
AppUpdatesURL=https://github.com/erivgout/strata-sc2-editor/releases
AppComments=A StarCraft II map editor
DefaultDirName={autopf}\Strata
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
OutputDir={#OutputDir}
OutputBaseFilename=strata-v{#AppVersion}-windows-x64-setup
SetupIconFile=..\crates\strata-app\assets\icons\icon.ico
UninstallDisplayIcon={app}\strata.exe
UninstallDisplayName=Strata
VersionInfoVersion={#AppVersion}
VersionInfoProductName=Strata
VersionInfoDescription=Strata setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
ChangesAssociations=yes
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\strata.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\strata-mcp.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Strata"; Filename: "{app}\strata.exe"; Comment: "StarCraft II map editor"
Name: "{autodesktop}\Strata"; Filename: "{app}\strata.exe"; Comment: "StarCraft II map editor"; Tasks: desktopicon

[Registry]
; How Strata opens maps.
Root: HKA; Subkey: "Software\Classes\Strata.SC2Map"; ValueType: string; ValueName: ""; ValueData: "StarCraft II map"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Strata.SC2Map\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\strata.exe,0"
Root: HKA; Subkey: "Software\Classes\Strata.SC2Map\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\strata.exe"" ""%1"""
; Offer Strata in "Open with" for .SC2Map files. The file type's current
; default is never changed (see the finish page).
Root: HKA; Subkey: "Software\Classes\.SC2Map\OpenWithProgids"; ValueType: string; ValueName: "Strata.SC2Map"; ValueData: ""; Flags: uninsdeletevalue
; List Strata in Settings > Default apps.
Root: HKA; Subkey: "Software\Strata\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "Strata"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Strata\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "StarCraft II map editor"
Root: HKA; Subkey: "Software\Strata\Capabilities"; ValueType: string; ValueName: "ApplicationIcon"; ValueData: "{app}\strata.exe,0"
Root: HKA; Subkey: "Software\Strata\Capabilities\FileAssociations"; ValueType: string; ValueName: ".SC2Map"; ValueData: "Strata.SC2Map"
Root: HKA; Subkey: "Software\Strata"; Flags: uninsdeletekeyifempty
Root: HKA; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "Strata"; ValueData: "Software\Strata\Capabilities"; Flags: uninsdeletevalue
; The name and file types "Open with" shows for strata.exe.
Root: HKA; Subkey: "Software\Classes\Applications\strata.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Strata"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\strata.exe\SupportedTypes"; ValueType: string; ValueName: ".SC2Map"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\strata.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\strata.exe"" ""%1"""
; "Open in Strata" on map folders (folders named *.SC2Map).
Root: HKA; Subkey: "Software\Classes\Directory\shell\Strata.Open"; ValueType: string; ValueName: ""; ValueData: "Open in Strata"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\shell\Strata.Open"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\strata.exe,0"
Root: HKA; Subkey: "Software\Classes\Directory\shell\Strata.Open"; ValueType: string; ValueName: "AppliesTo"; ValueData: "System.FileName:""*.SC2Map"""
Root: HKA; Subkey: "Software\Classes\Directory\shell\Strata.Open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\strata.exe"" ""%1"""
; Win+R "strata".
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\strata.exe"; ValueType: string; ValueName: ""; ValueData: "{app}\strata.exe"; Flags: uninsdeletekey

[Run]
Filename: "{app}\strata.exe"; Description: "Start Strata"; Flags: nowait postinstall skipifsilent
; Windows 11 opens Strata's own Default apps page; Windows 10 opens Default apps.
Filename: "ms-settings:defaultapps?registeredAppUser=Strata"; Description: "Choose Strata for .SC2Map files (opens Windows settings)"; Flags: postinstall shellexec skipifsilent

[UninstallDelete]
; Updates the in-app updater downloaded but never finished.
Type: files; Name: "{app}\.strata-update-*.exe"
