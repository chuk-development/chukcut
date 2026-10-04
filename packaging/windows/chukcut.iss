; EXPERIMENTAL. The Windows installer, built with Inno Setup 6 from the
; staging directory packaging\windows\package.ps1 leaves behind:
;
;   iscc /DAppVersion=0.2.0 /DSourceDir=target\windows\chukcut-0.2.0-x86_64-windows ^
;        /DOutputDir=target\dist packaging\windows\chukcut.iss
;
; Writes <OutputDir>\chukcut-<version>-x86_64-windows-setup.exe. It installs
; per user by default (no administrator prompt) and offers a machine-wide
; install. Unsigned: SmartScreen warns until a code-signing certificate
; signs it.

#ifndef AppVersion
  #error Pass /DAppVersion=<version>
#endif
#ifndef SourceDir
  #error Pass /DSourceDir=<staging directory>
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

[Setup]
AppId={{6F1C2B9E-6A57-4C1F-9B7B-2C1A7D0E5C31}
AppName=chukcut
AppVersion={#AppVersion}
AppPublisher=chukcut contributors
AppPublisherURL=https://github.com/chuk-development/chukcut
AppSupportURL=https://github.com/chuk-development/chukcut/issues
DefaultDirName={autopf}\chukcut
DefaultGroupName=chukcut
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile={#SourceDir}\LICENSE.txt
OutputDir={#OutputDir}
OutputBaseFilename=chukcut-{#AppVersion}-x86_64-windows-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes
UninstallDisplayIcon={app}\chukcut.exe

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{autoprograms}\chukcut"; Filename: "{app}\chukcut.exe"
Name: "{autodesktop}\chukcut"; Filename: "{app}\chukcut.exe"; Tasks: desktopicon

[Registry]
; .chukcut project files open in the editor.
Root: HKA; Subkey: "Software\Classes\.chukcut"; ValueType: string; ValueName: ""; ValueData: "chukcut.project"; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\chukcut.project"; ValueType: string; ValueName: ""; ValueData: "chukcut project"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\chukcut.project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\chukcut.exe,0"
Root: HKA; Subkey: "Software\Classes\chukcut.project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\chukcut.exe"" ""%1"""

[Run]
Filename: "{app}\chukcut.exe"; Description: "{cm:LaunchProgram,chukcut}"; Flags: nowait postinstall skipifsilent
