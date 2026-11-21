; dongsi NSIS 설치 프로그램 hook. tauri.conf.json의 bundle.windows.nsis.installerHooks로 들어간다.
; 이 파일은 UTF-8(BOM)이어야 한다. BOM이 없으면 NSIS가 시스템 코드 페이지로 읽어 한글이 깨진다.
;
; 시작 메뉴 바로 가기 이름을 "동시"로 한다(D-064). 윈도우 검색은 시작 메뉴 바로 가기 이름으로 앱을 찾는다.
; Tauri는 ${PRODUCTNAME}.lnk(dongsi.lnk)를 만들고 이름을 바꾸는 설정이 없어서, 만든 뒤에 이름을 바꾼다
; (AppUserModelID 같은 바로 가기 속성은 그대로 남는다).
; 업데이트 모드(/UPDATE)에서는 Tauri가 바로 가기를 만들지도 지우지도 않으므로 여기서도 건드리지 않는다.
; 바탕 화면 바로 가기는 그대로 dongsi.lnk다(마침 페이지에서 고르면 이 hook 뒤에 만들어진다).

!define DONGSI_START_LNK "$SMPROGRAMS\동시.lnk"

!macro NSIS_HOOK_POSTINSTALL
  ${If} ${FileExists} "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    Delete "${DONGSI_START_LNK}"
    Rename "$SMPROGRAMS\${PRODUCTNAME}.lnk" "${DONGSI_START_LNK}"
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    !insertmacro IsShortcutTarget "${DONGSI_START_LNK}" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "${DONGSI_START_LNK}"
      Delete "${DONGSI_START_LNK}"
    ${EndIf}
  ${EndIf}
!macroend
