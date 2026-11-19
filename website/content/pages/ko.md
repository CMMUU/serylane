<!-- docs -->
# 시작하기와 플랫폼 지원

기기에 맞는 정식 버전을 설치하고 자신의 구독 또는 Mihomo 설정을 사용하세요. 이 문서는 주요 기능과 플랫폼 제한을 설명합니다.

## 시작하기 {#start}

구독을 추가해서 선택하거나 프로필에서 로컬 YAML을 가져옵니다. 시스템 프록시를 켜고 실제 연결을 확인합니다. 자세한 단계는 [사용 가이드](site:guide)를 참고하세요.

## 현재 기능 {#current-capabilities}

구독, 노드, 연결, 로그와 진단을 확인할 수 있습니다. 수동 노드 선택과 AI 프록시는 동시에 사용하지 않습니다. [도메인 라우팅](site:routing)은 기본적으로 꺼져 있고 앱별 프록시와 새 Codex 연동은 비활성화되어 있습니다.

## TUN 플랫폼 제한 {#tun-compatibility}

macOS는 Developer ID로 서명하고 Apple 공증을 받은 전체 설치 파일과 도우미 승인이 필요합니다. Windows는 실험 기능이라 관리자 권한이 필요합니다. Linux TUN은 미지원이며 시스템 프록시는 GNOME gsettings를 사용합니다.

## 다운로드와 아키텍처 {#download-center}

