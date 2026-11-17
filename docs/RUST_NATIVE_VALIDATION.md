# Rust 네이티브 구조와 호환성 검증

대상은 Windows x64와 Apple Silicon macOS입니다. Intel Mac은 검증 대상에서 제외합니다. 기준은 기존 QuotaBar v0.1.4의 기능, 화면 구성, 조작 방식입니다. 픽셀 단위의 일치는 요구하지 않습니다.

## 제품 구성

| 역할 | Rust 소스 | 제품 실행 파일 |
| --- | --- | --- |
| 상주 모니터·인증·설정·조회·업데이트 | `native/monitor_main.rs`, `native/` | Windows `QuotaBar.Monitor.exe`, Mac `QuotaBar.app/Contents/MacOS/QuotaBar` |
| Dashboard·계정·연결·설정·활동 UI | `native/ui/` | Windows `Dashboard/QuotaBar.Dashboard.exe`, Mac `Helpers/QuotaBar Dashboard.app/Contents/MacOS/QuotaBar` |
| Claude status line callback | `native/statusline_main.rs` | Windows `StatusLine/QuotaBar.StatusLine.exe`, Mac `quotabar-statusline` |

제품은 .NET 또는 SwiftUI 애플리케이션 프로세스를 시작하지 않습니다. `native/bridge`의 Rust ABI는 기존 구현의 비교 테스트에서 사용합니다. C#/WPF와 Swift/SwiftUI 소스도 비교 기준으로 유지합니다.

Windows 패키지의 이름이 같은 `.dll` 세 개는 이전 업데이터가 버전을 읽을 수 있게 하는 메타데이터 파일입니다. 메서드 정의와 진입점은 0이며 앱 구현이나 .NET 런타임을 포함하지 않습니다. Mac 패키지의 세 프로그램은 arm64 Mach-O, Windows의 세 프로그램은 x64 PE로 검사합니다.

## 완료 기준

- 계정 연결·수정·삭제, 조회·갱신, 설정 저장, Claude callback, 로그인 취소·실패·시간 초과, 업데이트·재시작·복구의 계약과 결과가 원본과 같아야 합니다.
- Dashboard·계정·연결 창·활동·설정·문서·메뉴바/트레이에서 내용, 크기, 배치와 조작 방식이 같아야 합니다. 밝은/어두운 화면, 선택·포커스·스크롤·툴팁·긴 이름·빈 상태·오류 상태를 포함합니다.
- Windows의 중복 실행은 기존 창을 열고 키보드 포커스를 복원합니다. 이는 원본 테스트에서 기대하는 동작을 유지하기로 한 결정입니다.
- 설치·재설치·제거와 자동 업데이트는 기존 사용자 데이터와 다른 파일을 보존해야 합니다.

실제 마우스·키보드·접근성 입력, IPC 명령과 창 위치/크기는 동작 검사로 비교합니다. 화면 보고서의 픽셀 차이는 진단 자료로 보존하며, 글꼴 래스터화나 합성 과정의 작은 차이로 동작 검사를 실패시키지 않습니다.

CI의 자동 화면 검사는 모든 장면의 캡처, 같은 창 크기, 내용이 그려졌는지, 8비트 채널 평균 오차가 2.5/255 이하인지 확인합니다. 이 수치만으로 화면 구성이 같다고 판단하지 않으며, 동작·위치 검사 및 실제 화면의 시각적 검토와 함께 사용합니다. 비교 실패·누락·빈 캡처·창 크기 차이는 계속 실패로 처리합니다.

## 실행

```powershell
./windows/scripts/test.ps1
cargo test --locked --workspace --all-targets
```

```sh
bash scripts/test.sh
cargo test --locked --workspace --all-targets
python3 macos/scripts/check-dashboard.py
python3 macos/scripts/compare-dashboard.py --allow-raster-differences
```

`macos/scripts/compare-dashboard.py`는 옵션 없이 실행하면 이전처럼 픽셀 완전 일치까지 요구합니다. Windows의 `--compare-dashboards`, `--compare-visible-dashboards`도 원래 픽셀 결과를 기록합니다. CI는 `scripts/ui_comparison.py`로 화면 기준을 별도 판정하며, `--strict-pixels`로 완전 일치를 다시 요구할 수 있습니다.

샘플 계정, 격리한 저장소와 로컬 HTTP 응답으로 원본과 Rust의 계약을 비교합니다. 테스트를 위해 사용자의 실제 인증 정보를 읽거나 변경하지 않습니다. 공식 배포 ZIP 비교와 비교용 진입점을 추가해 빌드한 원본 소스 비교는 보고서에서 구분합니다. Rust 의존성에 필요한 NOTICE 항목은 유지합니다.
