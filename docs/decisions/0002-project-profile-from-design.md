# 0002. 결과물 형태·기술 스택은 설계 초안에서 정하고 지도와 함께 확정
- 날짜: 2026-10-26
- 상태: 채택

## 맥락
qtest1에서 "쇼핑몰 웹을 만들어줘"라고 요청했는데 Agent가 HTML 문자열을 반환하는 Python 코드를 만들었다.
사용자는 HTML이나 React 같은 프론트 결과를 기대했다. Heim은 어떤 프롬프트에도 결과물 형태나 스택을 넣지 않았고
(노드의 `technology`는 항상 "To be decided"), 빈 폴더라 Codex가 임의로 골랐다.

## 결정
- 프로젝트 사양 `profile = {outputForm, stack[], constraints[], source, rationale}`을 둔다. 별도 입력창은 만들지 않는다.
- `stack`은 영역별 묶음 `[{area, items[]}]`이다(2026-10-27, v4). 프론트는 JS, 백엔드는 Spring처럼 영역마다 스택이 다를 수 있다. 영역 이름은 Agent가 자유롭게 정하고, 단일 스택이면 영역 하나만 둔다.
- 확정 사양은 하단 작업 영역(Chat/Tickets/Terminal 옆)의 Stack 탭에서 항상 볼 수 있다.
- 지도 초안(`map_draft`)이 profile을 필수로 낸다. 결정 순서:
  1. 사용자 요청에 형태나 기술이 있으면 그대로 쓴다 (`user_specified`).
  2. 없으면 기존 파일에서 감지한다 (`detected`).
  3. 둘 다 없으면 AI가 제안하고 이유를 쓴다 (`ai_proposed`).
- profile은 초안 승인 때 지도와 함께 확정된다(`project_profiles`). 바꾸려면 초안 수정을 채팅으로 요청한다.
  profile이 없는 초안(지도 직접 편집)은 기존 확정 profile을 유지한다.
- 확정 profile은 설계 맥락, 티켓 분해, 티켓 개발 프롬프트에 항상 들어간다. 개발 Agent는 다른 언어나 프레임워크를 들이지 않고 blocker로 보고한다.
- profile이 없는 기존 프로젝트: 개발 Agent는 작업공간에 이미 쓰인 스택을 따른다. 빈 폴더면 스택을 고르지 않고 blocked로 보고한다.
- 계약 `contractVersion: 4` (v3에서 stack을 영역별로 바꿈).

## 기각한 대안
- 시작 화면의 사양 입력 폼 — 사용자가 원하지 않음. 자연어 요청에서 읽고, 없으면 AI가 제안하는 쪽을 택했다.
- 노드별 기술 지정 — 같은 스택이 노드마다 반복되고 어긋나기 쉽다. 노드 `technology`는 메모로 유지하고 스택은 프로젝트 사양의 영역으로 표현한다.
- 영역 이름 고정 목록(Frontend/Backend/…) — 프로젝트마다 나뉘는 방식이 달라 AI 자유로 둠.
- 툴바의 스택 칩 — 영역이 여러 개면 담기 좁아서 Stack 탭으로 옮김.

## 결과
- profile이 없는 기존 프로젝트에서는 profile을 정할 경로가 초안 수정 요청뿐이다(후속 과제).
