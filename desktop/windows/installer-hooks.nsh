; Windows 安装包（NSIS）的钩子，由 tauri.conf.json 的 bundle > windows > nsis > installerHooks 引入。
;
; 内置 TeX Live 有约 1.9 万个文件，NSIS 默认把每个文件名都打在状态行上（「Extract: texlive\...」），
; 一闪一闪看不清，还拖慢安装。装文件期间只留一句固定的说明，进度条照常走；「显示细节」按钮也不要了

ShowInstDetails nevershow
ShowUninstDetails nevershow

; 语言 ID：2052 简中、1028 繁中、1041 日语、1042 韩语，其余用英文
!macro SUBO_STATUS ZH TW JA KO EN
  SetDetailsPrint textonly
  ${If} $LANGUAGE == 2052
    DetailPrint "${ZH}"
  ${ElseIf} $LANGUAGE == 1028
    DetailPrint "${TW}"
  ${ElseIf} $LANGUAGE == 1041
    DetailPrint "${JA}"
  ${ElseIf} $LANGUAGE == 1042
    DetailPrint "${KO}"
  ${Else}
    DetailPrint "${EN}"
  ${EndIf}
  SetDetailsPrint none
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro SUBO_STATUS "正在安装素帛，请稍候…" "正在安裝素帛，請稍候…" "素帛をインストールしています。しばらくお待ちください…" "Subo를 설치하는 중입니다. 잠시 기다려 주세요…" "Installing Subo, please wait…"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  SetDetailsPrint both
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro SUBO_STATUS "正在移除素帛…" "正在移除素帛…" "素帛を削除しています…" "Subo를 제거하는 중…" "Removing Subo…"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  SetDetailsPrint both
!macroend
