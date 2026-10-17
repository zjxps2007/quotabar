# 경량화 설계

플랫폼 분리 이후 기존 Swift 구현은 `macos/`에 있습니다. 이 문서의 아래 항목은 기존 macOS 동작을 설명합니다. Windows 구조와 검증은 [Windows 검증 기록](WINDOWS_VALIDATION.md), 공통 목표는 [플랫폼 계획](PLATFORM_PLAN.md)을 참고하세요. Windows 모니터·Dashboard·Claude callback은 별도 실행 파일이며 익명 파이프와 현재 사용자 전용 저장소로 연결합니다.

기준: sylearn/AIUsage `e3502ea77b79c2ef5b88f33e36231f156243a86b`, 2026-10-16 확인.

0.2.2부터 제품·Swift 패키지·실행 파일 이름은 QuotaBar입니다. 데이터 경로 `~/Library/Application Support/AIUsage Lite`와 앱 식별자 `com.aiusage.lite`는 기존 계정, 로그인 파일, 단일 인스턴스 잠금의 호환을 위해 유지합니다. 이전 검증 기록에 등장하는 AIUsage Lite는 이 앱의 이전 이름입니다.

원본의 Dashboard는 Provider 조회 결과뿐 아니라 Claude/Codex/OpenCode의 비용 원장, 프록시 집계, 계정 레지스트리, 전체 앱 설정에 의존합니다. Provider 계정 화면도 API provider 관리와 CLI 전환 기능을 함께 참조합니다. 따라서 두 화면 파일만 복사하면 프록시·게이트웨이·서버 계층이 계속 필요합니다.

Lite는 원본의 quota 파싱 및 공식 Claude status line 연결 코드를 분리하고, 계정 저장·스케줄링·Dashboard를 작은 앱 계층으로 다시 구성합니다.

| 영역 | Lite의 처리 |
| --- | --- |
| Codex quota | 고정 ChatGPT quota URL, 워크스페이스 헤더, 기간 라벨, 계정 식별 코드를 재사용. 앱 전용 인증의 OAuth 회전은 별도 모듈에서 처리. |
| Gemini quota | Code Assist 조회 및 모델군 집계를 재사용. OAuth 갱신·CLI 실행·클라이언트 비밀값 탐색 제거. |
| Claude quota | 공식 CLI status line snapshot, 기존 명령 보존, 설치·복구·세션 세대 검증 재사용. 저장 위치를 Lite로 격리. |
| Local tokens | 원본 스트리밍 JSON projection을 확장하고 표시 기간의 토큰 집계 구현. 프록시 비용 원장·영구 DB 제외. |
| UI | 상주 `NSStatusItem`과 필요할 때 생성하는 `NSPopover`. Dashboard·Providers·Settings는 창을 열 때 실행하는 SwiftUI 화면 프로세스. WebView와 웹 번들 없음. |
| Background | 다음 작업 시각에 일회성 타이머 하나 예약, 3개 이하 계정 요청, 창 비활성 시 로그 스캔 중단, 잠자기 요청 취소. |
| Credentials | 사용자 전용 폴더의 계정별 로그인 파일. 폴더 0700·파일 0600, 원자적 교체. 비밀값을 설정·로그·화면에 출력하지 않음. |
| Release | SwiftPM 실행 파일을 주 앱과 내장 Dashboard helper로 패키징. 주 실행 파일은 Claude callback의 짧은 실행 모드도 처리. |

## 화면별 역할

