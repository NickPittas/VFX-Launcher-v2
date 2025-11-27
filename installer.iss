; VFX Launcher Installer Script for Inno Setup
; Download Inno Setup from: https://jrsoftware.org/isdl.php

#define MyAppName "VFX Launcher"
#define MyAppVersion "1.0.0"
#define MyAppPublisher "Your Company Name"
#define MyAppExeName "VFX_Launcher.exe"

[Setup]
; NOTE: The value of AppId uniquely identifies this application.
AppId={{VFX-LAUNCHER-2024}}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
AllowNoIcons=yes
OutputDir=installer_output
OutputBaseFilename=VFX_Launcher_Setup
SetupIconFile=ui_slick\icons\icon_3.ico
Compression=lzma
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=admin
ArchitecturesInstallIn64BitMode=x64

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; Main executable (single-file build from PyInstaller)
Source: "dist\VFX_Launcher.exe"; DestDir: "{app}"; Flags: ignoreversion
; NOTE: Don't use "Flags: ignoreversion" on any shared system files

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[Code]
var
  DataDirPage: TInputDirWizardPage;

procedure InitializeWizard;
begin
  { Create a custom page for database location }
  DataDirPage := CreateInputDirPage(wpSelectDir,
    'Select Database Location', 'Where should the application database be stored?',
    'The database will store all project and file information. ' +
    'Select a location that is accessible to all users who will use this application.' + #13#10#13#10 +
    'Recommended: A shared network drive or local folder with proper permissions.',
    False, 'New Folder');
  
  { Set default database location to user's AppData }
  DataDirPage.Add('Database Folder');
  DataDirPage.Values[0] := ExpandConstant('{commonappdata}\{#MyAppName}');
end;

function GetDataDir(Param: String): String;
begin
  { Return the selected data directory }
  Result := DataDirPage.Values[0];
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  DataDir: String;
  ConfigFile: String;
begin
  if CurStep = ssPostInstall then
  begin
    { Create the data directory }
    DataDir := DataDirPage.Values[0];
    if not DirExists(DataDir) then
      CreateDir(DataDir);
    
    { Create a config file with the database path }
    ConfigFile := ExpandConstant('{app}\config.ini');
    SaveStringToFile(ConfigFile, '[Database]' + #13#10, False);
    SaveStringToFile(ConfigFile, 'Path=' + DataDir + '\vfx_launcher.db' + #13#10, True);
    SaveStringToFile(ConfigFile, #13#10 + '[Paths]' + #13#10, True);
    SaveStringToFile(ConfigFile, 'DataDir=' + DataDir + #13#10, True);
  end;
end;

