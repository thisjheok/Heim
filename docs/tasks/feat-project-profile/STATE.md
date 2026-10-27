# STATE: 프로젝트 사양(결과물 형태·스택) (feat/project-profile)
<!-- 60줄 이하. 확인된 사실만. 작업 완료 시 /doc-gc로 정리 후 삭제. -->

## 목표
요청에 결과물 형태·스택이 있으면 그대로, 없으면 AI가 제안 → 지도 승인 때 확정 → 모든 프롬프트에 주입.
입력 폼은 만들지 않는다(사용자 결정). 계기: qtest1 "쇼핑몰 웹" 요청에 Python 코드가 나옴.

## 완료
- 2026-10-26: `profile.rs`(검증·파싱·저장·`project_profile` 명령·프롬프트 섹션), 지도 초안 profile 필수, 승인 시 `project_profiles` 확정,
  설계/티켓 분해/티켓 개발 프롬프트 주입, ADR 0002, README 쌍. (feat/ticket 버그 4440f9b는 별도 커밋)
- 2026-10-27 stack 영역화 (계약 v4): `profile.rs`의 `StackArea {area, items}`, `stack: Vec<StackArea>`.
  `deserialize_stack`이 v3 평평한 목록을 `Stack` 영역 하나로 읽음(DB의 기존 사양·초안 호환). 영역 1–10, 이름 중복 금지, 영역당 1–20개.
  `agent.rs` `CONTRACT_VERSION = 4`, 프롬프트 문자열·테스트 v4. `PROFILE_DRAFT_RULES`에 영역 규칙(이름은 AI 자유).
- 2026-10-27 UI: 하단 작업 탭에 Stack 추가(`App.tsx` `WorkspaceTab`/`workspaceTabs`, `ProjectStack.tsx`의 `ProjectStackPanel`, `StackAreaList`).
  타입·라벨은 `app/src/data/profile.ts`. 초안 카드(`aria-label="Saved design draft"`)는 세로 섹션 + `<details>`로 재구성, 영역별 표시.
- 2026-10-27 글자 크기 기준: `index.css`의 `--text-caption` 12px(하한)/`--text-body` 13px/`--text-title` 14px, `App.css`의 12px 미만 값을 모두 토큰으로 교체. ADR 0003, AGENTS.md 규칙.
  오른쪽 패널 입력칸 bold 제거(`.draft-node-detail`, `.node-editor`, `.run-review-decision` 입력 `font-weight: 400`).
- 검증(2026-10-27): `cargo test --locked` 35개 통과, `cargo fmt --check`, `npm run lint`, `npm run build` 통과.
  **실제 Tauri UI와 Codex 실행은 아직 안 함.**

## 현재 상태
- 위 변경 전부 이 handoff 커밋에 포함. 미확인 위험: 글자 키움으로 노드 카드·파일 트리·상세 패널·탭 줄이 좁은 폭에서 넘칠 수 있음(노드 카드는 ellipsis).

## 다음 할 일
1. `cd app && npm run tauri:dev` → 화면 전반 글자 크기·넘침 확인(특히 노드 카드, 왼쪽 트리, 오른쪽 상세, 하단 탭 줄). 넘치면 폰트를 줄이지 말고 배치로 해결(ADR 0003).
2. 빈 폴더에서 "쇼핑몰 웹 만들어줘" → 초안 profile이 v4 영역 형식으로 오는지, 브라우저 프론트인지 확인 → 승인 → Stack 탭 표시 → 티켓 개발 결과 확인.
3. "프론트는 React, 백엔드는 Spring으로" 요청 → 영역 2개 이상, `source: user_specified` 확인.
4. v3 시절 확정 사양이 있는 기존 프로젝트를 열어 Stack 탭에 `Stack` 영역 하나로 보이는지 확인.

## 버린 시도
- 시작 화면 사양 입력 폼 — 사용자가 원치 않음.
- 툴바(프로젝트 이름 옆) 스택 칩 + 팝오버 — 영역이 여러 개면 좁음. Stack 탭으로 옮김.
- 노드별 기술 스택 저장 — 노드마다 반복·불일치. 프로젝트 사양의 영역으로 표현.
- 영역 이름 고정 목록 — 사용자가 AI 자유로 결정.
- 정보가 많을 때 폰트 축소 — 사용자가 강하게 거부(ADR 0003).

## 열린 질문
- profile 없는 기존 프로젝트(이미 확정 지도 있음)에서 profile을 정하는 경로가 초안 수정 요청뿐.
- 티켓 개발 프롬프트가 노드↔영역을 명시적으로 연결하지 않음(Agent 추론에 맡김). 실제 결과 보고 판단.

## 승격 후보
- (ADR 0002 v4, ADR 0003로 승격 완료)