- Dashboard는 요약 통계, 계정별 잔여량·리셋 시각, 상단바 표시 선택, 로컬 사용 활동을 표시합니다. 연결 오류는 짧게 알리고 계정 관리로 이동시킵니다.
- Providers & Accounts는 별도의 연결 관리 목록을 사용합니다. 연결 방식·상태·오류, 로그인, 개별 새로고침, 모니터링 일시 정지와 삭제를 제공하며 이메일은 이름과 다를 때만 추가로 표시합니다. 연결 확인 필요 필터에는 잔여량 부족을 포함하지 않습니다.
- 설정은 화면 모드, Dock·상단바 표시, 조회 주기, 로컬 통계 옵션, 저장 위치와 앱 정보를 제공합니다. 상단 계정 연결 버튼은 계정 관리에, 전체 새로고침 버튼은 Dashboard에만 둡니다.
- 공통 사이드바에는 탐색만 두며 Provider별 계정 수와 반복 설명을 제거했습니다. 메뉴와 단축키의 Dashboard 열기는 Dashboard 탭으로 이동합니다.
- 공통 테마는 `AppTheme`의 동적 무채색 팔레트를 사용합니다. 시스템의 밝은 모드에서는 검정 강조색·흰 카드, 어두운 모드에서는 흰 강조색·검정 계열 표면을 사용합니다. 선택 메뉴와 주요 버튼은 반대 색상으로 글자를 표시합니다. 시트·상단바 팝업·앱 아이콘에도 같은 블랙·화이트 방향을 적용하며 Provider별 색상과 실제 경고색은 유지합니다.
- 앱 브랜드는 `Resources/AppMark.svg`의 세 개 상승 막대 모양으로 통일합니다. `AppLogo`는 사이드바·팝업·앱 정보의 테마 대응 타일, `AppMark`는 상단바와 설정 미리보기의 단색 심볼입니다. SVG는 한 번 읽어 캐시하고, `scripts/make-icon.swift`도 같은 SVG를 그려 투명 여백·둥근 검정 타일이 있는 Dock용 ICNS를 만듭니다. 별도의 SF Symbol을 앱 로고로 사용하지 않습니다.
- 상단바 심볼은 높이 14pt로 표시하고 비율을 유지합니다. 네이티브 `NSImage.size`와 SwiftUI 기본 높이가 같은 상수를 사용하며, 사이드바·팝업의 타일은 별도 지정 높이를 사용합니다.
- 이름 + 잔여량 모드는 14pt 로고와 시스템 상단바 크기의 고정 폭 숫자 글꼴을 6pt 간격으로 한 템플릿 이미지에 그립니다. `MenuBarExtra`의 기본 아이콘·텍스트 배치에 의존하지 않으며 설정 미리보기도 같은 이미지를 사용합니다. 현재 문자열의 이미지 하나만 캐시하고 표시 값이 바뀔 때 교체하므로 주기적인 이미지 생성이나 캐시 누적은 없습니다. 도움말과 접근성 이름은 기존 텍스트를 제공합니다.
- 로컬 `.app` 교체 후 Dock이 이전 아이콘을 유지하는 경우를 처리하기 위해, 시작 완료 시 같은 원본에서 만든 512px `AppDockIcon.png`를 `NSApplication.applicationIconImage`에 한 번 지정합니다. ICNS의 모든 해상도를 메모리에 올리지 않습니다. Finder와 앱 정보에는 `AppIcon-4.icns`를 사용하며 Dashboard helper도 같은 아이콘을 포함합니다.
- `LiteSettings.appearance`에 `system`·`light`·`dark`를 저장합니다. 이전 설정의 누락 값과 이후 버전의 알 수 없는 모드는 `system`으로 해석하며 계정 로딩을 막지 않습니다. 설정 저장에 성공한 뒤 `NSApplication.appearance`를 한 번 변경해 앱 창·시트에 적용하고, 상단바 팝업에는 같은 값의 `preferredColorScheme`도 지정합니다. 시스템 모드에서는 `nil`로 되돌려 macOS 설정을 따르며 반복 확인 타이머는 사용하지 않습니다. 시작 시 저장된 값을 적용하고 샘플 모드의 `--demo-appearance`도 같은 경로를 사용합니다.
- Dock 표시 여부는 `LiteSettings.showDockIcon`에 저장하고 시작 완료 시와 설정 변경 시에만 `NSApplication`의 활성화 정책을 `regular` 또는 `accessory`로 적용합니다. 전환 중에는 기존 창의 포커스를 복원합니다. 상단바는 항상 유지하고 팝업에 Dashboard·설정 열기를 제공하므로 Dock을 숨긴 상태에서도 설정을 되돌릴 수 있습니다.
- `menuBarDisplayMode`는 `providerAndRemaining`·`remainingOnly`·`iconOnly`를 지원합니다. 저장된 값이 없거나 알 수 없으면 기존 이름 + 잔여량으로 표시하고 Dock도 기본적으로 표시합니다. 실제 `NSStatusItem`과 설정 미리보기는 `MenuBarTitleImage`의 글꼴·로고·합성 이미지를 공유하며, 잔여량 계산·갱신 필요 표시·자세한 도움말은 모든 형식에서 같은 계정 선택을 사용합니다. 추가 폴링이나 타이머는 없습니다.

