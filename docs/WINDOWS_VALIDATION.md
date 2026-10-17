# Windows 시제품 구현 및 검증

검증일: 2026-10-18. 환경: Windows x64 build 26200, .NET SDK 10.0.402 / 런타임 10.0.13. Windows 코드 빌드와 합성 통합 검사, 배포본 실행을 완료했다. 실제 서비스 계정으로 OAuth 로그인·quota 조회를 완료한 결과는 아니다.

## 구현 구조

```text
macos/                       기존 Swift 앱, UsageCore, 테스트, Mac 스크립트
windows/
  src/QuotaBar.Core/          모델, provider HTTP, OAuth 동시 갱신, 로그 집계
  src/QuotaBar.Windows/       DPAPI·ACL, CLI 로그인, Claude 연결, 상태 저장
  src/QuotaBar.Protocol/      버전 1 익명 파이프, 4MiB 길이 프레임
  src/QuotaBar.Monitor/       WinForms 트레이, 조회·전원·단일 실행 관리
  src/QuotaBar.Dashboard/     필요할 때 실행하는 WPF 화면
  src/QuotaBar.StatusLine/    Claude 입력 전달·quota 수집용 짧은 실행 파일
  tests/QuotaBar.Tests/       외부 테스트 패키지가 없는 통합 검사 실행 파일
  scripts/                   build, test, run, probe, package, smoke
shared/
  assets/                    기존 로고 원본
  fixtures/                  합성 quota 응답, 중복·분기·스트리밍 로그 및 기대값
  version.json               양쪽 빌드 버전
scripts/                     기존 Mac 명령을 유지하는 호환 진입점
```

Windows에서는 트레이 아이콘 자체에 남은 비율을 숫자로 표시하고 메뉴·툴팁에서 계정과 기간별 값을 제공한다. Dashboard에는 계정별 quota, 월요일 기준 최근 26주의 서비스별 활동 히트맵·모델별 토큰과 트레이 표시 스위치가 있다. 계정 연결·별칭·일시 정지·개별 새로고침은 Providers & Accounts에서 관리한다. Codex 앱 로그인과 CLI 파일 가져오기, Claude status line 연결·복원, Gemini Code Assist 파일 연결을 구현했다. 실시간 계정 조회 실패 시 마지막 성공 값을 보존한다.

설정과 암호화된 인증은 `%LOCALAPPDATA%\QuotaBar`에 저장한다. 원자적 파일 교체, 현재 사용자 전용 ACL, 현재 사용자 DPAPI, 인증 파일 소유자·하드링크·reparse point 검사, JSON 크기 제한을 적용했다. 모니터만 상태를 변경하고 화면에는 토큰을 보내지 않는다. 정상 종료는 이미 발급된 OAuth 회전 결과의 저장을 기다린다.

## macOS와 디자인 통일

Mac의 `AppTheme.swift`, `AppLogo.swift`, `Views.swift`, `ActivityView.swift`를 기준으로 Windows 화면을 맞췄다. Mac 소스는 변경하지 않았다.

- 공통 흑백 배경·사이드바·선택 상태, 16px 둥근 카드, 같은 SVG 앱 마크와 서비스별 로고·색상. 실행 파일 아이콘도 Mac의 검정 둥근 타일·흰색 마크를 사용한다.
- 메뉴를 Dashboard / Providers & Accounts / 설정으로 통일했다. 요약 4개, 적응형 구독 카드, 서비스별 활동을 Dashboard에 배치했다.
- 계정 검색·Provider·연결 상태 필터, 개별 새로고침, 모니터링 스위치를 계정 관리 화면에 배치했다. 트레이 표시 스위치는 구독 카드 하단에 있다.
- 설정을 화면 모드, 작업 표시줄·트레이, 모니터링, 앱 정보로 분리하고 즉시 적용·저장하도록 연결했다. Windows 트레이의 플랫폼별 동작은 유지한다.
- Dashboard·활동 영역·계정 관리·설정·Codex/Claude 연결 창을 라이트/다크로 렌더링했다. 최소 900×660, 넓은 1320×900, 빈 계정, 긴 이름·조회 오류·일시 정지 샘플을 포함해 실행당 18개 이미지를 생성한다. 자동화 이미지는 실제 WPF 컨트롤을 렌더링하며 Windows 창 테두리는 포함하지 않는다.

