// 로그인된 Claude Code CLI(`claude -p`) 또는 Codex CLI(`codex exec`)를 호출한다.
// API 키 불필요 — CLI의 로그인 계정을 그대로 쓴다. 동기(blocking) 호출이므로 UI 스레드 밖에서 부른다.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::process::Command;
use std::sync::Mutex;

/// 문장 생성·정리·첨삭을 맡길 CLI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Engine {
    #[default]
    Claude,
    Codex,
}

/// Claude는 `haiku` 별칭(항상 최신 Haiku)으로 부르므로, 실제로 풀린 모델 ID를 응답에서 읽어 둔다.
static CLAUDE_MODEL: Mutex<Option<String>> = Mutex::new(None);
const CODEX_FALLBACK: &str = "gpt-6.1-terra";

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Engine::Claude => "Claude",
            Engine::Codex => "Codex",
        }
    }

    /// 화면에 보여줄 모델 이름.
    pub fn model_label(self) -> String {
        match self {
            Engine::Claude => CLAUDE_MODEL.lock().unwrap().clone().unwrap_or_else(|| "Haiku (최신)".into()),
            Engine::Codex => codex_model(),
        }
    }
}

/// Codex가 받아 둔 모델 목록(~/.codex/models_cache.json)에서 가장 최신 Terra. 목록이 없으면 고정값.
fn codex_model() -> String {
    // 화면을 그릴 때마다 부르므로 실행 중 한 번만 읽는다.
    static MODEL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    MODEL.get_or_init(read_codex_model).clone()
}

fn read_codex_model() -> String {
    let path = std::env::home_dir().unwrap_or_default().join(".codex/models_cache.json");
    let v: Value = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    latest_terra(&v).unwrap_or_else(|| CODEX_FALLBACK.into())
}

/// priority가 작을수록 목록 위(최신).
fn latest_terra(v: &Value) -> Option<String> {
    v["models"]
        .as_array()?
        .iter()
        .filter(|m| m["visibility"] == "list" && m["slug"].as_str().is_some_and(|s| s.contains("terra")))
        .min_by_key(|m| m["priority"].as_i64().unwrap_or(i64::MAX))
        .and_then(|m| m["slug"].as_str().map(String::from))
}

const SYSTEM: &str = "You are a friendly English tutor for Korean learners. Answer only through the structured output.";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Line {
    /// 대화형일 때 화자 이름(타이핑 대상 아님). 소설이면 빈 문자열.
    #[serde(default)]
    pub speaker: String,
    pub en: String,
    pub ko: String,
    /// 유튜브 세트일 때 영상 구간(초). Claude 응답에는 없고 youtube::align이 채운다.
    #[serde(default)]
    pub start: f64,
    #[serde(default)]
    pub end: f64,
}

impl Line {
    /// "Mia: " 형태의 화자 라벨. 소설이면 빈 문자열.
    pub fn who(&self) -> String {
        if self.speaker.is_empty() { String::new() } else { format!("{}: ", self.speaker) }
    }
}

