; TuplePad's Windows installer (Inno Setup 6). It installs for the current person, with no
; administrator password, into %LOCALAPPDATA%\Programs\TuplePad, so TuplePad can update itself
; there. TuplePad.exe is the app; bin\tuplepad.exe is the terminal command (the two names can't
; share a folder on Windows, which ignores case), and bin goes on the PATH.
;
;   iscc /DAppVersion=v001 /DSourceDir=<folder with tuplepad-app.exe and tuplepad.exe> packaging\windows\tuplepad.iss
;
; Updates run it with /VERYSILENT /relaunch=1, which reopens TuplePad when it's done.

#ifndef AppVersion
  #define AppVersion "v000"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\core\target\release"
#endif

[Setup]
AppId={{6F1E5B8C-3A2D-4E7F-9C41-2B8D7A5E0F13}
AppName=TuplePad
AppVersion={#AppVersion}
AppVerName=TuplePad {#AppVersion}
AppPublisher=Baz Studios LLC
AppPublisherURL=https://github.com/Baz-Studios-LLC/TuplePad
AppSupportURL=https://github.com/Baz-Studios-LLC/TuplePad/issues
AppUpdatesURL=https://github.com/Baz-Studios-LLC/TuplePad/releases
DefaultDirName={autopf}\TuplePad
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename=TuplePad-{#AppVersion}-windows-x64-setup
SetupIconFile=TuplePad.ico
UninstallDisplayIcon={app}\TuplePad.exe
UninstallDisplayName=TuplePad
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
ChangesEnvironment=yes
ChangesAssociations=yes
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "contextmenu"; Description: "Add ""Edit with TuplePad"" to File Explorer's menu"; GroupDescription: "File Explorer:"
Name: "addtopath"; Description: "Add the tuplepad command to PATH"; GroupDescription: "Terminal:"
Name: "desktopicon"; Description: "Put TuplePad on the desktop"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\tuplepad-app.exe"; DestDir: "{app}"; DestName: "TuplePad.exe"; Flags: ignoreversion
Source: "{#SourceDir}\tuplepad.exe"; DestDir: "{app}\bin"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\TuplePad"; Filename: "{app}\TuplePad.exe"
Name: "{autodesktop}\TuplePad"; Filename: "{app}\TuplePad.exe"; Tasks: desktopicon

[Registry]
; "Open with" lists TuplePad for any file.
Root: HKCU; Subkey: "Software\Classes\Applications\TuplePad.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "TuplePad"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Applications\TuplePad.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\TuplePad.exe"" ""%1"""
; Edit with TuplePad, on every file's menu.
Root: HKCU; Subkey: "Software\Classes\*\shell\TuplePad"; ValueType: string; ValueName: ""; ValueData: "Edit with TuplePad"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\*\shell\TuplePad"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\TuplePad.exe"; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\*\shell\TuplePad\command"; ValueType: string; ValueName: ""; ValueData: """{app}\TuplePad.exe"" ""%1"""; Tasks: contextmenu

[Run]
Filename: "{app}\TuplePad.exe"; Description: "Open TuplePad"; Flags: nowait postinstall skipifsilent
; After an update.
Filename: "{app}\TuplePad.exe"; Flags: nowait; Check: Relaunching

[Code]
const
  EnvironmentKey = 'Environment';

function Relaunching: Boolean;
begin
  Result := ExpandConstant('{param:relaunch|0}') = '1';
end;

{ Adds a folder to the person's PATH, once. }
procedure AddToPath(Folder: string);
var
  Paths: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path', Paths) then
    Paths := '';
  if Pos(';' + Uppercase(Folder) + ';', ';' + Uppercase(Paths) + ';') > 0 then
    exit;
  if (Paths <> '') and (Copy(Paths, Length(Paths), 1) <> ';') then
    Paths := Paths + ';';
  RegWriteExpandStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path', Paths + Folder);
end;

procedure RemoveFromPath(Folder: string);
var
  Paths: string;
  Found: Integer;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path', Paths) then
    exit;
  Paths := ';' + Paths + ';';
  Found := Pos(';' + Uppercase(Folder) + ';', Uppercase(Paths));
  if Found = 0 then
    exit;
  Delete(Paths, Found, Length(Folder) + 1);
  RegWriteExpandStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path', Copy(Paths, 2, Length(Paths) - 2));
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToPath(ExpandConstant('{app}\bin'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}\bin'));
end;
