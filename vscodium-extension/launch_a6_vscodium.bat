@echo off
REM Launch Agent 6's OWN VSCodium instance with the A6 tool extension loaded.
REM
REM Key detail: A6 runs in an ISOLATED profile (its own --user-data-dir and
REM --extensions-dir). That gives A6 a clean, separate VSCodium AND sidesteps
REM Electron's single-instance lock, so this never gets swallowed by the
REM operator's GUI VSCodium (the "Start VSCodium did nothing" failure mode).
REM
REM The window is normal here; keeping it out of the way (minimize / off-screen /
REM the Vi_minimizer virtual desktop) is a launch-side concern handled elsewhere.
setlocal
cd /d "%~dp0"

REM This extension lives inside the GGUF-Chatbox repo (…\GGUF-Chatbox\vscodium-extension\),
REM so VSCodium and A6's workspace are TWO levels up in the file-cabinet hub.
set "EXT_DIR=%~dp0"
set "VSCODIUM=%~dp0..\..\VSCODIUM\VSCodium.exe"
if not defined A6_WORKSPACE set "A6_WORKSPACE=%~dp0..\..\A6_workspace"
REM Isolated profile: A6's own VSCodium user-data + extensions (also sidesteps the
REM single-instance lock). Override A6_PROFILE to run a throwaway/test profile.
if not defined A6_PROFILE set "A6_PROFILE=%USERPROFILE%\.a6_vscodium"
REM Tool bridge dir (inbox/outbox/processed). Default under GGUF Chatbox's home,
REM since the coordinator that fills it lives in GGUF Chatbox. Override to relocate.
if not defined A6_TOOLS_DIR set "A6_TOOLS_DIR=%USERPROFILE%\.gguf-chatbox\a6_tools"

if not exist "%VSCODIUM%" (
    echo VSCodium.exe not found at "%VSCODIUM%"
    echo Point this script at your VSCodium, or install it via the SOC Master Widget.
    exit /b 1
)

REM NOTE: space-form flags, not --flag=value. This VSCodium build ignores the
REM '=' form of --user-data-dir (falls back to the default profile and forwards
REM to any running instance); the space form is honored.
"%VSCODIUM%" ^
    --extensionDevelopmentPath "%EXT_DIR%" ^
    --user-data-dir "%A6_PROFILE%\user-data" ^
    --extensions-dir "%A6_PROFILE%\extensions" ^
    --disable-workspace-trust ^
    "%A6_WORKSPACE%"

endlocal
