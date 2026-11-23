; ProxyDock Windows installer (Inno Setup 6). Built by: python tools/build.py installer
; which passes AppVersion, SourceDir (dist\win\ProxyDock), OutputDir and OutputName.
; Per-user install (no administrator): %LOCALAPPDATA%\Programs\ProxyDock. Settings stay in %APPDATA%\ProxyDock
; and are kept on uninstall.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\dist\win\ProxyDock"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist\release"
#endif
#ifndef OutputName
  #define OutputName "ProxyDock-setup"
#endif

[Setup]
AppId={{6F1B7E2A-3C4D-4E8F-9A1B-2C3D4E5F6A7B}
AppName=ProxyDock
AppVersion={#AppVersion}
AppVerName=ProxyDock {#AppVersion}
AppPublisher=NenkaLab
AppPublisherURL=https://github.com/NenkaLab
DefaultDirName={autopf}\ProxyDock
DefaultGroupName=ProxyDock
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
LicenseFile=..\LICENSE
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
Compression=lzma2/max
SolidCompression=yes
UninstallDisplayIcon={app}\ProxyDock.exe
UninstallDisplayName=ProxyDock
; running ProxyDock / sing-box / hysteria lock files in {app}: close them (and kill them if they will not close)
CloseApplications=force
RestartApplications=no
VersionInfoVersion={#AppVersion}
ShowLanguageDialog=auto

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "ko"; MessagesFile: "compiler:Languages\Korean.isl"
Name: "ja"; MessagesFile: "compiler:Languages\Japanese.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
#ifexist "compiler:Languages\ChineseSimplified.isl"
Name: "zh_CN"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"
#endif
#ifexist "compiler:Languages\ChineseTraditional.isl"
Name: "zh_TW"; MessagesFile: "compiler:Languages\ChineseTraditional.isl"
#endif

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[InstallDelete]
; files of an older version that the new one no longer ships
Type: filesandordirs; Name: "{app}\_internal"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\ProxyDock"; Filename: "{app}\ProxyDock.exe"
Name: "{autodesktop}\ProxyDock"; Filename: "{app}\ProxyDock.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\ProxyDock.exe"; Description: "{cm:LaunchProgram,ProxyDock}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /T /IM ProxyDock.exe"; Flags: runhidden; RunOnceId: "StopProxyDock"

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Cmd: String;
begin
  { remove "start at login" only if it points at this installation }
  if (CurUninstallStep = usUninstall) and RegQueryStringValue(HKCU, RunKey, 'ProxyDock', Cmd) then
    if Pos(Lowercase(ExpandConstant('{app}')), Lowercase(Cmd)) > 0 then
      RegDeleteValue(HKCU, RunKey, 'ProxyDock');
end;