`DashboardWindow.cs`는 창과 테마·공통 구성, `DashboardPage.cs`는 구독·활동, `ManagementPages.cs`는 계정·설정, `Dialogs.cs`는 연결·이름 변경, `Controls.xaml`은 버튼·스위치·입력·스크롤 스타일을 담당한다. UI 이미지 검사는 실계정 로그인 성공이나 모든 입력 장치의 상호작용 검증을 의미하지 않는다.

## 트레이 숫자 표시

- 기본값은 남은 비율 숫자다. 모니터링과 트레이 표시가 켜진 계정의 모든 quota 기간 중 최솟값을 반올림한다. `38`은 38% 남음을 뜻하며 작은 아이콘에서는 `%`를 생략한다.
- 20% 이하 또는 연결 확인이 필요한 상태는 주황색으로 표시한다. 조회 실패 시 이전 숫자를 유지하고 툴팁에 이전 값이 포함되었음을 알린다. 값이 없으면 `—`, 오류로 값을 알 수 없으면 `!`, 표시할 계정이 없으면 로고를 사용한다.
- 설정의 작업 표시줄 및 트레이에서 `남은 비율 숫자`와 `로고만`을 선택한다. 즉시 적용·저장하며 이전 설정 파일도 읽을 수 있다. Dashboard를 닫아도 모니터가 트레이와 주기적 조회를 유지한다.
- Windows 작업 표시줄 테마와 DPI를 기준으로 아이콘을 만들고 테마·화면 변경 이벤트에서 갱신한다. 라이트/다크 각각 16·20·24·32·40·48·64px와 9개 값/상태, 총 126개 렌더링 및 네이티브 아이콘 크기를 확인했다. `tray-light.png`, `tray-dark.png`는 렌더링 표본이며 운영체제 트레이 화면을 캡처한 이미지는 아니다.

## 확인한 결과

### 우클릭 메뉴

트레이 좌클릭·좌측 더블클릭 처리기를 제거하고 `NotifyIcon.ContextMenuStrip`의 우클릭 동작을 사용한다. `TrayMenu.cs`는 macOS 팝업의 흑백 표면·둥근 카드·서비스 로고·색상을 적용한 WinForms 메뉴다. 사용량 카드와 **Dashboard 열기·종료만** 표시한다. 계정별 quota 기간은 두 열로 배치하며, 5시간 한도 없이 주간 한도만 있는 경우처럼 행에 기간이 하나면 카드 전체 너비를 사용한다. 잔여 막대·리셋 시간·조회 중·연결 확인 필요 상태를 함께 보여준다. 설정의 화면 테마를 따르고 메뉴가 열린 동안 갱신된 값도 반영한다.

`TrayMenuProbe.cs`는 합성 계정으로 **해당 검사 프로세스의 NotifyIcon 콜백 창에만** Windows 마우스 메시지를 보낸다. 좌클릭·더블클릭 무동작, 우클릭 메뉴 열기, Dashboard 미실행, Esc 닫기, 메뉴 열린 중 값 갱신·선택 유지, Dashboard·종료 명령 전달과 나머지 명령 제거를 확인했다. 15개 계정 상태에서 키보드로 스크롤해 하단 동작에 접근했고 메뉴가 작업 영역 안에 남음을 확인했다. 현재 144 DPI(150%)에서 일반·단일 기간·빈 계정의 라이트/다크, 긴 이름·오류·조회 중, 계정 다수 등 8개 메뉴 이미지를 생성했다. 단일 기간의 막대와 퍼센트가 카드 오른쪽 여백까지 확장된 화면도 확인했다. 실제 메뉴 컨트롤의 렌더링이며 Explorer 트레이 아이콘을 사람이 클릭한 수동 검사는 아니다.