#[derive(Deserialize)]
struct Lines {
    lines: Vec<Line>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Review {
    pub corrected: String,
    pub feedback_ko: String,
    pub errors: Vec<Fix>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Fix {
    pub original: String,
    pub fix: String,
    pub why_ko: String,
}

pub fn generate_lines(engine: Engine, level: &str, chat: bool, topic: &str) -> Result<Vec<Line>, String> {
    let kind = if chat {
        "a casual chat/messenger conversation between two friends. Put the speaker's first name in `speaker` and only the spoken words in `en` (never include the name or a colon prefix in `en`)"
    } else {
        "a short, simple story told in past tense. Leave `speaker` as an empty string"
    };
    let prompt = format!(
        "Write {kind} about \"{topic}\" for a Korean learner of English at CEFR level {level}.\n\
         8 to 12 lines, one short sentence per line, natural everyday English, plain ASCII punctuation only (no curly quotes, no em dashes).\n\
         For each line give `speaker`, `en` (the English) and `ko` (a natural Korean translation of `en` only)."
    );
    call_lines(engine, &prompt)
}

/// 유튜브 자막 조각 → 문장 단위 Line (원문 단어 유지, 구두점 복원, 한국어 번역).
pub fn lines_from_transcript(engine: Engine, chunk: &str) -> Result<Vec<Line>, String> {
    let prompt = format!(
        "Below is a raw fragment of an English YouTube transcript (it may lack punctuation and start or end mid-sentence).\n\n\
         {chunk}\n\n\
         Turn it into typing practice lines for a Korean learner:\n\
         - Keep the speaker's words in the same order. Only restore punctuation and capitalization; you may drop filler like \"um\", \"uh\". Do not reorder, summarize or add words.\n\
         - One sentence per line; split very long sentences at a comma or conjunction.\n\
         - Plain ASCII punctuation only (no curly quotes, no em dashes, no music notes).\n\
         - `speaker` is an empty string. `ko` is a natural Korean translation of `en`."
    );
    call_lines(engine, &prompt)
}

fn call_lines(engine: Engine, prompt: &str) -> Result<Vec<Line>, String> {
    let schema = json!({
        "type": "object",
        "properties": {
            "lines": {"type": "array", "items": {
                "type": "object",
                "properties": {
                    "speaker": {"type": "string"},
                    "en": {"type": "string"},
                    "ko": {"type": "string"}
                },
                "required": ["speaker", "en", "ko"],
                "additionalProperties": false
            }}
        },
        "required": ["lines"],
        "additionalProperties": false
    });
    let out: Lines = call(engine, prompt, schema)?;
    if out.lines.is_empty() {
        return Err("빈 결과가 왔습니다. 다시 시도하세요.".into());
    }
    Ok(out.lines)
}

pub fn review_writing(engine: Engine, lines: &[Line], written: &str) -> Result<Review, String> {
    let ko: Vec<String> = lines.iter().map(|l| format!("{}{}", l.who(), l.ko)).collect();
    let model: Vec<String> = lines.iter().map(|l| format!("{}{}", l.who(), l.en)).collect();
    let prompt = format!(
        "A Korean learner translated these Korean lines into English.\n\n\
         Korean:\n{}\n\nReference English:\n{}\n\nLearner's English:\n{}\n\n\
         Review the learner's English. Do not require it to match the reference; accept any natural, correct English. The learner may omit speaker names; that is fine.\n\
         - `corrected`: the learner's text with minimal fixes so it is correct and natural.\n\
         - `errors`: each mistake as original fragment, fixed fragment, and a short explanation in Korean.\n\
         - `feedback_ko`: 2-3 encouraging sentences in Korean: what they did well and the one thing to focus on next.",
        ko.join("\n"),
        model.join("\n"),
        written
    );
    let schema = json!({
        "type": "object",
        "properties": {
            "corrected": {"type": "string"},
            "feedback_ko": {"type": "string"},
            "errors": {"type": "array", "items": {
                "type": "object",
                "properties": {
                    "original": {"type": "string"},
                    "fix": {"type": "string"},
                    "why_ko": {"type": "string"}
                },
                "required": ["original", "fix", "why_ko"],
                "additionalProperties": false
            }}
        },
        "required": ["corrected", "feedback_ko", "errors"],
        "additionalProperties": false
    });
    call(engine, &prompt, schema)
}

fn call<T: DeserializeOwned>(engine: Engine, prompt: &str, schema: Value) -> Result<T, String> {
    match engine {
        Engine::Claude => call_claude(prompt, schema),
        Engine::Codex => call_codex(prompt, schema),
    }
}

fn call_codex<T: DeserializeOwned>(prompt: &str, schema: Value) -> Result<T, String> {
    // 스키마와 답을 파일로 주고받는다. 호출마다 다른 이름.
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let base = std::env::temp_dir().join(format!("typing_eng_codex_{}_{nanos}", std::process::id()));
    let (schema_path, out_path) = (base.with_extension("schema.json"), base.with_extension("out.json"));
    std::fs::write(&schema_path, schema.to_string()).map_err(|e| format!("임시 파일 쓰기 실패: {e}"))?;
    let run = Command::new("codex")
        .current_dir(std::env::temp_dir())
        .args(["exec", "-m", &codex_model(), "-c", "model_reasoning_effort=\"low\""])
        // 사용자 설정(알림 훅 등)·규칙·세션 기록 없이, 읽기 전용으로
        .args(["--skip-git-repo-check", "--ephemeral", "--ignore-user-config", "--ignore-rules", "-s", "read-only"])
        .arg("--output-schema")
        .arg(&schema_path)
        .arg("-o")
        .arg(&out_path)
        .arg(format!("{SYSTEM}\n\n{prompt}"))
        .output();
    let answer = std::fs::read_to_string(&out_path);
    let _ = std::fs::remove_file(&schema_path);
    let _ = std::fs::remove_file(&out_path);
    let out = run.map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            "Codex CLI(`codex`)를 찾을 수 없습니다. 설치 후 터미널에서 `codex login`으로 로그인하세요.".to_string()
        }
        _ => format!("codex 실행 실패: {e}"),
    })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.trim().lines().rev().take(3).collect();
        return Err(format!(
            "Codex 오류: {} (터미널에서 `codex login` 상태를 확인하세요)",
            tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
        ));
    }
    let answer = answer.map_err(|_| "Codex 응답이 비어 있습니다. 다시 시도하세요.".to_string())?;
    serde_json::from_str(&answer).map_err(|e| format!("구조화 응답 형식 오류: {e}"))
}

