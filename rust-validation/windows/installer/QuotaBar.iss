#ifndef PayloadDir
  #error PayloadDir must point to the extracted Windows package.
#endif
#ifndef AppVersion
  #error AppVersion is required.
#endif
#ifndef OutputPath
  #error OutputPath is required.
#endif

[Setup]
AppId=QuotaBar.Windows
AppName=QuotaBar
AppVersion={#AppVersion}
AppPublisher=QuotaBar contributors
AppPublisherURL=https://github.com/zjxps2007/quotabar
AppSupportURL=https://github.com/zjxps2007/quotabar/issues
AppUpdatesURL=https://github.com/zjxps2007/quotabar/releases
DefaultDirName={localappdata}\Programs\QuotaBar
DefaultGroupName=QuotaBar
PrivilegesRequired=lowest
SetupArchitecture=x64
ArchitecturesAllowed=x64compatible and not arm64
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
WizardStyle=modern dynamic
DisableProgramGroupPage=auto
OutputDir={#OutputPath}
OutputBaseFilename=QuotaBar-{#AppVersion}-Windows-x64-Setup
SetupIconFile=..\assets\QuotaBar.ico
UninstallDisplayIcon={app}\QuotaBar.Monitor.exe
LicenseFile={#PayloadDir}\LICENSE
Compression=lzma2/normal
SolidCompression=yes
CloseApplications=yes
RestartApplications=no
Uninstallable=yes

[Languages]
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#PayloadDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\QuotaBar"; Filename: "{app}\QuotaBar.Monitor.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\QuotaBar"; Filename: "{app}\QuotaBar.Monitor.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\QuotaBar.Monitor.exe"; Description: "{cm:LaunchProgram,QuotaBar}"; Flags: nowait postinstall skipifsilent

[Registry]
; Registration is opt-in inside the app; uninstall removes only our Run value.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "QuotaBar"; Flags: dontcreatekey uninsdeletevalue

; User settings and credentials live outside {app} and are never removed.
; Claude connections must be disconnected in the app before uninstalling.

[Code]
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DashboardFile: String;
  Attempt: Integer;
begin
  if CurUninstallStep <> usPostUninstall then Exit;
  DashboardFile := ExpandConstant('{app}\Dashboard\QuotaBar.Dashboard.exe');
  if not FileExists(DashboardFile) then Exit;
  Log('Waiting for the dashboard executable to be released.');
  for Attempt := 1 to 50 do
  begin
    if DeleteFile(DashboardFile) or not FileExists(DashboardFile) then
    begin
      RemoveDir(ExtractFileDir(DashboardFile));
      RemoveDir(ExpandConstant('{app}'));
      Exit;
    end;
    Sleep(100);
  end;
  Log('The dashboard executable is still in use: ' + DashboardFile);
end;
