; Inno Setup script for Ledgit.
;
; Build it with:   installer\build.ps1
; or by hand:      cargo build --release
;                  iscc installer\ledgit.iss
;
; Inno Setup rather than WiX: WiX is the right answer when an org needs MSI
; policy deployment, and a great deal of ceremony otherwise. This ships two
; statically linked exes and a file association, which is one page of script
; here and an afternoon of XML there.

#define AppName        "Ledgit"
#define AppVersion     "0.1.0"
#define AppPublisher   "Ledgit"
#define AppExeName     "ledgit-gui.exe"
#define CliExeName     "ledgit.exe"
#define SourceDir      "..\target\release"

[Setup]
; Never change AppId: it is how Windows recognises an upgrade rather than a
; second copy of the program.
AppId={{08c002af-6c79-4592-8c94-a350b0f0ad7a}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
UninstallDisplayIcon={app}\{#AppExeName}
OutputDir=..\target\installer
OutputBaseFilename=Ledgit-{#AppVersion}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Per-user install when run without elevation, per-machine with it. A budget is
; a personal tool; do not demand admin for it.
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"
Name: "addtopath";   Description: "Add the ledgit command line tool to PATH"; GroupDescription: "Command line:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\{#CliExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md";               DestDir: "{app}"; DestName: "README.md"; Flags: ignoreversion
Source: "..\docs\*";                  DestDir: "{app}\docs"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{group}\{#AppName}";            Filename: "{app}\{#AppExeName}"
Name: "{group}\Uninstall {#AppName}";  Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}";      Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Registry]
; Associate .ledgit so double-clicking a budget opens it. The app takes the
; file as its first argument, which is exactly what the shell passes.
Root: HKA; Subkey: "Software\Classes\.ledgit"; ValueType: string; ValueName: ""; ValueData: "Ledgit.Budget"; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Ledgit.Budget"; ValueType: string; ValueName: ""; ValueData: "Ledgit budget"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Ledgit.Budget\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExeName},0"
Root: HKA; Subkey: "Software\Classes\Ledgit.Budget\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}"" ""%1"""
; Optional PATH entry for the CLI.
Root: HKA; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NotAlreadyOnPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\{#AppExeName}"; Description: "Open {#AppName}"; Flags: nowait postinstall skipifsilent

[Code]
{ Adding the same directory to PATH twice is a classic installer bug; check first. }
function NotAlreadyOnPath(Dir: string): Boolean;
var
  Existing: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Existing) then
    Existing := '';
  Result := Pos(LowerCase(Dir), LowerCase(Existing)) = 0;
end;
