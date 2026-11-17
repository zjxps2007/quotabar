# Rust 네이티브 빌드와 레거시 비교 검증

main은 Windows x64와 Apple Silicon macOS용 Rust 앱을 포함합니다. Intel Mac은 검증 대상에서 제외합니다. 기준은 기존 QuotaBar v0.1.4의 기능, 화면 구성, 조작 방식이며 픽셀 완전 일치는 요구하지 않습니다.

## 소스 구분

- main: Rust 모니터·Dashboard·Claude callback, 공유 리소스, 빌드·배포 스크립트와 Rust 검사.
- [legacy](https://github.com/zjxps2007/quotabar/tree/legacy): 기존 Swift/WPF 앱. 기존 앱 파일은 보존하고 rust-validation/ 아래에 검증 완료된 Rust 이관 비교 작업 공간을 추가했습니다.
- main에는 .swift 및 .cs 소스 파일이 없습니다. 현재 검증용 Swift·C# 소스와 보조 도구는 legacy/rust-validation/에서만 가져옵니다.

| 역할 | Rust 소스 | 실행 파일 |
| --- | --- | --- |
| 상주 모니터·인증·저장·조회·업데이트 | native/monitor_main.rs, native/ | Windows QuotaBar.Monitor.exe, Mac Contents/MacOS/QuotaBar |
| Dashboard·계정·설정·활동 UI | native/ui/ | Windows Dashboard/QuotaBar.Dashboard.exe, Mac Dashboard helper |
| Claude status line callback | native/statusline_main.rs | Windows StatusLine/QuotaBar.StatusLine.exe, Mac quotabar-statusline |

Rust 앱은 .NET 또는 SwiftUI 앱 프로세스를 시작하지 않습니다. native/bridge의 ABI는 레거시 비교 검사에서 사용하며 제품에 포함하지 않습니다.

Windows의 DLL 3개는 구버전 업데이터가 버전을 읽는 메타데이터 파일입니다. ComponentVersion.csproj는 C# 소스 없이 이 파일만 생성하므로 .NET 10 SDK는 이 데이터 생성과 레거시 비교 검사에 필요합니다. DLL의 메서드와 진입점은 0입니다.

Mac 아이콘은 검증된 네이티브 배포본과 동일한 macos/assets/AppIcon-4.icns 및 AppDockIcon.png를 사용합니다. Swift 아이콘 생성 코드는 legacy의 검증 기록에 보존합니다.

## 일반 빌드와 Rust 검사

```powershell
./windows/scripts/build.ps1
cargo test --locked --workspace --all-targets
./windows/scripts/package.ps1
```

```sh
bash scripts/test.sh
bash macos/scripts/package.sh
python3 macos/scripts/check-dashboard.py
```

macOS 패키징은 Swift 소스나 Swift 패키지를 빌드하지 않습니다. 두 플랫폼의 앱 실행 파일은 모니터·Dashboard·callback 각각 하나씩이며 Windows는 x64 PE, Mac은 arm64 Mach-O로 확인합니다.

## 레거시 비교 검사

```sh
python3 scripts/prepare_legacy_validation.py
python3 macos/scripts/compare-dashboard.py --allow-raster-differences
```

```powershell
./windows/scripts/test.ps1
./windows/scripts/smoke.ps1 -VerifyForeground
```

prepare_legacy_validation.py는 shared/legacy-validation.json에 고정한 legacy 커밋과 트리 해시를 확인하고 Git에서 제외한 .tools/legacy-validation 아래에 별도 Git 복제본을 만듭니다. 레거시 검사 코드를 복원한 다음 현재 체크아웃의 Rust 소스·Cargo 설정·공유 데이터·운영체제 리소스·제품 빌드 스크립트를 덮어쓰고 모든 파일의 SHA-256을 대조합니다. Swift·C# 소스를 main의 추적 파일로 복원하지 않습니다.

CI는 별도 폴더에서 기존 Swift 109개 검사, Windows 통합 검사와 실제 창·팝오버·입력·트레이 비교를 실행합니다. 제품 빌드·설치·제거 검사는 깨끗한 현재 체크아웃을 사용하고, 비교 검사에는 여기서 만든 제품 패키지를 전달합니다. 배포 메타데이터의 source_dirty 및 소스 커밋을 확인하므로 레거시 검사 코드 복원 때문에 서명용 패키지가 수정된 소스로 표시되지 않습니다. 업데이트·복구의 임시 버전과 서명 키는 별도 검증 폴더에서만 사용합니다. 코드 제거를 이유로 기능·화면 검사 항목을 생략하지 않습니다.

로컬에서 비교 검사를 실행하려면 Swift 6 이상(macOS) 또는 .NET 10 SDK(Windows)가 필요합니다. 얕은 복제에 legacy 객체가 없으면 고정한 자료를 확인하기 위해 origin의 legacy 브랜치를 가져옵니다. --destination으로 별도 검증 폴더를 지정하거나 --script와 뒤의 인자로 보관된 검사를 직접 실행할 수 있습니다.

## 동작과 화면 기준

- 계정 연결·수정·삭제, 설정 저장, 로그인 취소·실패·시간 초과, 조회·갱신, Claude callback, 업데이트·재시작·복구의 명령과 결과가 같아야 합니다.
- Dashboard·계정·연결 창·활동·설정·문서·메뉴바/트레이의 내용·크기·배치·조작 방식을 비교합니다. 두 테마, 선택·검색 포커스·Esc·스크롤·툴팁·긴 이름·빈 상태·오류 상태를 포함합니다.
- Windows 중복 실행은 기존 창을 열고 키보드 포커스를 복원합니다.
- 설치·재설치·제거와 자동 업데이트는 사용자 데이터와 관리 대상이 아닌 파일을 보존해야 합니다.

화면 검사는 장면 누락·빈 캡처·창 크기 차이를 실패로 처리하고, 작은 래스터 차이는 별도 진단으로 남깁니다. 채널 평균 오차 2.5/255 이하 검사와 실제 입력·위치·내용 및 시각 검토를 함께 사용합니다. --strict-pixels를 쓰면 픽셀 완전 일치도 요구할 수 있습니다.

NOTICE는 현재 Rust 구현의 경로와 배포 의존성에 맞게 갱신하므로 고지 화면 비교에서는 두 렌더러에 같은 현재 NOTICE 문서를 제공합니다. 원본 UI 구현, 동작 검증과 화면 오차 기준은 유지합니다.

검사는 합성 계정과 격리한 저장소·로컬 HTTP 응답을 사용합니다. 사용자의 실제 인증 정보는 읽거나 변경하지 않습니다. Rust 및 기존 파생 코드의 저작권·출처와 의존성 NOTICE는 유지합니다.
