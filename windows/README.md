# QuotaBar for Windows

Windows 10/11 x64용 네이티브 트레이 시제품입니다. macOS 앱과 별도 구현이며 Windows ARM64 및 WSL 계정은 아직 지원하지 않습니다.

## 설치와 실행

[Windows 설치 EXE](https://github.com/zjxps2007/quotabar/releases/download/v0.2.2/QuotaBar-0.2.2-Windows-x64-Setup.exe)를 실행한 뒤 시작 메뉴에서 **QuotaBar**를 엽니다. 한국어·영어 설치 마법사를 제공하며 관리자 권한을 요청하지 않습니다. 기본 설치 위치는 `%LOCALAPPDATA%\Programs\QuotaBar`이고, 계정·설정 데이터는 별도 `%LOCALAPPDATA%\QuotaBar`에 저장합니다. .NET 런타임이 포함되어 별도 설치가 필요 없습니다.

같은 설치 파일을 다시 실행해 재설치할 수 있습니다. 업데이트나 제거 전에는 트레이에서 앱을 종료하세요. **Windows 설정 → 앱 → 설치된 앱 → QuotaBar → 제거**로 삭제할 수 있으며 계정·설정 데이터는 남습니다. Claude를 연결했다면 제거 전에 앱에서 연결을 해제하세요.

포터블 ZIP도 계속 제공합니다. ZIP 전체를 고정된 폴더에 풀고 `QuotaBar.Monitor.exe`를 실행합니다. 파일 하나만 옮기지 마세요.

`Dashboard/`와 `StatusLine/`에는 각 프로세스의 런타임이 있습니다. WPF와 기본 .NET 런타임의 동명 파일이 충돌하므로 폴더 내용을 합치지 마세요.

- 트레이 아이콘 **우클릭**: 계정별 사용량 보기와 **Dashboard 열기 · 종료**만 표시합니다. **좌클릭과 좌측 더블클릭은 동작하지 않습니다.** 새로고침·계정 관리·활동·설정은 Dashboard에서 이용합니다.
- Dashboard 창을 닫으면 화면 프로세스가 종료되고 모니터가 남습니다. 트레이의 **종료**로 앱 전체를 종료합니다.
- `QuotaBar.Monitor.exe --demo`: 실제 계정·설정·로그·네트워크에 접근하지 않는 샘플 화면.
- `QuotaBar.Monitor.exe --background`: 트레이에서 시작합니다.

현재 시제품은 Authenticode 서명과 자동 업데이트가 없습니다. 자동 시작 등록도 하지 않습니다.

## 계정 연결

**Providers & Accounts → 계정 연결**을 사용합니다.

- **Codex 브라우저 로그인**: Windows용 공식 Codex CLI의 `codex.exe`가 필요합니다. PATH 또는 공식 npm 설치 경로에서 찾습니다. 격리된 임시 `CODEX_HOME`에서 로그인하며 기존 CLI 로그인을 변경하지 않습니다. 완료된 인증은 현재 사용자 DPAPI로 보호하고 앱에서 토큰을 갱신합니다. 로그인은 취소할 수 있고 5분 후 종료됩니다.
- **Codex 파일 연결**: 기본 `%USERPROFILE%\.codex\auth.json` 또는 `CODEX_HOME`의 파일. 읽기 전용으로 연결하며 앱에서 이 파일의 토큰을 갱신하지 않습니다. CLI 로그인 계정이 달라지면 새 연결을 요구합니다.
- **Claude Code**: `%USERPROFILE%\.claude` 또는 `CLAUDE_CONFIG_DIR` 폴더. 동의 후 `settings.json`의 status line을 래핑합니다. 기존 명령의 입력·출력·종료 코드를 보존하며 연결 해제 시 원래 명령으로 복원합니다. 연결 후 Claude를 다시 열고 응답을 한 번 받아야 사용량이 표시됩니다. API/Bedrock/Vertex/Foundry 라우팅은 구독 사용량 대상이 아닙니다. Git Bash와 PowerShell을 지원합니다.
- **Gemini**: `%USERPROFILE%\.gemini\oauth_creds.json`. Code Assist 서버에서 현재 허용하는 계정만 동작합니다. 개인용 서비스 종료와 조직 계약 정책에 따라 사용할 수 없습니다. 파일 자동 갱신은 하지 않습니다.

Claude 연결 후 앱 폴더를 옮기려면 먼저 **연결 해제**하고 새 위치에서 연결하세요. 앱을 삭제할 때도 먼저 Claude 연결을 해제해야 원래 status line이 복원됩니다. 사용자가 나중에 바꾼 status line 명령은 덮어쓰지 않습니다.

연결 시 Claude의 Windows 셸 선택 규칙에 맞춰 Git Bash 또는 PowerShell 명령을 생성합니다. Git Bash 설치·제거로 CLI 셸을 바꾼 경우 같은 설정 폴더를 다시 연결하세요. PowerShell에서는 원문 바이트를 전달하여 한글·이모지와 기존 명령의 종료 코드를 보존합니다.

## 데이터와 활동 통계

데이터는 `%LOCALAPPDATA%\QuotaBar`에 저장합니다. 인증은 현재 Windows 사용자만 복호화할 수 있고 앱 폴더는 사용자 전용 ACL로 보호합니다. 설정 파일이 손상되면 덮어쓰지 않고 시작 오류를 알립니다.

활동 통계는 Codex `sessions`, `archived_sessions`와 Claude `projects` JSONL을 읽습니다. 대화·도구 내용은 스트림에서 건너뛰고 토큰 수만 집계합니다. Dashboard를 보고 있을 때 스캔하며 다른 화면으로 이동하거나 창을 닫거나 절전으로 전환하면 중단합니다. 최대 25만 사용량 이벤트를 읽고, 월요일 기준 최근 26주를 서비스별로 표시합니다. 입력+출력에 캐시 입력을 중복해서 더하지 않습니다. 이 값은 로컬 기록이며 계정별 청구량과 동일하지 않을 수 있습니다.

## 화면 구성

macOS와 같은 흑백 테마, 앱·서비스 로고, 서비스별 색상, 둥근 카드와 메뉴 구성을 사용합니다. Windows의 기본 창 테두리와 트레이 동작은 Windows 방식으로 유지합니다.

- **Dashboard**: 모니터링·확인 필요·토큰 요약, 계정별 잔여량과 트레이 표시 스위치, 서비스별 사용 활동. 날짜 위에 마우스를 올리면 모델별 토큰을 확인할 수 있습니다.
- **Providers & Accounts**: 이름·이메일 검색, Provider·연결 상태 필터, 연결·이름 변경·일시 정지·개별 새로고침·연결 해제.
- **설정**: 시스템/라이트/다크, 작업 표시줄 아이콘, 트레이 숫자/로고, 새로고침 간격, 로컬 통계, 버전·데이터 위치. 변경은 바로 적용·저장됩니다. 미리보기 모드에서는 설정을 메모리에만 적용합니다.

트레이는 기본적으로 남은 비율을 숫자로 표시합니다. 예를 들어 `38`은 **38% 남음**이며, Dashboard에서 트레이 표시를 켠 활성 계정의 모든 기간 중 최솟값입니다. 공간 때문에 `%` 기호는 생략합니다. 마우스를 올리면 요약·계정별 잔여량을, 우클릭하면 기간별 내역을 확인할 수 있습니다. 잔여량 20% 이하와 연결 확인 필요 상태는 경고색입니다. 데이터가 없으면 `—`, 조회 오류가 있고 이전 값도 없으면 `!`로 표시하며 이전 성공 값이 있으면 경고색으로 유지합니다. 표시할 계정이 없으면 로고를 사용합니다. 설정 → 작업 표시줄 및 트레이에서 **로고만**으로 바꿀 수 있습니다.

우클릭 메뉴는 설정의 시스템/라이트/다크 테마를 따릅니다. 기간별 비율·리셋 시간을 카드 안에 표시하며, 5시간 한도 없이 주간 한도만 있는 계정처럼 기간이 하나면 카드 전체 너비를 사용합니다. 긴 계정명은 말줄임표와 툴팁으로 확인합니다. 조회 중·연결 확인 필요·이전 값 상태도 구분합니다. 계정이 많으면 메뉴의 스크롤 화살표나 키보드 방향키로 이동할 수 있고, Esc 또는 바깥 클릭으로 닫습니다. 메뉴만 열 때는 Dashboard 프로세스를 실행하지 않습니다.

숫자는 창을 닫아도 표시되고, 기존 백그라운드 조회 간격으로 갱신됩니다. Windows가 아이콘을 숨김 영역으로 보낸 경우 작업 표시줄 설정에서 QuotaBar를 표시하거나 트레이 밖으로 끌어내세요.

## 소스에서 빌드

.NET 10 SDK와 PowerShell 7을 사용합니다. `windows/global.json`의 SDK 버전을 준비하세요. 개발 스크립트는 저장소 `.tools/dotnet`이 있으면 우선 사용하고, 없으면 PATH의 SDK를 사용합니다.

```powershell
.\windows\scripts\build.ps1
.\windows\scripts\test.ps1
.\windows\scripts\probe.ps1
.\windows\scripts\run.ps1 -Demo
.\windows\scripts\package.ps1
.\windows\scripts\package-installer.ps1
.\windows\scripts\test-installer.ps1
```

`package-installer.ps1`은 ZIP을 사용자별 설치 EXE로 묶습니다. 공식 Inno Setup 7.1.0을 해시·게시자 서명 확인 후 `.tools/`에 포터블로 준비하며 시스템에 컴파일러를 등록하지 않습니다. `-PackagePath`와 `-ExpectedPackageSha256`으로 이미 배포한 ZIP을 지정할 수 있습니다. 앱 파일은 ZIP과 동일하게 유지하고 설치 프로그램의 소스 커밋은 `artifacts/windows/installer-build-info.json`에 따로 기록합니다.

`test-installer.ps1`은 격리된 한글 경로에 설치·재설치하고, 시작 메뉴·파일 해시·앱 실행을 검사한 뒤 테스트 설치를 제거합니다. 같은 사용자의 기존 QuotaBar 설치가 등록되어 있으면 중단하므로 별도 테스트 계정을 사용하세요.

기존 배포 앱이 실행 중일 때는 먼저 트레이에서 종료하거나 `package.ps1 -OutputFolder QuotaBar-Windows-x64-new`처럼 별도 출력 폴더를 지정하세요. 새 폴더의 앱을 사용하기 전에는 기존 앱을 종료해야 합니다.

보호 저장소 테스트는 현재 사용자의 일반 프로세스에서 실행해야 합니다. 제한 토큰 샌드박스에서 사용자 전용 ACL 접근이 거부되는 것은 정상이며 ACL을 완화하지 않습니다. 테스트는 합성 인증·가짜 CLI·가짜 HTTP 응답을 사용합니다. 실계정 서버 검증은 별도입니다.

프로젝트: `QuotaBar.Core`(모델·조회·집계), `QuotaBar.Windows`(Windows 인증·저장소·연결), `QuotaBar.Protocol`(IPC), `QuotaBar.Monitor`(상주 트레이), `QuotaBar.Dashboard`(일회성 WPF), `QuotaBar.StatusLine`(Claude 콜백). 모니터는 WPF를 로드하지 않습니다.