## 식별과 실패 처리

Codex는 이메일만으로 합치지 않습니다. 워크스페이스와 사용자 ID 또는 이메일 조합을 저장하고 새 파일의 정체성이 일치할 때만 토큰을 반영합니다. Gemini는 subject, 이메일, refresh token의 SHA-256 식별값 순서로 연결을 구분합니다. 표시 이름과 파일 경로는 계정 정체성으로 사용하지 않습니다.

네트워크 실패가 나면 이전 성공 스냅샷을 유지하면서 실패 상태를 따로 표시합니다. quota가 없으면 100%로 만들어 표시하지 않습니다. 요청이 끝날 때 작업 세대와 계정 존재 여부를 다시 확인하므로 삭제된 카드가 늦은 응답으로 되살아나지 않습니다.

계정의 `showInMenuBar`는 모니터링의 `enabled`와 분리합니다. 기존 설정에 값이 없으면 표시가 기본값입니다. Dashboard 카드의 스위치는 이 값만 저장하며, 상단바 요약·도움말·팝오버는 활성 상태이면서 표시가 켜진 계정만 사용합니다. 숨긴 계정도 기존 주기로 조회하고 Dashboard에 표시합니다. 계정 재연결 시에도 기존 표시 선택을 유지합니다.

설정 파일이 손상되면 빈 설정으로 덮어쓰지 않습니다. 삭제 시 로그인 파일 작업이 실패하면 계정 항목을 유지합니다. Claude 연결 복구가 불가능할 때는 프로필 백업을 남기고 원인을 표시합니다.

## 의도적인 범위 축소

이 버전은 사용량 추적기입니다. CLI 계정 전환, API 키의 청구 비용, OpenCode, 프록시, 게이트웨이, API node 배포, Call Analytics, 자동 업데이트, Windows 앱은 포함하지 않습니다. 추가 Provider는 `ProviderKind`와 in-memory fetch adapter를 확장하는 방식으로 붙일 수 있습니다.

## 로컬 집계 및 화면 갱신 최적화 (2026-10-17)

- `SessionProjectionReader`는 Claude의 content 배열과 도구 이름·입력 등 사용하지 않는 메타데이터를 스트림에서 건너뜁니다. `LocalUsageScanner.parse`는 실제 사용량 행과 세션 메타데이터에만 날짜 파서를 적용합니다.
- Codex 중복 제거는 `Set<Event>`로 시간·모델·수치·누적 여부를 비교합니다. 이벤트마다 수치를 문자열로 바꾸지 않으며, 분기 원본 탐색과 토큰 합계의 중간 배열도 만들지 않습니다.
- 표시 기간의 달력 날짜 시작점과 표시 문자열을 한 번 생성하고 이진 검색으로 각 이벤트를 배정합니다. 24시간 고정 간격을 가정하지 않아 DST와 30/45분 시간대를 보존합니다. 범위 밖의 오래된 누적 값은 여전히 분기·증분 기준값으로 사용합니다.
- 집계 단계에서도 작업 취소를 확인합니다. `ActivityView`는 동일한 보고서가 전달되면 SwiftUI의 동등성 비교로 본문 재계산을 건너뜁니다. 실제 새 스캔은 `scannedAt`이 바뀌므로 날짜와 통계가 다시 표시됩니다.
- 변경되지 않은 오류·안내·창 상태의 Published 대입과 동일한 설정의 파일 저장을 생략합니다. 상단바 문자열 한 번 계산 중에는 선택 계정 목록을 재사용합니다. 조회 주기·동시 요청 상한·캐시 상한과 로그인/갱신 프로토콜은 동일합니다.