공통 Claude SVG의 WinForms용 PNG는 `powershell.exe -NoProfile -File windows/scripts/make-provider-assets.ps1`로 재생성할 수 있다. 변환 시에만 WPF를 사용하며 상주 모니터에는 PNG를 포함한다.

### 빌드와 실행

| 범위 | 결과 |
| --- | --- |
| Release 빌드 | 경고 0, 오류 0 |
| 합성 통합 검사 | **30 통과, 0 실패** |
| 인증·로그인 | DPAPI 왕복·회전·삭제, 과대 입력 거부, 하드링크 거부, 격리된 가짜 CLI 성공·실패·취소·정리 |
| 계정 경계 | 워크스페이스 분리, 요청별 bearer/계정 헤더, 캐시 금지, 다른 CLI 계정으로 바뀌었을 때 기존 snapshot 보존 |
| OAuth 오류 | 동시 요청 12개를 1회 갱신으로 병합, 저장 실패 후 같은 회전 결과 재저장, 삭제 후 늦게 도착한 인증 재생성 방지, revoked 재시도 중단 |
| Claude callback | 배포 helper를 실제 PowerShell 및 Git Bash에서 실행, 공백·한글·작은따옴표 경로, 한글·이모지 stdin/stdout 바이트와 종료 코드 7 보존, 설정 복원, 사용자 수정 보존, 오래된 연결 세대 제외 |
| 로컬 통계 | 공통 fixture 기대값, 누적·복제·분기·부모 없는 분기, Claude 스트리밍 중복 제거, 약 32MiB 본문 건너뛰기, 미완성·손상 행, 파일 삭제·시간대·취소 |
| 재시작 | 별도 쓰기 프로세스 종료 후 새 읽기 프로세스에서 테마·조회 간격·트레이 모드·한글 이름·DPAPI 인증·마지막 quota 복원 |
| 트레이 | 표시 대상·기간별 최솟값, 0·100·반올림, 미조회·오류·이전 값 구분, 기존 설정 호환 및 잘못된 모드 거부 |
| 우클릭 메뉴 | 좌클릭·더블클릭 무동작, 우클릭 열기, Esc 닫기, 15개 계정 스크롤, 실시간 상태 갱신, Dashboard·종료만 유지, 단일 기간 전체 너비 |
| IPC·중복 실행 | 한 바이트씩 나눈 프레임, 한글, 크기·버전·잘린 EOF 거부, 동일 사용자 중복 실행 활성화 |
| 배포 UI | .NET 경로 설정 제거 후 self-contained 실행, Dashboard 2회 열기·닫기, WPF 화면 18개 + 트레이 메뉴 8개 이미지 생성 |
| 프로세스 수명 | 창을 닫은 후 Dashboard 종료, 모니터 WPF 미로드, 모니터 강제 종료 후 Job Object로 Dashboard 정리 |

마지막 배포 실행 검사의 `packaged-probe.json`에서 `passed=true`, `dashboardRunning=false`, `wpfLoadedInMonitor=false`, `parentCrashCleanup=true`, `selfContained=true`를 확인했다. `trayMenu`의 클릭·닫기·스크롤·갱신·명령 검사도 모두 `true`다. Dashboard 종료 후 `trayText="16"`, `trayVisibleAfterDashboardClose=true`, 현재 배율의 `trayPixelSize=24`도 확인했다. 샘플 메뉴와 창을 열고 닫은 직후 단일 관측은 private bytes 26,894,336, working set 84,353,024였다. 이는 실계정 장시간 대기 성능 측정이 아니며 macOS의 `phys_footprint`와 직접 비교하지 않는다.

최종 ZIP을 공백·한글이 있는 새 경로에 추출한 상태에서도 UI·프로세스 검사와 30개 통합 검사를 통과했다. 런타임 라이선스 포함도 확인했다. ZIP은 154,137,209 bytes이며 SHA-256은 `BBB0A0EC8AC3E3F349CC0D2CBB359EF3387762FC345E21D59F48A3A1C1E42622`이다.

로그와 화면은 로컬 `artifacts/windows/`의 `tests.log`, `packaged-probe.json`, `*-light.png`, `*-dark.png`에 생성된다. 합성 데이터만 사용하며 저장소에는 포함하지 않는다.

