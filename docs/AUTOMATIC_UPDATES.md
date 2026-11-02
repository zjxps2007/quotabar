# 자동 업데이트

자동 업데이트는 **0.1.1부터** 지원합니다. 0.1.0에는 이 기능이 없으므로 0.1.1을 한 번 직접 설치해야 합니다. 0.2.2 프리뷰 사용자도 정식 버전으로 한 번 직접 전환해야 합니다. 버전 비교는 숫자로 수행하며 자동으로 낮은 버전을 설치하지 않습니다.

## 사용자 동작

- 설정의 **자동 업데이트**는 기본으로 켜져 있습니다. 앱 시작 15초 후, 이후 실행 중에는 하루에 한 번 GitHub 최신 정식 릴리즈를 확인합니다. 미리보기·초안 릴리즈는 제외합니다.
- 새 버전의 서명·크기·SHA-256을 확인한 뒤 디스크에 다운로드합니다. 다운로드 상태와 오류는 설정에 표시합니다.
- Dashboard가 닫혀 있을 때 설치를 시작합니다. 설정의 **설치 후 다시 시작**으로 즉시 설치할 수도 있습니다. 진행 중인 인증 갱신과 저장을 마친 뒤 종료하며, 새 인증 정보를 저장할 수 없으면 설치를 중단합니다. 계정 정보와 설정은 앱 폴더 밖의 기존 위치에 유지됩니다.
- 자동 업데이트를 끄면 자동 확인·다운로드·설치를 중지합니다. **업데이트 확인**과 수동 설치는 계속 사용할 수 있습니다. 절전 중에는 자동 작업을 중지하고, 미리보기 모드는 네트워크와 설치를 실행하지 않습니다.
- macOS는 같은 볼륨에서 앱을 원자적으로 교체합니다. Windows 설치형은 기존 사용자 설치 위치에서 조용히 설치하며 시스템을 재부팅하지 않습니다. 포터블은 기존 폴더를 교체하고 사용자가 추가한 파일을 보존합니다.
- 새 앱의 시작 확인 전까지 이전 앱을 보관합니다. 새 앱이 실행되지 않으면 복구를 시도합니다. 새 앱이 실행 중이지만 시작 확인을 보내지 않으면 실행 중인 파일을 교체하지 않고 백업을 보존합니다. 실패한 버전은 사용자가 다시 확인하기 전까지 자동 설치를 반복하지 않습니다.

앱이 쓰기 불가능한 폴더에 있거나 설치 파일이 사용 중이면 기존 앱을 유지하고 오류를 표시합니다. 업데이트 전용 서명은 배포 파일의 출처·무결성을 검증하며 Apple 공증이나 Windows Authenticode 서명을 대체하지 않습니다.

## 릴리즈 준비: 개인 키는 로컬에만 보관

앱에는 `shared/assets/UpdatePublicKey.pem`의 P-256 공개 키만 포함합니다. 개인 키는 `.tools/update-signing/private.pem`에 권한 0600으로 보관하며 Git에서 제외합니다. **GitHub 시크릿, CI 환경, 릴리즈 첨부 파일에 개인 키를 전송하지 않습니다.** 키를 잃으면 기존 클라이언트용 업데이트 서명을 만들 수 없으므로 관리자가 별도로 안전하게 백업해야 합니다. 공개 키를 임의로 교체하면 기존 macOS 클라이언트가 업데이트를 거부합니다.

1. `shared/version.json`의 버전과 빌드를 올리고 커밋합니다. 두 플랫폼의 Native platforms 작업이 같은 커밋에서 성공해야 합니다.
2. 해당 커밋의 깨끗한 체크아웃에서 두 아티팩트를 내려받습니다. 이전 빌드 파일이 섞이지 않도록 새 폴더를 사용합니다.

   ```sh
   gh run download RUN_ID --name macos-release --dir artifacts/update-input/macos
   gh run download RUN_ID --name windows-x64-release --dir artifacts/update-input/windows
   python3 scripts/sign-update.py \
     --artifacts artifacts/update-input \
     --output artifacts/update-feed \
     --private-key .tools/update-signing/private.pem
   ```

3. 서명 도구는 커밋·버전·빌드·소스 변경 여부와 각 배포 파일의 해시를 검증합니다. 모든 검사가 통과해야 `update-manifest.json`과 DER 형식 `update-manifest.sig`를 생성합니다. 개인 키는 출력하거나 복사하지 않습니다.
4. 승인된 릴리즈를 게시할 때 플랫폼 파일 세 개와 **두 업데이트 서명 파일**을 같은 `vVERSION` 릴리즈에 첨부합니다. ZIP·EXE를 서명 이후 다시 압축하거나 바꾸면 안 됩니다. 모든 파일을 초안에 올린 뒤 최신 정식 릴리즈로 공개합니다.

필수 파일 이름은 `QuotaBar-VERSION-macOS-arm64.zip`, `QuotaBar-VERSION-Windows-x64.zip`, `QuotaBar-VERSION-Windows-x64-Setup.exe`, `update-manifest.json`, `update-manifest.sig`입니다. GitHub 자산의 `sha256:` digest, 서명된 manifest의 해시, 실제 다운로드 바이트가 모두 일치해야 합니다. 서명이 없는 릴리즈는 설치하지 않습니다.

## 검증

`bash scripts/test.sh`는 macOS 버전 비교·공유 서명·변조 거부·설정/IPC·원자적 교체/복구를 검사합니다. `windows/scripts/test.ps1`은 같은 계약과 Windows 다운로드 취소·완료 전 임시 파일 정리·포터블 파일 보존·경로 이탈 거부를 검사합니다. `python3 -m unittest discover -s scripts/tests -v`는 일회용 테스트 키로 서명 도구를 검사합니다. CI에는 생산용 개인 키를 제공하지 않습니다.

별도 Windows CI 작업은 `windows/scripts/test-updater.ps1`로 두 버전의 앱과 일회용 키를 만듭니다. 실제 설치 도우미를 실행해 포터블·설치형 업그레이드, 새 앱의 시작 확인, 시작 실패 시 롤백, 설정과 사용자 파일 보존을 검사합니다. 테스트 앱과 키는 배포 아티팩트로 업로드하지 않습니다. 서명된 공유 fixture는 Git의 Windows 줄바꿈 변환에서 제외합니다.

자동 업데이트 상태는 macOS의 `~/Library/Application Support/AIUsage Lite/Updates`, Windows의 `%LOCALAPPDATA%\QuotaBar\Updates`에 기록됩니다. 실패 시 이곳의 `last-result.json`과 대상 앱 폴더 옆의 백업을 확인할 수 있습니다. 자동으로 복구하지 못한 백업은 앱을 종료한 뒤 수동 복구할 수 있도록 남깁니다.
