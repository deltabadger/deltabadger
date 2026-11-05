; Deltabadger's NSIS hooks (PLAN-B7 r2.3, r3.3, r4.3, r5.4), inserted by tauri-bundler's installer.nsi
; (bundle.windows.nsis.installerHooks). PREINSTALL runs at the start of the Install section, before the template's
; CheckIfAppIsRunning and before the first file is copied; POSTINSTALL after the last one.
;
; The installer never copies over a running engine: an update's launcher has stopped its own engine before it started
; this installer, and once the launcher has exited only the installer can keep that true. So PREINSTALL first writes
; `$INSTDIR\.updating` (every engine start refuses while it exists, rust/src/bundle.rs `enter`; the launcher shows U13
; or U10, supervisor.rs `wait_while_updating`), then asks Restart Manager whether any process uses the engine's file
; (a query only: RmShutdown is never called). One does → nothing is copied: the mark goes, exit code 2, and the app is
; started again only when the command line carries /R (an update the user clicked; the launcher's attempt file makes it
; show U5d, update.rs `settle_attempt`). The installer keeps `.updating` open until POSTINSTALL: the launcher reads "an
; installer is still copying" from that handle, which closes with the process, so a killed installer leaves an orphaned
; mark (U10, D-DESKTOP-WIN-INTERRUPTED-COPY).
;
; Test hooks: compiled only into a build for the test identifier, each behind a file in
; %LOCALAPPDATA%\deltabadger-test-hooks (script/rust/desktop_smoke/update_smoke.py). src-tauri/src/tests.rs and
; desktop.yml check that the release installer has none.

Var DbUpdating

!macro NSIS_HOOK_PREINSTALL
  !if "${BUNDLEID}" == "com.deltabadger.app.test"
    ; r5.4: a barrier before anything is written, so a test can start an engine after the launcher has gone.
    ${If} ${FileExists} "$LOCALAPPDATA\deltabadger-test-hooks\hold"
      FileOpen $0 "$LOCALAPPDATA\deltabadger-test-hooks\waiting" w
      FileClose $0
      ${DoWhile} ${FileExists} "$LOCALAPPDATA\deltabadger-test-hooks\hold"
        Sleep 100
      ${Loop}
    ${EndIf}
  !endif

  ClearErrors
  FileOpen $DbUpdating "$INSTDIR\.updating" w
  ${If} ${Errors}
    DetailPrint "Deltabadger: could not write $INSTDIR\.updating"
  ${EndIf}

  ; Restart Manager: which processes use the engine's file. A zero-length list answers ERROR_MORE_DATA (234) when any
  ; do; any other failure counts as in use (nothing is copied). A first install has no engine file to be in use. A
  ; file in use is asked about again for 5 s: a scanner holds a new file only briefly, a running engine stays.
  Push $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  Push $6
  Push $7
  StrCpy $5 0
  ${Do}
    ; Every answer starts as "-": a call the System plugin couldn't make leaves it, and only the string "0" counts as
    ; success (NSIS's numeric "=" would read "" or "-" as 0).
    StrCpy $0 "-"
    StrCpy $2 "-"
    StrCpy $4 1
    ${IfNot} ${FileExists} "$INSTDIR\deltabadger-engine.exe"
      StrCpy $4 0
    ${Else}
      System::Call 'rstrtmgr::RmStartSession(*i .r1, i 0, w .r6) i .r0'
      ${If} $0 == 0
        ; RmRegisterResources takes an array of string pointers: one string, and a one-cell array holding its address.
        System::Call '*(&w${NSIS_MAX_STRLEN} "$INSTDIR\deltabadger-engine.exe") p .r3'
        System::Call '*(p r3) p .r7'
        StrCpy $0 "-"
        System::Call 'rstrtmgr::RmRegisterResources(i r1, i 1, p r7, i 0, p 0, i 0, p 0) i .r0'
        ${If} $0 == 0
          StrCpy $0 "-"
          System::Call 'rstrtmgr::RmGetList(i r1, *i 0 .r2, *i 0, p 0, *i 0) i .r0'
          ${If} $0 == 0
          ${AndIf} $2 == 0
            StrCpy $4 0
          ${EndIf}
        ${EndIf}
        System::Free $7
        System::Free $3
        System::Call 'rstrtmgr::RmEndSession(i r1)'
      ${EndIf}
    ${EndIf}
    DetailPrint "Deltabadger: the engine's file in use: $4 (Restart Manager $0, $2 processes)"
    !if "${BUNDLEID}" == "com.deltabadger.app.test"
      ; What each look found, for the proofs' logs.
      FileOpen $3 "$LOCALAPPDATA\deltabadger-test-hooks\rm-result" a
      FileSeek $3 0 END
      FileWrite $3 "in use $4, Restart Manager $0, $2 processes$\r$\n"
      FileClose $3
    !endif
    ${If} $4 = 0
    ${OrIf} $5 >= 5
      ${ExitDo}
    ${EndIf}
    IntOp $5 $5 + 1
    Sleep 1000
  ${Loop}

  ${If} $4 = 1
    FileClose $DbUpdating
    Delete "$INSTDIR\.updating"
    ; Someone ran this installer by hand while Deltabadger runs: say so (an update's installer runs passive: no box).
    ${IfNot} ${Silent}
    ${AndIf} $PassiveMode != 1
      MessageBox MB_OK|MB_ICONINFORMATION "Deltabadger is running. Quit it from the Deltabadger icon's menu, then open this installer again."
    ${EndIf}
    ClearErrors
    ${GetOptions} $CMDLINE "/R" $0
    ${IfNot} ${Errors}
      nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" ""
    ${EndIf}
    SetErrorLevel 2
    Quit
  ${EndIf}
  Pop $7
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0

  !if "${BUNDLEID}" == "com.deltabadger.app.test"
    ; U13's window: a second launch while the installer holds the mark.
    ${If} ${FileExists} "$LOCALAPPDATA\deltabadger-test-hooks\sleep"
      Sleep 20000
    ${EndIf}
    ; U10: the installer ends after writing the mark, as one killed during the copy would.
    ${If} ${FileExists} "$LOCALAPPDATA\deltabadger-test-hooks\quit"
      Quit
    ${EndIf}
  !endif
!macroend

!macro NSIS_HOOK_POSTINSTALL
  FileClose $DbUpdating
  Delete "$INSTDIR\.updating"
!macroend