### 상단바 대기 상태

- `RefreshSchedule`은 계정 조회와 Dashboard 로그 스캔의 다음 시각을 계산합니다. 30초 반복 타이머 대신 가장 이른 작업에 일회성 타이머 하나를 예약합니다. 창을 닫으면 기존 최소 15분 조회 주기를 유지하며 로그 스캔은 예약하지 않습니다. 할 일이 없거나 시스템이 잠자면 타이머도 없습니다.
- 창·탭·설정·활성 계정 변경 때 예약을 다시 계산하고, 스캔 중에는 스캔 타이머를 두지 않습니다. 읽기 실패도 5분 간격을 적용해 즉시 재시도 반복을 방지합니다. 수동 새로고침은 바로 실행합니다.
- Dashboard를 닫으면 화면 프로세스를 종료해 SwiftUI 메타데이터·화면·파서 캐시를 모두 반환합니다. 상주 모니터는 계정 스냅샷·로그인 세션·조회 예약을 유지합니다. 로컬 활동은 다음에 창을 열 때 다시 읽으며 대화 본문은 보관하지 않습니다. 팝업은 닫힐 때 hosting controller를 해제합니다. 2초 뒤 한 번 실행하는 `malloc_zone_pressure_relief(nil, 0)`도 유지하고 창을 다시 열면 정리를 취소합니다.

### Dashboard의 수명과 내부 통신

- 주 프로세스만 `instance.lock`과 자격 증명 저장소를 소유합니다. Dashboard는 `Contents/Helpers/QuotaBar Dashboard.app`을 필요할 때 실행하며 별도 Dock 아이콘을 만들지 않습니다. 창을 닫으면 종료하고 다시 열면 기존 설정·계정·조회 결과를 받아 표시합니다.
- `DashboardHost`와 `DashboardClient`는 부모가 물려준 익명 파이프 두 개로 통신합니다. 포트나 공용 IPC 파일은 없으며 화면에는 설정·quota·오류만 전달합니다. `DashboardPipe`는 4바이트 길이, 최대 4MiB 메시지, 분할 수신, EOF와 잘못된 메시지 처리를 제공합니다. 계정 연결 요청은 응답을 받은 뒤 완료 표시하며 실패·연결 단절·30초 시간 초과를 구분합니다.
- 화면의 `MonitorStore`는 상태 복사본입니다. quota 조회·설정 저장·계정 삭제·인증 저장은 주 프로세스에 명령을 보내 기존 로직에서 수행합니다. 새 Codex 브라우저 로그인 결과만 파이프로 한 번 전달하며 로그나 설정에 쓰지 않습니다. 토큰 자동 갱신과 중복 회전 방지 로직은 주 프로세스에 그대로 있습니다.
- 로컬 로그 스캔은 화면 프로세스만 실행합니다. Dashboard가 보이고 해당 탭이 선택됐을 때만 예약하며, 창 종료 시 프로세스와 함께 정리됩니다. 상주 모니터는 창 표시 상태를 받아 기존 조회 간격과 상단바 대기 간격을 전환합니다.
- 부모가 종료되거나 내부 연결이 끊어지면 화면도 정상 종료합니다. 진행 중인 Codex 로그인 정리는 기존 앱 종료 핸들러가 수행합니다. 화면이 준비되지 않으면 10초 뒤 실행을 정리합니다. 다른 모드로 직접 실행한 helper는 파이프가 없으면 시작하지 않습니다.
- 패키징된 리소스는 `AppResources`에서 주 앱 또는 helper의 자체 리소스 번들을 먼저 찾습니다. 개발 빌드 디렉터리에 의존하지 않습니다.

