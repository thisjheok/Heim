<div align="center">
  <img src="icon.png" alt="Heim 앱 아이콘" width="128" />
</div>

<p align="center"><a href="README.md">English</a> | <strong>한국어</strong></p>

# Heim

Heim은 AI 에이전트와 함께 소프트웨어를 계획하고 개발하는 데스크톱 워크스페이스입니다. 프로젝트의 아키텍처 지도, 설계 결정, 개발 단위, 에이전트 작업을 한곳에서 관리해 승인된 설계에서 범위가 명확하고 검토 가능한 구현으로 이어지도록 돕습니다.

## 주요 기능

- 유형이 지정된 컴포넌트와 관계로 프로젝트 아키텍처 지도를 만들고 편집합니다.
- AI 에이전트에게 설계 초안 생성이나 수정을 요청하고, 검토 후 명시적으로 승인해 확정 지도로 반영합니다.
- 지도 노드 하나, 목표, 관찰 가능한 완료 조건을 묶어 개발 단위를 정의합니다.
- Codex CLI의 설정된 권한과 승인 정책에 따라 개발 작업을 실행합니다.
- 검토 결정을 기록하기 전에 에이전트 보고서, 변경 파일, 검사 결과와 확인 가능한 실행 근거를 살펴봅니다.
- 프로젝트 지도, 초안, 개발 단위, 보고서와 검토 결과를 로컬 SQLite 데이터베이스에 저장합니다.

Heim은 에이전트를 별도로 격리하거나 모든 명령에 승인을 강제한다고 보장하지 않습니다. 에이전트의 접근 권한과 승인 동작은 설치된 Codex CLI 설정을 따릅니다.

## 요구 사항

- Node.js와 npm
- Rust 안정 버전 툴체인 및 Cargo
- 운영체제별 Tauri v2 빌드 사전 요구 사항 ([Tauri 사전 요구 사항](https://v2.tauri.app/start/prerequisites/))
- AI 에이전트 기능 사용 시 `PATH`에서 실행할 수 있는 Codex CLI

## 로컬에서 실행하기

Tauri 데스크톱 앱은 `app/` 디렉터리에 있습니다.

```sh
cd app
npm install
npm run tauri:dev
```

UI 개발 중 브라우저에서 프런트엔드만 실행하려면 다음 명령을 사용하세요.

```sh
cd app
npm install
npm run dev
```

브라우저용 프런트엔드에서는 Tauri 네이티브 백엔드 기능을 사용할 수 없습니다.

## 검사 및 프로덕션 빌드

`app/` 디렉터리에서 실행합니다.

```sh
npm run lint
npm run build
npm run tauri:build
```

`build`는 프런트엔드 타입을 검사하고 프로덕션 파일을 생성합니다. `tauri:build`는 먼저 프런트엔드를 빌드한 뒤 데스크톱 애플리케이션을 패키징합니다.

## 프로젝트 구조

```text
app/
  src/            React 및 TypeScript 사용자 인터페이스
  src/data/       지도 관련 프런트엔드 데이터와 타입
  src-tauri/      Tauri 데스크톱 백엔드, 에이전트 연동, SQLite 저장소
icon.png          README 상단에 표시하는 프로젝트 아이콘
```

## 라이선스

현재 지정된 라이선스가 없습니다.