## Mac 보존 범위

이동 전 SHA-256과 비교해 기존 소스·리소스·테스트 **44개 파일의 내용이 모두 동일**함을 확인했다. `Package.swift`의 내용도 유지했다. `com.aiusage.lite`, Dashboard bundle ID와 기존 `AIUsage Lite` 데이터 경로는 변경하지 않았다. 빌드·테스트·callback·벤치마크 스크립트만 새 위치와 루트 `artifacts/` 경로에 맞췄으며 기존 루트 명령은 wrapper로 유지한다. Bash 문법 검사도 통과했다.

공통 로그 fixture를 읽는 `SharedContractTests.swift`를 추가했다. **이 Windows 환경에서는 Mac 빌드와 기존 77개 + 새 계약 테스트를 실행하지 않았다.** `.github/workflows/platforms.yml`에 Mac 빌드·Swift 검사·패키징된 callback과 Windows 검사·패키징을 추가했지만 원격 CI 실행은 아직 하지 않았다. Mac 내부의 `PlatformMac` 재분리는 회귀 실행 환경 확보 후 진행하도록 남겼다.

## 배포본 구성과 재현

`artifacts/QuotaBar-0.2.2-Windows-x64.zip`은 .NET 런타임을 포함한다. 압축을 풀고 루트의 `QuotaBar.Monitor.exe`를 실행한다. 기존 버전이 실행 중이면 먼저 트레이에서 종료한다. 로컬 실행 파일은 `package.ps1 -OutputFolder QuotaBar-Windows-x64-compact`로 기존 실행 폴더와 분리한 `artifacts/QuotaBar-Windows-x64-compact/QuotaBar.Monitor.exe`에 준비했다. 배포 폴더의 883개 파일이 최종 ZIP과 동일함을 SHA-256으로 확인했다.

`Dashboard/`, `StatusLine/`의 런타임 파일을 루트로 합치면 안 된다. 최초 평면 번들 검사에서 Core의 작은 `WindowsBase.dll`이 WPF 파일을 덮어써 Dashboard 시작이 실패했으며, 프로세스별 폴더 분리 후 배포 실행 검사를 통과했다. 런타임 배포 크기는 Mac 앱보다 크며 이 시제품은 아직 설치 프로그램·서명·자동 업데이트를 제공하지 않는다.

```powershell
.\windows\scripts\test.ps1
.\windows\scripts\package.ps1
$env:QUOTABAR_TEST_HELPER = Join-Path $PWD 'artifacts\QuotaBar-Windows-x64\StatusLine\QuotaBar.StatusLine.exe'
.\windows\scripts\test.ps1 -NoBuild
.\windows\scripts\smoke.ps1
```

ACL/DPAPI 검사와 Git Bash는 제한 토큰 환경에서 접근 거부가 발생할 수 있다. 검증은 일반 사용자 프로세스에서 수행했으며 보호 ACL은 완화하지 않았다. 최초 제한 네트워크 복원에서 NuGet 조회 경고가 발생했고, 정상 네트워크 권한으로 복원·검사를 다시 수행했다.

## 아직 확인하지 않은 범위

- Windows 실계정 Codex 브라우저 로그인·장시간 토큰 회전, Claude 실제 세션 callback 발생, 현재 허용되는 Gemini 조직 계정의 quota.
- 실제 절전·복귀, 여러 모니터 사이 DPI 전환, 장시간 CPU·메모리·전력 측정. 스케줄 계산과 전원 이벤트 연결은 구현했으나 수동 환경 검증은 별도다.
- Mac에서 디렉터리 이동 이후 빌드·회귀 테스트·기존 계정 업그레이드 실행.
- Windows ARM64, WSL 프로필, Authenticode 서명, 설치·업데이트 배포.

Gemini 개인용 및 조직 상품의 서비스 종료·계약 정책은 [Google 공식 안내](https://docs.cloud.google.com/gemini/docs/codeassist/sunset)를 따른다. 로컬 parser가 통과했다는 사실로 실계정 서비스 가용성을 보장하지 않는다.
