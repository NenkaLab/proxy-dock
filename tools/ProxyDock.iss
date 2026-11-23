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
; /ALLUSERS (used by the "Program Files" button) restarts setup as administrator
PrivilegesRequiredOverridesAllowed=commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
LicenseFile=..\LICENSE
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
Compression=lzma2/ultra64
LZMAUseSeparateProcess=yes
LZMADictionarySize=1048576
LZMANumFastBytes=273
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
; Inno Setup 6 has no Chinese: copies from its repository (tools\innosetup\README.md)
Name: "zh_CN"; MessagesFile: "innosetup\ChineseSimplified.isl"
Name: "zh_TW"; MessagesFile: "innosetup\ChineseTraditional.isl"

[CustomMessages]
en.DirProgramFiles=Program Files (all users)
en.StartMenuIcon=Create a Start menu shortcut
en.DirUser=AppData (this user only)
en.NeedAdmin=Installing to Program Files needs administrator rights.%n%nRestart setup as administrator?
ko.DirProgramFiles=Program Files (모든 사용자)
ko.StartMenuIcon=시작 메뉴에 바로 가기 만들기
ko.DirUser=AppData (현재 사용자만)
ko.NeedAdmin=Program Files에 설치하려면 관리자 권한이 필요합니다.%n%n관리자 권한으로 설치를 다시 시작할까요?
ja.DirProgramFiles=Program Files (すべてのユーザー)
ja.StartMenuIcon=スタート メニューにショートカットを作成する
ja.DirUser=AppData (現在のユーザーのみ)
ja.NeedAdmin=Program Files にインストールするには管理者権限が必要です。%n%n管理者としてセットアップを再起動しますか?
de.DirProgramFiles=Program Files (alle Benutzer)
de.StartMenuIcon=Verknüpfung im Startmenü erstellen
de.DirUser=AppData (nur dieser Benutzer)
de.NeedAdmin=Für die Installation in Program Files sind Administratorrechte nötig.%n%nSetup als Administrator neu starten?
fr.DirProgramFiles=Program Files (tous les utilisateurs)
fr.StartMenuIcon=Créer un raccourci dans le menu Démarrer
fr.DirUser=AppData (cet utilisateur)
fr.NeedAdmin=L'installation dans Program Files nécessite des droits administrateur.%n%nRelancer l'installation en tant qu'administrateur ?
es.DirProgramFiles=Program Files (todos los usuarios)
es.StartMenuIcon=Crear un acceso directo en el menú Inicio
es.DirUser=AppData (solo este usuario)
es.NeedAdmin=Instalar en Program Files requiere permisos de administrador.%n%n¿Reiniciar la instalación como administrador?
zh_CN.DirProgramFiles=Program Files (所有用户)
zh_CN.StartMenuIcon=在开始菜单中创建快捷方式
zh_CN.DirUser=AppData (仅当前用户)
zh_CN.NeedAdmin=安装到 Program Files 需要管理员权限。%n%n以管理员身份重新启动安装程序?
zh_TW.DirProgramFiles=Program Files (所有使用者)
zh_TW.StartMenuIcon=在開始功能表中建立捷徑
zh_TW.DirUser=AppData (僅目前使用者)
zh_TW.NeedAdmin=安裝到 Program Files 需要系統管理員權限。%n%n以系統管理員身分重新啟動安裝程式?

[Tasks]
Name: "startmenuicon"; Description: "{cm:StartMenuIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[InstallDelete]
; files of an older version that the new one no longer ships
Type: filesandordirs; Name: "{app}\_internal"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\ProxyDock"; Filename: "{app}\ProxyDock.exe"; Tasks: startmenuicon
Name: "{autodesktop}\ProxyDock"; Filename: "{app}\ProxyDock.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\ProxyDock.exe"; Description: "{cm:LaunchProgram,ProxyDock}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /T /IM ProxyDock.exe"; Flags: runhidden; RunOnceId: "StopProxyDock"

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';

var
  Relaunching: Boolean;

function ProgramFilesDir: String;
begin
  Result := ExpandConstant('{commonpf}\ProxyDock');
end;

function UserDir: String;
begin
  Result := ExpandConstant('{userpf}\ProxyDock');
end;

function IsUnder(Path, Base: String): Boolean;
begin
  Path := Lowercase(AddBackslash(Path));
  Base := Lowercase(AddBackslash(Base));
  Result := Copy(Path, 1, Length(Base)) = Base;
end;

{ start setup again with /ALLUSERS: Inno Setup asks for administrator rights itself (UAC), so the
  "launch ProxyDock" checkbox at the end still runs the app as the normal user }
procedure RelaunchAsAdmin(Dir: String);
var
  Code: Integer;
begin
  if MsgBox(CustomMessage('NeedAdmin'), mbConfirmation, MB_YESNO) <> IDYES then
    Exit;
  if ShellExec('', ExpandConstant('{srcexe}'), '/ALLUSERS /LANG=' + ActiveLanguage + ' /DIR="' + Dir + '"',
               '', SW_SHOW, ewNoWait, Code) then
  begin
    Relaunching := True;
    WizardForm.Close;
  end;
end;

procedure ProgramFilesClick(Sender: TObject);
begin
  if IsAdminInstallMode then
    WizardForm.DirEdit.Text := ProgramFilesDir
  else
    RelaunchAsAdmin(ProgramFilesDir);
end;

procedure UserClick(Sender: TObject);
begin
  WizardForm.DirEdit.Text := UserDir;
end;

procedure InitializeWizard;
var
  Pf, User: TNewButton;
  W: Integer;
begin
  { two buttons under "Browse...", right-aligned with it }
  W := WizardForm.DirBrowseButton.Width;
  if W < ScaleX(190) then
    W := ScaleX(190);
  Pf := TNewButton.Create(WizardForm);
  Pf.Parent := WizardForm.DirBrowseButton.Parent;
  Pf.Caption := CustomMessage('DirProgramFiles');
  Pf.Width := W;
  Pf.Height := WizardForm.DirBrowseButton.Height;
  Pf.Left := WizardForm.DirBrowseButton.Left + WizardForm.DirBrowseButton.Width - W;
  Pf.Top := WizardForm.DirBrowseButton.Top + WizardForm.DirBrowseButton.Height + ScaleY(8);
  Pf.OnClick := @ProgramFilesClick;
  User := TNewButton.Create(WizardForm);
  User.Parent := Pf.Parent;
  User.Caption := CustomMessage('DirUser');
  User.Width := W;
  User.Height := Pf.Height;
  User.Left := Pf.Left;
  User.Top := Pf.Top + Pf.Height + ScaleY(6);
  User.OnClick := @UserClick;
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  { a folder under Program Files typed or browsed by hand: same as the button }
  if (CurPageID = wpSelectDir) and not IsAdminInstallMode and
     (IsUnder(WizardDirValue, ExpandConstant('{commonpf64}')) or IsUnder(WizardDirValue, ExpandConstant('{commonpf32}'))) then
  begin
    RelaunchAsAdmin(WizardDirValue);
    Result := False;
  end;
end;

procedure CancelButtonClick(CurPageID: Integer; var Cancel, Confirm: Boolean);
begin
  if Relaunching then
    Confirm := False;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Cmd: String;
begin
  { remove "start at login" only if it points at this installation }
  if (CurUninstallStep = usUninstall) and RegQueryStringValue(HKCU, RunKey, 'ProxyDock', Cmd) then
    if Pos(Lowercase(ExpandConstant('{app}')), Lowercase(Cmd)) > 0 then
      RegDeleteValue(HKCU, RunKey, 'ProxyDock');
end;
