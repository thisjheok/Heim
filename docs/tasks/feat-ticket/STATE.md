# STATE: 노드/티켓 분리 (feat/ticket)
<!-- 60줄 이하. 확인된 사실만. 작업 완료 시 /doc-gc로 정리 후 삭제. -->

## 목표
프로젝트 지도(노드)와 개발 작업 단위(티켓)를 분리한다.
- 노드: 사용자가 프로젝트를 이해하기 위한 지도의 구성품.
- 티켓: 사용자가 dev를 지시하는 단위 = Codex 세션 단위. 무엇을 개발할지, 연관 노드(복수 가능)를 담는다.
  AI 자동 생성 + 사용자 직접 생성 둘 다 가능.
- 큰 기능을 작은 티켓들로 나누고, 하위 티켓이 모두 끝나면 큰 기능 완료를 알린다.

## 배경 (기존 방식의 문제)
- 맵 변경/노드 추가 시 next node 계산이 반복되는 버그 → next 연산 시점을 "개발 종료 시"로 바꿈.
- 이미 개발된 노드가 next로 나와서 막음.
- LLM으로 next를 연산하는 건 복잡도가 높음 → 다른 방식 필요.
- 노드 상태가 애매함: 노드1 개발 후 노드2 작업 중 노드1을 고치는 일이 생김.
  "구현 완료 노드는 next 불가" 방식은 이 현실과 맞지 않음.

## 완료 (2026-10-26)
- 1~3, 5단계 구현. 검증: lint, build, cargo fmt --check, cargo test 31개 통과. 실제 Tauri UI 수동 검증은 안 함.
- 백엔드 `app/src-tauri/src/tickets.rs`: features/tickets/ticket_nodes/ticket_runs·reports·evidence·reviews
  (스키마는 lib.rs SCHEMA, 구 development_* 테이블 DROP). CRUD·순서 이동·결정적 next·검토로 상태 전이·테스트 5개.
- agent.rs: 계약 v2(`CONTRACT_VERSION`), `start_ticket_development`(연관 노드 상세·이웃·전체 지도 요약 주입),
  `plan_feature_tickets`(ticket_plan). 추천/spec 코드 제거.
- 프론트: `TicketBoard.tsx`(하단 Tickets 탭), `data/tickets.ts`, 지도 노드 오버레이(open/active/NEXT),
  노드 상세 Tickets 탭, 툴바 `Develop next`. 기능 완료 시 알림.
- 문서: ADR 0001, Heim-development-plan.md, README 쌍, AGENTS.md.

## 현재 상태
- 4단계(완료 후 지도 변경 제안)는 기존 `changed` 이벤트 → `propose_map_update` 흐름이 그대로 동작하므로 추가 작업 없음.
- 커밋 완료. 실제 Tauri UI 수동 검증은 아직 안 함.

## 다음 할 일
1. `npm run tauri:dev`로 수동 검증: 기능 생성 → AI 분해 → 티켓 개발 → 검토 → 기능 완료 알림, 지도 변경 후 next 유지.
2. 별도 작업 후보: 프로젝트 사양(결과물 형태·기술 스택·제약)을 모든 프롬프트에 주입.
   계기: qtest1에서 "쇼핑몰 웹" 요청에 Python 코드가 나옴. 브랜치 `feat/project-profile` 제안, 범위 미확정.

## 버린 시도
- 노드 단위 개발 + 구현 완료 노드를 next에서 제외 — 노드 간 상호 수정이 생겨 상태가 맞지 않음.
- LLM으로 next node 연산 — 복잡도가 높고 맵 변경 시 재계산 반복 문제.

## 열린 질문
- 티켓 의존성(depends_on) 도입 시점.
- 앱이 실행 중 종료되면 run이 running으로 남아 티켓 편집·삭제가 막힘 (구 단위 방식도 동일했던 기존 문제).
- map_proposals는 여전히 노드 `status`를 제안함 — ADR상 메모일 뿐이라 유지했지만 정리 여부 결정 필요.

## 승격 후보
- (ADR 0001로 승격 완료)