fn call_claude<T: DeserializeOwned>(prompt: &str, schema: Value) -> Result<T, String> {
    let out = Command::new("claude")
        // 프로젝트 CLAUDE.md 등을 읽지 않도록 임시 폴더에서 실행
        .current_dir(std::env::temp_dir())
        .args(["-p", prompt, "--output-format", "json", "--json-schema", &schema.to_string()])
        .args(["--system-prompt", SYSTEM, "--tools", "", "--strict-mcp-config"])
        .args(["--model", "haiku", "--no-session-persistence", "--effort", "low"])
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                "Claude Code CLI(`claude`)를 찾을 수 없습니다. 설치 후 터미널에서 `claude`로 로그인하세요.".to_string()
            }
            _ => format!("claude 실행 실패: {e}"),
        })?;

    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        let err = String::from_utf8_lossy(&out.stderr);
        format!("claude 응답을 읽을 수 없습니다: {}", err.trim())
    })?;
    // --output-format json 은 결과 객체 하나 또는 이벤트 배열을 준다 → 마지막 "result" 이벤트를 쓴다.
    let result = match &v {
        Value::Array(events) => events.iter().rev().find(|e| e["type"] == "result").unwrap_or(&Value::Null),
        _ => &v,
    };
    if result["is_error"] == true || result.is_null() {
        let msg = result["result"].as_str().unwrap_or("알 수 없는 오류");
        return Err(format!("Claude 오류: {msg} (터미널에서 `claude` 로그인 상태를 확인하세요)"));
    }
    if let Some(m) = result["modelUsage"].as_object().and_then(|m| m.keys().next()) {
        *CLAUDE_MODEL.lock().unwrap() = Some(m.clone());
    }
    serde_json::from_value(result["structured_output"].clone())
        .map_err(|e| format!("구조화 응답 형식 오류: {e}"))
}

#[cfg(test)]
mod tests {
    use super::Engine;

    #[test]
    fn picks_latest_terra() {
        let v = serde_json::json!({"models": [
            {"slug": "gpt-6.1-sol", "visibility": "list", "priority": 1},
            {"slug": "gpt-5.6-terra", "visibility": "list", "priority": 8},
            {"slug": "gpt-6.1-terra", "visibility": "list", "priority": 3},
            {"slug": "gpt-7-terra", "visibility": "hide", "priority": 0},
        ]});
        assert_eq!(super::latest_terra(&v).as_deref(), Some("gpt-6.1-terra"));
        assert_eq!(super::latest_terra(&serde_json::Value::Null), None);
    }

    // 실제 CLI를 부른다(사용량 소모). `cargo test -- --ignored` 로만 실행.
    #[test]
    #[ignore]
    fn live_generate_codex() {
        let lines = super::generate_lines(Engine::Codex, "A2", false, "a rainy day").unwrap();
        assert!(lines.len() >= 5 && lines.iter().all(|l| !l.en.is_empty() && !l.ko.is_empty()), "{lines:?}");
    }

    #[test]
    #[ignore]
    fn live_generate() {
        let lines = super::generate_lines(Engine::Claude, "A2", true, "ordering coffee").unwrap();
        assert!(Engine::Claude.model_label().contains("haiku"), "{}", Engine::Claude.model_label());
        assert!(lines.len() >= 5, "{lines:?}");
        assert!(lines.iter().all(|l| !l.en.is_empty() && !l.ko.is_empty()));
        // 대화형: 화자 이름이 분리되고 대사에 "이름:" 접두가 남지 않아야 한다
        assert!(lines.iter().all(|l| !l.speaker.is_empty()), "{lines:?}");
        assert!(lines.iter().all(|l| !l.en.starts_with(&format!("{}:", l.speaker))), "{lines:?}");
    }
}