[공식 다운로드](site:home#download)에서 운영체제와 x64 또는 ARM64를 선택하세요. M 시리즈 Mac은 Apple Silicon을 선택합니다. 정식 버전은 [다운로드 센터](https://downloads.cmmuu.com/projects/serylane)를 참고하세요.

## 이전 릴리스 기록 {#history}

과거 기록은 [영어 문서](/docs/#history)나 [중국어 간체 문서](/zh/docs/#history)에 보존됩니다. 과거 기록이 현재 다운로드 가능 버전을 의미하지는 않습니다.

<!-- routing -->
# 도메인에 따라 출구 지정

코어에 도달한 요청 중 도메인이 일치하는 연결만 지정한 정책 그룹으로 보냅니다. 기본적으로 꺼져 있습니다.

## 도구와 사용자 정의 도메인 {#node-metadata}

OpenAI, Claude, Gemini 사전 설정과 직접 넣은 도메인을 사용합니다. `example.com`은 루트, `*.example.com`은 하위 도메인에만 일치합니다. 사전 설정을 꺼도 사용자 항목은 남습니다.

## 저장과 적용 {#save-apply}

현재 구독에서 출구를 정하고 초안을 저장한 뒤 켜기 또는 적용을 확인합니다. 목록에 없는 연결은 원래 규칙을 따릅니다. 출구가 사라지면 다시 선택해야 하며 직접 연결로 바뀌지 않습니다.

## 동일한 라우팅 상태 {#global-controls}

상단 버튼과 라우팅 페이지에서 동일한 상태를 제어하고 상태를 새로 고칠 수 있습니다. 이 동작은 코어를 시작하거나 시스템 프록시와 TUN을 전환하지 않습니다.

## 적용 상태 확인 {#enablement}

규칙 모드, 코어 규칙과 트래픽 진입을 확인하세요. 단순 저장 상태는 실제 라우팅을 뜻하지 않습니다. 새 연결에서 규칙과 출구를 확인하세요.

## 프록시 정지 {#proxy-routing-state}

코어를 멈추면 켜진 라우팅은 일시 정지되고 설정은 남습니다. 프록시를 다시 명시적으로 시작하면 상태를 확인합니다.

## 노드와 안정성 {#stability}

AI 프록시는 조건에 맞는 노드 1~10개를 지원하며 수동 선택과 함께 사용하지 않습니다. 노드가 하나면 예비가 없습니다. 노드 전환은 새 연결에만 적용됩니다.

## 적용 범위 {#validation-boundaries}

TLS를 복호화하거나 암호화 경로를 기준으로 삼지 않고 Codex 설정도 수정하지 않습니다. 신규 Codex 연동, 앱별 프록시, 이전 고급 안정화 전략은 꺼져 있습니다. [AI 도구 사용](site:guide#ai-tools).

<!-- verification -->
# 다운로드와 업데이트 검증

정식 배포 기록의 아키텍처, 파일명, SHA-256, 프로젝트 서명을 확인하세요. 운영체제의 서명이나 권한과는 별개입니다.

## 다운로드 위치 {#download-origin}

[공식 다운로드](site:home#download)나 [다운로드 센터](https://downloads.cmmuu.com/projects/serylane)를 이용하세요. GitHub 소스 링크는 대체 다운로드가 아닙니다.

## SHA-256 비교 {#checksums}

같은 버전의 `SHA256SUMS.txt`에서 정확한 파일명과 해시를 비교하세요. 다르면 설치하지 마세요. 아래 예시의 파일명은 실제 파일명으로 바꿉니다.

- Windows: `Get-FileHash -LiteralPath ./installer.exe -Algorithm SHA256`
- macOS: `shasum -a 256 ./installer.dmg`
- Linux: `sha256sum ./installer.AppImage`

## 업데이트 서명 {#update-signatures}

업데이트 기능에서 프로젝트 서명을 검증합니다. 수동 확인 시 일치하는 릴리스의 서명, 매니페스트, 공개 키를 사용합니다.

## 운영체제 서명 {#system-signing}

프로젝트의 서명은 Windows Authenticode, Apple Developer ID, Apple 공증을 대체하지 않습니다. 릴리스 기록을 확인하세요.

## macOS TUN 확인 {#macos-tun}

서명 및 공증이 된 전체 설치 파일을 사용하세요. 기존 도우미가 `EX_CONFIG`로 실패하면 등록 제거와 새 도우미 확인이 필요합니다. 요청 시 로그인 항목에서 승인하고 앱으로 돌아와 트래픽을 확인하세요.

## 다운로드 중단 {#update-downloads}

GitHub 직접 연결, 가속 경로, 실행 중인 Serylane 프록시, 다운로드 센터 순으로 시도합니다. 경로가 달라도 버전, 아키텍처, 크기, 해시와 서명이 정확히 일치해야 합니다.

## 설치 실패 {#update-installation}

실패한 단계, 디스크 공간, 권한을 확인하세요. 이전 내장 업데이트가 계속 실패하면 설정을 남긴 채 대응하는 정식 전체 설치 파일로 교체하세요.

## 이전 검증 기록 {#verification-history}

이전 릴리스 세부 사항은 [영어 기록](/docs/verification#verification-history)과 [중국어 간체 기록](/zh/docs/verification#verification-history)에서 확인합니다.

<!-- privacy -->
# 개인정보와 데이터의 범위

웹사이트와 다운로드 센터, 데스크톱 앱은 서로 별개입니다. 웹사이트를 읽어도 프록시를 시작하거나 Codex 설정을 바꾸지 않습니다.

## 웹사이트 동작 {#website-data}

정적 문서와 공개 릴리스 정보를 제공합니다. 계정, 분석 또는 광고 추적 기능이 없으며 구독과 구성 업로드를 요구하거나 로컬 프록시에 접속하지 않습니다.

## 브라우저 저장 {#theme-preference}

외관을 위한 `serylane-theme`만 저장합니다. 언어는 URL에서 결정되며 언어 설정을 쿠키나 브라우저 저장소에 기록하지 않습니다. 테마는 서버에 보내지 않습니다.

## 호스팅 요청 {#hosting}

호스팅 서비스는 전달과 보안을 위해 IP, 시간, 경로와 브라우저 정보 같은 기본 액세스 데이터를 처리합니다.

## 다운로드와 외부 링크 {#downloads}

공개 릴리스와 해당 파일을 검증하며 브라우저의 쿠키나 인증 헤더를 전달하지 않습니다. 파일을 제공하는 곳과 외부 사이트의 처리 범위는 별도입니다. [검증 설명](site:verification).

## 데스크톱 클라이언트 {#desktop-client}

구독과 도메인 설정은 데스크톱 앱에서 처리합니다. 도메인 라우팅은 기본적으로 꺼져 있고 TLS를 해독하지 않습니다. 구독 URL에 담긴 자격 증명을 공유하지 마세요.

## 제보 {#reporting}

[GitHub 이슈](https://github.com/CMMUU/serylane/issues)에 오류를 제보할 때 구독 주소, 비밀번호, 토큰 및 원본 설정은 제거하세요. 민감한 취약점은 비공개 연락 경로를 먼저 확인하세요.
