; Build with ISCC /DStageDir=<absolute staged release folder>
;                 /DOutputDir=<absolute output folder>
;                 /DAppVersion=<version without v>
;                 /DIconPath=<absolute path to auris-studio.ico>.
; Keep the AppId stable so later installers upgrade the same installation.
[Setup]
AppId={{90A79E7B-D7E6-4914-8DED-499849D83182}
AppName=Auris Studio
AppVersion={#AppVersion}
AppPublisher=uthree
AppPublisherURL=https://github.com/uthree/auris-studio
AppSupportURL=https://github.com/uthree/auris-studio/issues
DefaultDirName={localappdata}\Programs\Auris Studio
DefaultGroupName=Auris Studio
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
SetupIconFile={#IconPath}
UninstallDisplayIcon={app}\auris-studio.exe
LicenseFile={#StageDir}\LICENSE
OutputDir={#OutputDir}
OutputBaseFilename=auris-studio-v{#AppVersion}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
ChangesEnvironment=yes
CloseApplications=yes
; Tell Explorer to reread file associations after the [Registry] entries change.
ChangesAssociations=yes

; Include every staged binary, including the update helper when present. Historical
; tags repackaged through workflow_dispatch do not need to provide the helper.
[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

; Open .auris projects with the application on double-click. The installer is per-user, so HKA
; is HKCU; `main` opens its first argument as a project.
[Registry]
Root: HKA; Subkey: "Software\Classes\.auris"; ValueType: string; ValueName: ""; ValueData: "AurisStudio.Project"; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.auris\OpenWithProgids"; ValueType: string; ValueName: "AurisStudio.Project"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\AurisStudio.Project"; ValueType: string; ValueName: ""; ValueData: "Auris Studio Project"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\AurisStudio.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\auris-studio.exe,0"
Root: HKA; Subkey: "Software\Classes\AurisStudio.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\auris-studio.exe"" ""%1"""

[Icons]
Name: "{autoprograms}\Auris Studio"; Filename: "{app}\auris-studio.exe"
Name: "{userdesktop}\Auris Studio"; Filename: "{app}\auris-studio.exe"; Tasks: desktopicon

[Tasks]
Name: "addtopath"; Description: "Add auris and auris-mcp to the user PATH"; Flags: checkedonce
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Code]
const
  UserEnvironment = 'Environment';
  InstallerState = 'Software\uthree\Auris Studio';

function PathContains(const CurrentPath, Wanted: String): Boolean;
var
  Remaining, Entry: String;
  Separator: Integer;
begin
  Result := False;
  Remaining := CurrentPath;
  repeat
    Separator := Pos(';', Remaining);
    if Separator = 0 then
    begin
      Entry := Remaining;
      Remaining := '';
    end
    else
    begin
      Entry := Copy(Remaining, 1, Separator - 1);
      Delete(Remaining, 1, Separator);
    end;
    if CompareText(Trim(Entry), Wanted) = 0 then
    begin
      Result := True;
      Exit;
    end;
  until Remaining = '';
end;

function WithoutPathEntry(const CurrentPath, Wanted: String;
  PreserveTrailingSeparator: Boolean): String;
var
  Entry: String;
  EntryStart, Separator, RelativeSeparator: Integer;
begin
  Result := CurrentPath;
  EntryStart := 1;
  while EntryStart <= Length(CurrentPath) do
  begin
    RelativeSeparator := Pos(';', Copy(CurrentPath, EntryStart, Length(CurrentPath)));
    if RelativeSeparator = 0 then
      Separator := Length(CurrentPath) + 1
    else
      Separator := EntryStart + RelativeSeparator - 1;
    Entry := Copy(CurrentPath, EntryStart, Separator - EntryStart);
    if CompareText(Trim(Entry), Wanted) = 0 then
    begin
      if EntryStart = 1 then
        Result := Copy(CurrentPath, Separator + 1, Length(CurrentPath))
      else if PreserveTrailingSeparator and (Separator > Length(CurrentPath)) then
        Result := Copy(CurrentPath, 1, EntryStart - 1)
      else
        Result := Copy(CurrentPath, 1, EntryStart - 2) +
          Copy(CurrentPath, Separator, Length(CurrentPath));
      Exit;
    end;
    EntryStart := Separator + 1;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  CurrentPath, InstallPath: String;
  AddedToPath: Cardinal;
begin
  if (CurStep <> ssPostInstall) or not WizardIsTaskSelected('addtopath') then
    Exit;
  InstallPath := ExpandConstant('{app}');
  if not RegQueryStringValue(HKCU, UserEnvironment, 'Path', CurrentPath) then
    CurrentPath := '';
  if PathContains(CurrentPath, InstallPath) then
    Exit;
  AddedToPath := 1;
  if CurrentPath = '' then
    CurrentPath := InstallPath
  else if CurrentPath[Length(CurrentPath)] = ';' then
  begin
    CurrentPath := CurrentPath + InstallPath;
    AddedToPath := 2;
  end
  else
    CurrentPath := CurrentPath + ';' + InstallPath;
  if RegWriteExpandStringValue(HKCU, UserEnvironment, 'Path', CurrentPath) then
    RegWriteDWordValue(HKCU, InstallerState, 'AddedToPath', AddedToPath)
  else
    Log('The command-line tools were installed, but could not be added to PATH.');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  CurrentPath, UpdatedPath: String;
  AddedToPath: Cardinal;
begin
  if CurUninstallStep <> usPostUninstall then
    Exit;
  if not RegQueryDWordValue(HKCU, InstallerState, 'AddedToPath', AddedToPath) then
    Exit;
  if (AddedToPath <> 1) and (AddedToPath <> 2) then
    Exit;
  if RegQueryStringValue(HKCU, UserEnvironment, 'Path', CurrentPath) then
  begin
    UpdatedPath := WithoutPathEntry(CurrentPath, ExpandConstant('{app}'), AddedToPath = 2);
    if UpdatedPath <> CurrentPath then
    begin
      if UpdatedPath = '' then
        RegDeleteValue(HKCU, UserEnvironment, 'Path')
      else
        RegWriteExpandStringValue(HKCU, UserEnvironment, 'Path', UpdatedPath);
    end;
  end;
  RegDeleteValue(HKCU, InstallerState, 'AddedToPath');
  RegDeleteKeyIfEmpty(HKCU, InstallerState);
end;