## 원본 사용 활동 UI 적용 (2026-10-17)

- 원본 `DashboardView+Heatmap.swift`의 26주 구성, 달력 눈금과 상세 툴팁을 Lite의 로컬 통계에 맞춰 적용하고 출처를 `NOTICE`에 명시했습니다. 색상과 Provider 아이콘은 구독 카드와 같은 공통 컴포넌트를 사용합니다. 공통 배경·테두리·강조색은 `AppTheme`에 모았습니다. Codex·Claude·Gemini의 원본 로고 PNG/SVG를 번들하고 Provider마다 한 번 읽어 재사용합니다. 로고별 투명 여백을 고려해 표시 크기를 맞춥니다.
- Claude·Codex 카드를 최소 300pt, 최대 560pt의 적응형 격자로 배치합니다. 셀은 7–16pt로 제한하고 기록이 없는 Provider는 빈 상태를 표시합니다. 하단 요약은 너비에 따라 한 줄 또는 두 줄로 배치합니다.
- 이전 일별 막대 차트를 원본 방식의 히트맵·날짜 상세로 교체하고 Charts 프레임워크 의존성을 제거했습니다. 보고서 변경 때만 달력·모델별 집계·농도 기준을 준비하며, 포인터 이동으로 로그를 다시 읽지 않습니다. 툴팁은 ScrollView 바깥 레이어에서 창 안에 배치합니다.
- 앱은 182일 집계를 요청하고 히트맵은 이번 주를 포함한 26주를 월요일부터 표시합니다. Core 스캐너의 기본 90일은 유지하고 요청 기간을 1–366일로 제한합니다. 기존 파일·이벤트·캐시 상한과 창 닫힘 시 절전 정책은 유지합니다.
- 표시 Provider는 등록 계정의 Provider 집합과 로컬 통계 지원 범위(Claude·Codex)의 교집합입니다. 같은 Provider는 한 번만 표시하며 `enabled`·`showInMenuBar`에 영향을 받지 않습니다. 등록 변경도 `ActivityView`의 동등성 비교에 포함하고, 삭제한 Provider의 툴팁을 정리합니다. 표시할 Provider가 없으면 사용 활동 전체를 숨깁니다.
- 강조색은 Codex 보라·파랑, Claude 주황, Gemini 파랑을 밝은/어두운 시스템 테마에 맞춰 사용합니다. Codex 팔레트는 참고 프로젝트의 UI 색상이며 공식 지정색으로 확인한 값은 아닙니다. 잔여량 막대·히트맵·툴팁·연결 상태와 계정의 스위치·버튼이 같은 Provider 색상을 참조합니다. 로고는 원본 색상을 유지하며 Gemini의 다색 별도 그대로 표시합니다.

## Codex 앱 로그인 및 자동 갱신 (2026-10-17)

`CodexLoginSession`은 공식 `codex -c cli_auth_credentials_store="file" login`을 권한 0700의 임시 홈에서 실행합니다. 공식 실행 파일은 PATH, ChatGPT/Codex 앱 번들, 표준 설치 경로에서 찾습니다. PKCE·state·브라우저 callback은 공식 CLI에 위임하며, 그 시도에서 생성된 auth.json만 가져옵니다. 실패 시 사용자의 기존 CLI 로그인으로 대체하지 않습니다. 완료·취소·실패·시간 초과·정상 앱 종료 시 해당 프로세스와 임시 파일을 정리합니다.

