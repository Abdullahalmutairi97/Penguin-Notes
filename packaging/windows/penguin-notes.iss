; Windows installer (Inno Setup). Built by .github/workflows/release.yml:
;   iscc /DVersion=0.6.0 /DDist=C:\path\to\dist packaging\windows\penguin-notes.iss
; Installs for the current user only, so it needs no admin rights.

[Setup]
AppId={{6F1C2E8A-4B7D-4E1F-9C3A-5D2B8E7F0A41}
AppName=Penguin Notes
AppVersion={#Version}
AppPublisher=Penguin Notes
AppPublisherURL=https://github.com/Abdullahalmutairi97/Penguin-Notes
DefaultDirName={autopf}\Penguin Notes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#Dist}
OutputBaseFilename=PenguinNotes-Setup
SetupIconFile=..\..\assets\icon.ico
UninstallDisplayIcon={app}\penguin-notes.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "Put Penguin Notes on the desktop"

[Files]
Source: "{#Dist}\penguin-notes\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{autoprograms}\Penguin Notes"; Filename: "{app}\penguin-notes.exe"
Name: "{autodesktop}\Penguin Notes"; Filename: "{app}\penguin-notes.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\penguin-notes.exe"; Description: "Open Penguin Notes"; Flags: nowait postinstall skipifsilent
