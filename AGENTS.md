# AGENTS.md
<!-- 150줄 이하 유지. 내용 대신 경로를 가리킬 것. -->

## 프로젝트 한 줄 요약
Heim — 프로젝트 지도(노드 그래프)를 설계·승인하고 Codex CLI로 노드 단위 개발을 진행하는 Tauri v2 데스크톱 앱.
(React 19 + TS + Vite 6 프론트, Rust 2024 + SQLite 백엔드. 실제 코드는 전부 `app/`.)

## 명령어
`app/`에서 (npm, `package-lock.json`):
- 설치: `npm install`
- 린트: `npm run lint` ✅
- 타입체크+빌드: `npm run build` (`tsc -b && vite build`) ✅
- 개발: `npm run tauri:dev` (전체 앱) / `npm run dev` (브라우저 전용, 백엔드 기능 없음, 포트 5180) — 미실행

`app/src-tauri/`에서:
- 포맷 검사: `cargo fmt --check` ✅
- 테스트: `cargo test --locked` ✅ (Rust 인라인 `#[cfg(test)]`, 프론트 테스트 러너 없음)

✅ = 2026-10-26 실행 확인.

## 지도
- `app/src/App.tsx` — 핵심 UI (대형 파일). 지도 타입/데이터: `app/src/data/projectMap.ts`
- `app/src-tauri/src/lib.rs` — DB, Tauri 명령, `generate_handler!` 등록
- `app/src-tauri/src/agent.rs` — Codex `app-server` 실행·세션·JSON 계약
- `app/src-tauri/src/map_proposals.rs` — 지도 변경 초안(proposal)
- `app/src-tauri/src/profile.rs` — 프로젝트 사양(결과물 형태·스택) 검증·저장·프롬프트 섹션
- `app/src-tauri/src/tickets.rs` — 기능·티켓·실행·검토 DB 로직 / UI: `app/src/TicketBoard.tsx`, 타입 `app/src/data/tickets.ts`
- `app/src-tauri/capabilities/default.json` — Tauri 권한
- `ui-test-existing-project/` — "기존 프로젝트 열기" 테스트용 픽스처 (앱 코드 아님)
- `Heim-verification-artifacts/` — 단계별 수동 검증 기록
- 설계 원칙·로드맵: `Heim-development-plan.md` (아키텍처, 단계별 현황)
- 설계 결정: `docs/decisions/`
- 진행 중 작업: `docs/tasks/<브랜치>/STATE.md`

## 규칙
- 지도 승인 상태(초안/확정)와 노드 구현 상태는 별개. 지도 변경은 반드시 초안 → 사용자 승인으로만 확정.
- Agent가 노드 상태를 자동으로 덮어쓰지 않는다.
- 노드(지도)와 티켓(개발 단위)은 분리 — 진행 상태는 티켓에만 둔다. next 티켓은 LLM 없이 순서로 결정 (docs/decisions/0001).
- 결과물 형태·스택(profile)은 지도 초안에서 정하고 승인 때 확정, 모든 Agent 프롬프트에 주입 (docs/decisions/0002).
- Agent 응답은 JSON 계약 `contractVersion: 4` 형식 유지 (`Heim-development-plan.md` 참고). 바꾸면 버전을 올린다.
- AI 제공자는 Codex CLI뿐 (API 키/.env 불필요). Heim은 자체 샌드박싱 없이 Codex 설정을 따른다.
- 커밋: `feat:` / `fix:` / `add:` + 짧은 영어 제목. 상세는 `.gjc/skills/git-commit/SKILL.md`.
- `README.md` ↔ `README.ko.md`는 쌍. 하나를 고치면 다른 쪽도 맞춘다.
- UI 글자 크기는 `index.css` 토큰(`--text-caption` 12px 하한 / `--text-body` 13px / `--text-title` 14px)만 쓴다. 12px 미만 금지. 정보가 많으면 폰트를 줄이지 말고 세로 목록, `<details>` 접기, 탭/상세 패널로 나눈다 (docs/decisions/0003).
- 새 Tauri 명령은 `lib.rs`의 `generate_handler!`에 등록하고 필요하면 capabilities도 갱신.

## 하지 말 것
- 개발 포트 5180을 한 곳만 바꾸지 말 것 — `vite.config`와 `tauri.conf.json`(CSP/devUrl)에 함께 박혀 있다.
- `app/dist/`, `app/src-tauri/target/`, `app/src-tauri/gen/`는 생성물. 편집·커밋 금지.
- `.gjc/`는 다른 에이전트 도구의 상태(추적 안 함). 건드리지 않는다.
- 루트 바이너리(`Heim.pdf`, `*.png`)를 불필요하게 갱신하지 않는다.

## 작업 방식
- 세션 시작: `/resume`, 종료 전: `/handoff`
- 넓은 탐색은 explorer 서브에이전트에게 맡긴다 (`App.tsx`, `lib.rs`, `agent.rs`는 각 1.6k~2.9k줄).
- 기각된 방식은 docs/decisions와 STATE.md의 "버린 시도"를 먼저 확인한다.
- 변경 후 최소 검증: `app/`에서 `npm run lint && npm run build`, `app/src-tauri/`에서 `cargo fmt --check && cargo test --locked`.