앱 로그인 계정은 `appManagedLogin: true`, 빈 sourcePath로 저장합니다. 기존 설정에 플래그가 없으면 파일 연결로 처리합니다. `CodexSessionManager`는 앱 전용 인증만 회전하며 파일 연결 계정은 갱신 대상으로 삼지 않습니다. 공식 CLI의 Codex OAuth grant와 원본 Provider가 쓰는 public client ID 및 `auth.openai.com/oauth/token`을 사용합니다. Responses API용 Sign in with ChatGPT의 dynamic client grant와 혼용하지 않습니다. 프로토콜 변경 시 호환성 보완이 필요할 수 있습니다.

갱신 결과의 사용자·워크스페이스를 검증하고, 같은 계정의 요청을 하나로 합칩니다. 로그인 파일 저장은 사용량 조회보다 먼저 수행하여 quota 요청 실패 시에도 새 refresh token을 유지합니다. 잠깐의 저장 실패는 교체 토큰을 메모리에 보관해 저장만 재시도합니다. 삭제·재연결 시 세대 검증과 원본 값 비교를 통해 늦은 응답을 폐기합니다. 취소된 사용량 조회와 독립적으로 진행 중인 갱신 결과는 보존합니다. 정상 앱은 프로세스 수명 동안 파일 잠금을 유지하여 여러 앱 인스턴스가 같은 토큰을 동시에 갱신하지 못하게 합니다.

로그인 토큰은 로그·UI·설정 파일에 출력하지 않으며 OAuth POST의 리다이렉트는 허용하지 않습니다. 일시적인 실패는 다음 스케줄에서 재시도하고, 만료·취소된 갱신 세션은 재로그인 전까지 반복 요청하지 않습니다. 계정 삭제는 로컬 연결과 로그인 파일을 제거하며 서버의 세션을 원격 revoke하지는 않습니다.

공식 절차 참고: [Codex 인증](https://learn.chatgpt.com/docs/auth). 제3자 ChatGPT plan usage용 별도 grant는 [Accounts and sessions](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)를 참고했습니다.

## 키체인 승인창 제거 (0.2.1)

일반 조회·연결·갱신·삭제는 `SecretVault`의 로컬 파일만 사용합니다. 자격 증명은 암호화하지 않으며 현재 사용자에게만 열리는 디렉터리와 파일 권한으로 보호합니다. 같은 사용자 권한으로 실행되는 다른 프로그램에 대한 격리는 제공하지 않습니다. 파일 크기를 1 MiB로 제한하고 심볼릭 링크·하드 링크 읽기를 거부합니다. 임시 파일을 0600으로 생성한 뒤 전체 쓰기와 동기화가 성공하면 같은 디렉터리에서 원자적으로 교체합니다.

`CredentialMigration`은 모니터링 시작 전 한 번만 실행합니다. 기존 계정 ID·정체성을 검증한 인증만 옮기고, 파일과 설정의 `credentialStorageVersion: 1`을 저장한 뒤 예전 키체인 항목 삭제를 시도합니다. 승인 없이 접근할 수 없는 계정은 카드만 유지하고 재로그인을 안내합니다. 앱 로그인 계정을 기존 CLI 파일로 대체하지 않습니다. 설정 저장이 실패하면 이전 항목을 삭제하지 않고 모니터링을 중단합니다. 접근이 거부되어 남은 키체인 항목은 이후 사용하지 않습니다.

과거의 file-based 키체인은 현대적인 `LAContext.interactionNotAllowed`만으로 모든 승인창을 막을 수 없으므로, 이전 작업에 한해 process-scoped `SecKeychainSetUserInteractionAllowed(false)`도 함께 적용하고 원래 값으로 복구합니다. 이 호환 API의 deprecated 경고는 예상된 경고입니다. [Apple의 두 키체인 구현 설명](https://developer.apple.com/documentation/technotes/tn3137-on-mac-keychains) 및 로컬 SDK의 `SecItem.h`·`SecKeychain.h`를 확인했습니다.
