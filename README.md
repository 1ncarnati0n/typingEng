# typingEng

**따라 치면서 익히고, 직접 써보면서 내 것으로 만드는 영어 학습 앱**

쉬운 영어 소설이나 채팅 대화를 한 줄씩 따라 타이핑하면서 표현을 손에 익히고, 이어서 한국어 뜻만 보고 같은 내용을 영어로 직접 써본 뒤 AI 첨삭을 받는 데스크톱 앱입니다.
문장 생성과 첨삭은 로그인된 **Claude Code CLI**가 맡기 때문에 API 키가 필요 없습니다.

---

## 학습 흐름

```
① 설정            ② 따라치기                    ③ 직접 쓰기
레벨·장르·주제  →  영어 한 줄씩 타이핑        →  한국어 뜻만 보고 영작
(A1~B2)           글자별 정오 표시, WPM/정확도     Claude가 교정 + 한국어 피드백
```

1. **설정** — 레벨(A1 / A2 / B1 / B2), 장르(채팅 / 소설), 주제(자유 입력)를 고르면 Claude가 8~12줄의 영어 문장과 한국어 뜻을 만듭니다.
2. **따라치기**
   - 한 줄씩 따라 칩니다. 맞은 글자는 초록, 틀린 글자는 빨강, 남은 글자는 회색으로 표시됩니다.
   - 줄 끝까지 친 뒤 Enter를 누르면 다음 줄로 넘어갑니다.
   - 채팅일 때는 화자 이름(`Mia:`)은 라벨로만 보여주고 **대사만** 타이핑합니다.
   - WPM과 정확도가 표시되고, 틀린 단어는 따로 모아둡니다.
3. **직접 쓰기**
   - 방금 친 문장의 **한국어 뜻만** 보고 영어로 다시 씁니다.
   - Claude가 교정문, 틀린 부분별 설명(`원래 표현 → 고친 표현 + 이유`), 격려 피드백을 줍니다.
   - 원문은 "모범 답안"으로 함께 보여줍니다. 원문과 똑같이 쓰지 않아도 자연스럽고 맞는 영어면 인정합니다.

누적 줄 수, 최고 WPM, **자주 틀린 단어**는 저장되어 첫 화면에 보입니다.

---

## 실행 방법

### 준비물
- Rust (edition 2024, 1.85 이상)
- [Claude Code](https://claude.com/claude-code) CLI 설치 및 로그인
  ```bash
  claude   # 처음 한 번 실행해서 로그인
  ```

### 실행
```bash
git clone https://github.com/1ncarnati0n/typingEng.git
cd typingEng
cargo run --release
```

### 테스트
```bash
cargo test                 # 타이핑 로직 단위 테스트 (사용량 없음)
cargo test -- --ignored    # 실제 claude CLI를 불러 문장 생성 확인 (Claude 사용량 소모)
```

---

## 기술 구성

| 영역 | 사용 기술 |
|---|---|
| 언어 | Rust (edition 2024) |
| GUI | [iced](https://iced.rs) 0.14 — Elm 아키텍처(State / Message / update / view), TokyoNight 테마 |
| AI | Claude Code CLI (`claude -p`) + JSON Schema 구조화 출력 |
| 직렬화 | serde, serde_json |
| 저장 | `~/.typing_eng.json` (JSON 파일) |

의존성은 `iced`, `serde`, `serde_json` 세 개뿐입니다.

### 프로젝트 구조

```
src/
├── main.rs     iced 앱: 상태, 메시지, 화면 3개(설정 / 따라치기 / 쓰기), 통계 저장
├── claude.rs   Claude CLI 호출: generate_lines(), review_writing()
└── typing.rs   순수 로직: 글자별 정오 판정, WPM, 틀린 단어 추출 + 단위 테스트
```

### Claude 연동 방식

HTTP API나 API 키 대신, 사용자가 이미 로그인해 둔 Claude Code CLI를 하위 프로세스로 실행합니다.

```bash
claude -p "<prompt>" \
  --output-format json \
  --json-schema '<JSON Schema>' \
  --system-prompt "<튜터 역할>" \
  --tools "" --strict-mcp-config \
  --no-session-persistence --effort low
```

- **구조화 출력:** `--json-schema`로 응답 형태를 고정하고, 결과 이벤트의 `structured_output` 필드를 serde로 바로 역직렬화합니다.
  - 문장 생성: `{ lines: [{ speaker, en, ko }] }`
  - 첨삭: `{ corrected, feedback_ko, errors: [{ original, fix, why_ko }] }`
- **가볍고 독립적인 호출:**
  - 도구와 MCP를 끄고, 세션 기록을 남기지 않습니다.
  - 임시 폴더에서 실행해서 다른 프로젝트의 `CLAUDE.md`를 읽지 않습니다.
- **UI가 멈추지 않게:** CLI 호출은 블로킹이므로 별도 스레드에서 실행하고, `oneshot` 채널로 결과를 받아 iced `Task`로 전달합니다.
- **오류 처리:** CLI가 없으면 설치·로그인 안내를, CLI 오류(로그인 만료 등)는 그 메시지를 화면에 보여줍니다.

### 타이핑 측정
- **글자 판정:** 바이트가 아닌 `char` 단위로 비교해서 유니코드에도 안전합니다.
- **WPM:** 표준 공식인 `(맞은 글자 수 / 5) / 분`을 씁니다.
- **정확도:** 끝낸 줄들의 `맞은 글자 / 전체 글자`입니다.
- **틀린 단어:** 틀린 글자가 하나라도 있는 단어를 소문자로, 구두점을 빼고 모아 누적합니다.

---

## 알려진 제한 / 다음 할 일

- 한글은 시스템 폰트로 대신 표시됩니다. 지금은 macOS 기준으로만 동작을 확인했습니다.
- `.app`으로 묶어 Finder에서 실행하면 `PATH`에 `claude`가 없을 수 있습니다. 지금은 터미널에서 실행하는 것을 전제로 합니다.
- 아이디어:
  - 자주 틀린 단어를 다음 문장 생성에 반영하기
  - 간격 반복(SRS) 복습
  - 내 txt 파일 불러오기
