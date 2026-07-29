; deskwarden/installer/deskwarden.iss
;
; Per-user Inno Setup installer for deskwarden. No admin rights / UAC prompt
; required (PrivilegesRequired=lowest, installs under %LOCALAPPDATA%).
;
; Build-time dependency: Inno Setup 6 (ISCC.exe) only. Unlike an earlier
; draft of this script, this version does NOT use the Inno Download Plugin
; (idp.iss) -- see the [Code] section comment on InstallBwCliIfMissing for
; why. See installer/README.md for full build instructions.
;
; Build with:
;   iscc deskwarden.iss /DAppVersion=0.1.0
;
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{63CBCB72-5383-4AE7-AFB7-5EE0530E4630}
AppName=Deskwarden
AppVersion={#AppVersion}
AppPublisher=deskwarden (unofficial, unaffiliated with Bitwarden, Inc.)
DefaultDirName={localappdata}\deskwarden
DefaultGroupName=Deskwarden
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputBaseFilename=deskwarden-{#AppVersion}-installer
; Compiled installer lands directly in installer/, matching what
; .github/workflows/release.yml (Task 6) expects to find and publish --
; without this, Inno's default OutputDir ("Output\" relative to this
; script) would put it one level down from where the release workflow
; looks.
OutputDir=.
Compression=lzma2
SolidCompression=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "..\target\release\deskwarden.exe"; DestDir: "{app}"; Flags: ignoreversion
; Helper script for the bw-CLI bootstrap step below. `dontcopy` means it's
; bundled into the installer payload but not installed onto the user's
; system as an app file; it's pulled out on demand via
; ExtractTemporaryFile and run once, then left in {tmp} for Windows to
; clean up.
Source: "bootstrap-bw.ps1"; DestDir: "{tmp}"; Flags: dontcopy

[Icons]
Name: "{group}\Deskwarden"; Filename: "{app}\deskwarden.exe"

[Registry]
; HKCU, not HKLM: this is a per-user install with no admin rights, and
; autostart should only apply to the user who installed it. Uninstall
; removes this value automatically via uninsdeletevalue.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Deskwarden"; ValueData: """{app}\deskwarden.exe"""; Flags: uninsdeletevalue; Tasks: autostart

[Tasks]
Name: "autostart"; Description: "Start Deskwarden automatically when you sign in"; Flags: checkedonce

[Code]
{ Runs bootstrap-bw.ps1 (see that file for full detail) to ensure bw.exe is
  available, downloading it from Bitwarden's own GitHub releases and
  verifying its Authenticode signature first if it's missing.

  This step shells out to PowerShell rather than using the Inno Download
  Plugin (idp.iss) that an earlier draft of this script referenced. Reasons:

  1. bitwarden/clients (the real, verified repo -- confirmed 2026-07-28
     against https://api.github.com/repos/bitwarden/clients/releases) is a
     monorepo publishing releases for multiple products (cli, desktop,
     browser, web) interleaved by date. GitHub's generic "latest release"
     for the repo is NOT reliably the CLI's latest release -- resolving the
     right one requires filtering the releases list by tag prefix
     ("cli-v*"), which means parsing JSON. Inno's Pascal Script has no JSON
     support and idpDownloadFile alone can't do this resolution; PowerShell
     (Invoke-RestMethod) can, in a handful of readable lines.
  2. This project's own signature-verification code (src/signature.rs)
     already shells out to PowerShell's Get-AuthenticodeSignature rather
     than binding WinVerifyTrust directly, specifically to avoid getting
     security-critical FFI wrong. Doing the same here keeps the bw-CLI
     bootstrap step's verification logic consistent with that choice
     instead of introducing a second, different verification mechanism.
  3. It avoids adding idp.iss as a build-time dependency that Task 6's CI
     workflow (choco install innosetup only installs Inno Setup itself, not
     the plugin) would otherwise need an extra step to fetch.
}
procedure InstallBwCliIfMissing();
var
  ScriptPath: String;
  ResultCode: Integer;
  ExecOk: Boolean;
begin
  ExtractTemporaryFile('bootstrap-bw.ps1');
  ScriptPath := ExpandConstant('{tmp}\bootstrap-bw.ps1');

  ExecOk := Exec('powershell.exe',
    Format('-NoProfile -ExecutionPolicy Bypass -File "%s" -InstallDir "%s"', [
      ScriptPath, ExpandConstant('{app}')]),
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode);

  if (not ExecOk) then
  begin
    MsgBox('Deskwarden could not launch PowerShell to set up the Bitwarden CLI (bw). ' +
      'Please install it yourself from https://bitwarden.com/help/cli/ and ensure it''s on your PATH.',
      mbInformation, MB_OK);
    exit;
  end;

  case ResultCode of
    0: ; { success: already present, or freshly installed and verified }
    2: MsgBox('The Bitwarden CLI download did not pass signature verification, so it was not installed. ' +
        'Please install it yourself from https://bitwarden.com/help/cli/ and ensure it''s on your PATH.',
        mbError, MB_OK);
  else
    MsgBox('Deskwarden could not automatically set up the Bitwarden CLI (bw). ' +
      'Please install it yourself from https://bitwarden.com/help/cli/ and ensure it''s on your PATH.',
      mbInformation, MB_OK);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    InstallBwCliIfMissing();
end;

{ Deliberately no CurUninstallStepChanged PATH cleanup here: per the design
  spec's Installer section, uninstalling deskwarden intentionally leaves the
  bw CLI (and the PATH entry bootstrap-bw.ps1 added for it) in place, since
  the user may be using bw independently of deskwarden. Only deskwarden's
  own tracked files and the autostart registry value (uninsdeletevalue,
  above) are removed. }
