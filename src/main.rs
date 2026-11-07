mod claude;
mod player;
mod store;
mod typing;
mod youtube;

use claude::{Line, Review};
use iced::widget::{
    Column, button, column, container, operation, progress_bar, rich_text, row, rule, scrollable, space, span,
    text, text_editor, text_input,
};
use iced::{Center, Color, Element, Fill, Font, Task, Theme, color};
use player::Player;
use std::time::Instant;
use store::{Lesson, Library, Stats, Video};
use typing::CharState;

const LEVELS: [&str; 4] = ["A1", "A2", "B1", "B2"];
const INPUT_ID: &str = "typing";
const GREEN: Color = color!(0x9ece6a);
const RED: Color = color!(0xf7768e);
const MUTED: Color = color!(0x737aa2);
const ACCENT: Color = color!(0x7aa2f7);
const YELLOW: Color = color!(0xe0af68);

// ---- 상태 ----

#[derive(PartialEq)]
enum Screen {
    Setup,
    Typing,
    Writing,
}

struct App {
    stats: Stats,
    library: Library,
    screen: Screen,
    level: &'static str,
    chat: bool,
    topic: String,
    lines: Vec<Line>,
    idx: usize,
    typed: String,
    line_start: Option<Instant>,
    total_secs: f64,
    total_correct: usize,
    total_chars: usize,
    editor: text_editor::Content,
    review: Option<Review>,
    /// 작업 중이면 안내 문구
    busy: Option<&'static str>,
    error: Option<String>,
    yt_url: String,
    /// 지금 치고 있는 유튜브 세트 (영상 ID, 세트 번호). AI 문장이면 None.
    cur_video: Option<(String, usize)>,
    player: Option<Player>,
    slow: bool,
}

#[derive(Debug, Clone)]
enum Msg {
    Level(&'static str),
    Chat(bool),
    Topic(String),
    Generate,
    Generated(Result<Vec<Line>, String>),
    Typed(String),
    Submit,
    Edit(text_editor::Action),
    AskReview,
    Reviewed(Result<Review, String>),
    Home,
    YtUrl(String),
    YtLoad,
    YtLoaded(Result<youtube::Transcript, String>),
    OpenSet(String, usize),
    SetReady(String, usize, Result<Vec<Line>, String>),
    OpenLesson(usize),
    DeleteLesson(usize),
    DeleteVideo(String),
    Replay,
    OpenPlayer,
    Slow(bool),
    Cookies(bool),
}

/// claude / yt-dlp 호출은 블로킹이라 별도 스레드에서 돌리고 결과만 future로 받는다.
fn off_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> impl Future<Output = T> {
    let (tx, rx) = iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    async move { rx.await.expect("worker thread panicked") }
}

fn mmss(secs: f64) -> String {
    let s = secs.max(0.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

impl App {
    fn new() -> Self {
        Self {
            stats: Stats::load(),
            library: Library::load(),
            screen: Screen::Setup,
            level: "A2",
            chat: true,
            topic: "ordering coffee".into(),
            lines: vec![],
            idx: 0,
            typed: String::new(),
            line_start: None,
            total_secs: 0.0,
            total_correct: 0,
            total_chars: 0,
            editor: text_editor::Content::new(),
            review: None,
            busy: None,
            error: None,
            yt_url: String::new(),
            cur_video: None,
            player: None,
            slow: false,
        }
    }

    fn update(&mut self, msg: Msg) -> Task<Msg> {
        match msg {
            Msg::Level(l) => self.level = l,
            Msg::Chat(c) => self.chat = c,
            Msg::Topic(t) => self.topic = t,
            Msg::Generate => {
                self.busy = Some("Claude가 문장을 쓰는 중…");
                self.error = None;
                let (level, chat, topic) = (self.level, self.chat, self.topic.clone());
                return Task::perform(
                    off_thread(move || claude::generate_lines(level, chat, &topic)),
                    Msg::Generated,
                );
            }
            Msg::Generated(res) => {
                self.busy = None;
                match res {
                    Ok(lines) => {
                        let genre = if self.chat { "채팅" } else { "소설" };
                        let title = format!("{} · {genre} · {}", self.level, self.topic.trim());
                        self.library.lessons.insert(0, Lesson { title, lines: lines.clone() });
                        self.library.save();
                        self.cur_video = None;
                        return self.begin(lines);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::Typed(s) => {
                if self.line_start.is_none() && !s.is_empty() {
                    self.line_start = Some(Instant::now());
                }
                self.typed = s;
            }
            Msg::Submit => {
                if self.typed.is_empty() {
                    self.play_current(); // 빈 칸에서 Enter = 다시 듣기
                } else if self.typed.chars().count() >= self.lines[self.idx].en.chars().count() {
                    self.finish_line();
                    if self.screen == Screen::Typing {
                        self.play_current(); // 다음 문장 재생
                    }
                }
            }
            Msg::Edit(a) => self.editor.perform(a),
            Msg::AskReview => {
                self.busy = Some("첨삭 중…");
                self.error = None;
                let (lines, written) = (self.lines.clone(), self.editor.text());
                return Task::perform(
                    off_thread(move || claude::review_writing(&lines, &written)),
                    Msg::Reviewed,
                );
            }
            Msg::Reviewed(res) => {
                self.busy = None;
                match res {
                    Ok(r) => self.review = Some(r),
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::Home => {
                self.screen = Screen::Setup;
                self.error = None;
            }
            Msg::YtUrl(u) => self.yt_url = u,
            Msg::YtLoad => {
                self.error = None;
                // 보관함에 있는 영상이면 다시 받지 않고 이어서
                if let Some(v) = youtube::video_id(&self.yt_url).and_then(|id| self.library.video(&id)) {
                    let msg = Msg::OpenSet(v.id.clone(), resume_set(v));
                    self.yt_url.clear();
                    return self.update(msg);
                }
                self.busy = Some("자막을 가져오는 중…");
                let (url, cookies) = (self.yt_url.clone(), !self.stats.no_yt_cookies);
                return Task::perform(off_thread(move || youtube::fetch_transcript(&url, cookies)), Msg::YtLoaded);
            }
            Msg::YtLoaded(res) => match res {
                Ok(t) => {
                    let chunks = youtube::chunks(&t.words, t.end, youtube::CHUNK_WORDS);
                    self.library.videos.retain(|v| v.id != t.id);
                    self.library.videos.insert(
                        0,
                        Video { id: t.id.clone(), title: t.title, sets: vec![None; chunks.len()], chunks, progress: 0 },
                    );
                    self.library.save();
                    self.yt_url.clear();
                    return self.update(Msg::OpenSet(t.id, 0));
                }
                Err(e) => {
                    self.busy = None;
                    self.error = Some(e);
                }
            },
            Msg::OpenSet(id, i) => {
                let Some(v) = self.library.video(&id) else { return Task::none() };
                if let Some(Some(lines)) = v.sets.get(i) {
                    let lines = lines.clone();
                    self.busy = None;
                    self.cur_video = Some((id, i));
                    return self.begin(lines);
                }
                let Some(chunk) = v.chunks.get(i).cloned() else { return Task::none() };
                self.busy = Some("Claude가 자막을 문장으로 정리하는 중…");
                self.error = None;
                return Task::perform(
                    off_thread(move || {
                        claude::lines_from_transcript(&chunk.text()).map(|mut lines| {
                            youtube::align(&chunk, &mut lines);
                            lines
                        })
                    }),
                    move |r| Msg::SetReady(id.clone(), i, r),
                );
            }
            Msg::SetReady(id, i, res) => {
                self.busy = None;
                match res {
                    Ok(lines) => {
                        if let Some(set) = self.library.video_mut(&id).and_then(|v| v.sets.get_mut(i)) {
                            *set = Some(lines.clone());
                        }
                        self.library.save();
                        self.cur_video = Some((id, i));
                        return self.begin(lines);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::OpenLesson(i) => {
                if let Some(l) = self.library.lessons.get(i) {
                    let lines = l.lines.clone();
                    self.cur_video = None;
                    return self.begin(lines);
                }
            }
            Msg::DeleteLesson(i) => {
                if i < self.library.lessons.len() {
                    self.library.lessons.remove(i);
                    self.library.save();
                }
            }
            Msg::DeleteVideo(id) => {
                self.library.videos.retain(|v| v.id != id);
                self.library.save();
            }
            Msg::Replay => {
                self.play_current();
                return operation::focus(INPUT_ID);
            }
            Msg::OpenPlayer => {
                match &self.player {
                    Some(p) => {
                        if let Err(e) = youtube::open_in_browser(&p.url()) {
                            self.error = Some(e);
                        }
                    }
                    None => self.play_current(), // 처음이면 서버를 띄우고 브라우저를 연다
                }
                return operation::focus(INPUT_ID);
            }
            Msg::Cookies(on) => {
                self.stats.no_yt_cookies = !on;
                self.stats.save();
            }
            Msg::Slow(s) => {
                self.slow = s;
                self.play_current();
                return operation::focus(INPUT_ID);
            }
        }
        Task::none()
    }

    /// 새 문장 묶음으로 따라치기 시작.
    fn begin(&mut self, lines: Vec<Line>) -> Task<Msg> {
        self.lines = lines;
        self.idx = 0;
        self.typed.clear();
        self.line_start = None;
        (self.total_secs, self.total_correct, self.total_chars) = (0.0, 0, 0);
        self.editor = text_editor::Content::new();
        self.review = None;
        self.error = None;
        self.screen = Screen::Typing;
        self.play_current();
        operation::focus(INPUT_ID)
    }

    /// 유튜브 세트면 현재 문장 구간을 플레이어에서 재생. 플레이어가 없으면 띄우고 브라우저로 연다.
    fn play_current(&mut self) {
        let Some((id, _)) = &self.cur_video else { return };
        let Some(line) = self.lines.get(self.idx) else { return };
        if self.player.is_none() {
            match Player::start() {
                Ok(p) => {
                    if let Err(e) = youtube::open_in_browser(&p.url()) {
                        self.error = Some(e);
                    }
                    self.player = Some(p);
                }
                Err(e) => {
                    self.error = Some(e);
                    return;
                }
            }
        }
        if let Some(p) = &self.player {
            p.play(id, line.start, line.end, if self.slow { 0.75 } else { 1.0 });
        }
    }

    fn finish_line(&mut self) {
        let target = &self.lines[self.idx].en;
        self.total_secs += self.line_start.map_or(0.0, |t| t.elapsed().as_secs_f64());
        self.total_correct += typing::count_correct(target, &self.typed);
        self.total_chars += target.chars().count();
        for w in typing::missed_words(target, &self.typed) {
            *self.stats.missed.entry(w).or_default() += 1;
        }
        self.stats.lines_typed += 1;
        self.typed.clear();
        self.line_start = None;
        self.idx += 1;
        if self.idx == self.lines.len() {
            self.stats.best_wpm = self.stats.best_wpm.max(self.wpm());
            self.screen = Screen::Writing;
            if let Some((id, i)) = &self.cur_video
                && let Some(v) = self.library.video_mut(id)
            {
                v.progress = v.progress.max(i + 1);
                self.library.save();
            }
        }
        self.stats.save();
    }

    fn wpm(&self) -> f64 {
        typing::wpm(self.total_correct, self.total_secs)
    }

    fn accuracy(&self) -> f64 {
        if self.total_chars == 0 {
            100.0
        } else {
            self.total_correct as f64 * 100.0 / self.total_chars as f64
        }
    }

    fn video(&self) -> Option<(&Video, usize)> {
        let (id, i) = self.cur_video.as_ref()?;
        Some((self.library.video(id)?, *i))
    }

    // ---- 화면 ----

    fn view(&self) -> Element<'_, Msg> {
        let body = match self.screen {
            Screen::Setup => self.setup_view(),
            Screen::Typing => self.typing_view(),
            Screen::Writing => self.writing_view(),
        };
        let mut page = column![].spacing(16);
        if let Some(b) = self.busy {
            page = page.push(banner(text(b).color(YELLOW)));
        }
        if let Some(e) = &self.error {
            page = page.push(banner(text(e).color(RED)));
        }
        page = page.push(body);
        scrollable(container(page.max_width(860)).center_x(Fill).padding(28)).into()
    }

    fn setup_view(&self) -> Element<'_, Msg> {
        let idle = self.busy.is_none();
        let header = row![
            column![
                text("typingEng").size(34).color(ACCENT),
                text("따라 치고, 직접 써보며 익히는 영어").color(MUTED),
            ]
            .spacing(4)
            .width(Fill),
            stat(self.stats.lines_typed.to_string(), "누적 줄"),
            stat(format!("{:.0}", self.stats.best_wpm), "최고 WPM"),
        ]
        .spacing(12)
        .align_y(Center);

        // 유튜브
        let yt = section(
            "유튜브로 연습",
            "영어 자막(자동 생성 포함)을 문장마다 재생 → 멈춤 → 따라 치기. 플레이어는 브라우저에 열립니다.",
            row![
                text_input("https://www.youtube.com/watch?v=…", &self.yt_url)
                    .on_input(Msg::YtUrl)
                    .on_submit_maybe(idle.then_some(Msg::YtLoad))
                    .padding(10),
                button(text("시작"))
                    .on_press_maybe((idle && !self.yt_url.trim().is_empty()).then_some(Msg::YtLoad))
                    .padding([10, 22]),
            ]
            .spacing(8),
        );
        let cookies = !self.stats.no_yt_cookies;
        let yt = column![
            yt,
            row![
                toggle("Chrome 로그인 쿠키 사용", cookies, Msg::Cookies(!cookies)),
                text("YouTube 요청 제한(429)을 피합니다. 처음 한 번 키체인 허용 창이 뜹니다.").size(12).color(MUTED),
            ]
            .spacing(10)
            .align_y(Center),
        ]
        .spacing(8);

        // AI 문장
        let levels = row(LEVELS.iter().map(|l| toggle(l, self.level == *l, Msg::Level(l)))).spacing(6);
        let genre =
            row![toggle("채팅", self.chat, Msg::Chat(true)), toggle("소설", !self.chat, Msg::Chat(false))].spacing(6);
        let topic = text_input("예: ordering coffee, a rainy day…", &self.topic)
            .on_input(Msg::Topic)
            .on_submit_maybe(idle.then_some(Msg::Generate))
            .padding(10);
        let ai = section(
            "AI 문장으로 연습",
            "레벨과 주제를 고르면 Claude가 8~12줄을 만들어 줍니다.",
            column![
                labeled("레벨", levels.into()),
                labeled("장르", genre.into()),
                labeled("주제", topic.into()),
                button(text("문장 만들기")).on_press_maybe(idle.then_some(Msg::Generate)).padding([10, 24]),
            ]
            .spacing(12),
        );

        // 보관함
        let mut items = Column::new().spacing(8);
        for v in &self.library.videos {
            let n = v.chunks.len();
            let done = v.progress.min(n);
            items = items.push(library_row(
                format!("▶ {}", v.title),
                format!("유튜브 · 세트 {done}/{n} 완료 · {}", mmss(v.chunks.last().map_or(0.0, |c| c.end))),
                if done == n { "처음부터" } else { "이어하기" },
                idle.then(|| Msg::OpenSet(v.id.clone(), resume_set(v))),
                Msg::DeleteVideo(v.id.clone()),
            ));
        }
        for (i, l) in self.library.lessons.iter().enumerate() {
            items = items.push(library_row(
                l.title.clone(),
                format!("AI 문장 · {}줄", l.lines.len()),
                "열기",
                idle.then_some(Msg::OpenLesson(i)),
                Msg::DeleteLesson(i),
            ));
        }
        let empty = self.library.videos.is_empty() && self.library.lessons.is_empty();
        let library = section(
            "보관함",
            "만든 문장과 영상은 자동 저장됩니다. 다시 열면 Claude를 부르지 않고 바로 시작합니다.",
            if empty { Element::from(text("아직 없음").color(MUTED)) } else { items.into() },
        );

        let mut missed: Vec<_> = self.stats.missed.iter().collect();
        missed.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let missed_line = if missed.is_empty() {
            "아직 없음".to_string()
        } else {
            missed.iter().take(15).map(|(w, n)| format!("{w} ×{n}")).collect::<Vec<_>>().join("   ")
        };
        let record = section("자주 틀린 단어", "", text(missed_line).font(Font::MONOSPACE).size(14));

        column![header, yt, ai, library, record].spacing(16).into()
    }

    fn typing_view(&self) -> Element<'_, Msg> {
        let line = &self.lines[self.idx];
        let target = &line.en;
        let video = self.video();

        let title = match video {
            Some((v, i)) => format!("{} · 세트 {}/{}", v.title, i + 1, v.chunks.len()),
            None => "AI 문장".to_string(),
        };
        let top = row![
            text(title).color(MUTED).width(Fill),
            text(format!("{} / {}", self.idx + 1, self.lines.len())).color(ACCENT),
            button(text("그만하기").size(13)).style(button::text).on_press(Msg::Home),
        ]
        .spacing(12)
        .align_y(Center);
        let progress = progress_bar(0.0..=self.lines.len() as f32, self.idx as f32).girth(6);

        let done = Column::with_children(self.lines[..self.idx].iter().map(|l| {
            row![
                text("✓").color(GREEN).size(14),
                text(format!("{}{}", l.who(), l.en)).font(Font::MONOSPACE).size(14).color(MUTED),
            ]
            .spacing(8)
            .into()
        }))
        .spacing(4);

        let spans: Vec<_> = target
            .chars()
            .zip(typing::char_states(target, &self.typed))
            .map(|(c, s)| {
                let (ch, color) = match s {
                    CharState::Correct => (c, GREEN),
                    CharState::Wrong if c == ' ' => ('_', RED),
                    CharState::Wrong => (c, RED),
                    CharState::Pending => (c, MUTED),
                };
                span::<(), Font>(ch.to_string()).color(color)
            })
            .collect();

        let input = text_input("여기에 따라 치세요", &self.typed)
            .id(INPUT_ID)
            .on_input(Msg::Typed)
            .on_submit(Msg::Submit)
            .font(Font::MONOSPACE)
            .size(24)
            .padding(14);

        let mut current = column![
            row![
                text(line.who()).font(Font::MONOSPACE).size(26).color(ACCENT),
                rich_text(spans).font(Font::MONOSPACE).size(26),
            ],
            text(&line.ko).size(15).color(MUTED),
            input,
        ]
        .spacing(14);

        if video.is_some() {
            current = current.push(
                row![
                    button(text("↻ 다시 듣기")).style(button::secondary).on_press(Msg::Replay).padding([6, 14]),
                    toggle("0.75배속", self.slow, Msg::Slow(!self.slow)),
                    button(text("플레이어 열기")).style(button::text).on_press(Msg::OpenPlayer).padding([6, 14]),
                    space::horizontal(),
                    text(format!("{} – {}", mmss(line.start), mmss(line.end))).size(13).color(MUTED),
                ]
                .spacing(8)
                .align_y(Center),
            );
        }

        let live_secs = self.total_secs + self.line_start.map_or(0.0, |t| t.elapsed().as_secs_f64());
        let live_correct = self.total_correct + typing::count_correct(target, &self.typed);
        let hint = if video.is_some() {
            "듣고 따라 친 뒤 Enter → 다음 문장 재생\n빈 칸에서 Enter → 다시 듣기"
        } else {
            "줄 끝까지 친 뒤 Enter"
        };
        let stats = row![
            stat(format!("{:.0}", typing::wpm(live_correct, live_secs)), "WPM"),
            stat(format!("{:.0}%", self.accuracy()), "정확도"),
            space::horizontal(),
            text(hint).size(13).color(MUTED),
        ]
        .spacing(12)
        .align_y(Center);

        column![top, progress, done, card(current), stats].spacing(16).into()
    }

    fn writing_view(&self) -> Element<'_, Msg> {
        let idle = self.busy.is_none();
        let summary = row![
            text("따라치기 완료").size(22).color(GREEN).width(Fill),
            stat(format!("{:.0}", self.wpm()), "WPM"),
            stat(format!("{:.0}%", self.accuracy()), "정확도"),
        ]
        .spacing(12)
        .align_y(Center);

        let prompts = Column::with_children(
            self.lines.iter().enumerate().map(|(i, l)| text(format!("{}. {}{}", i + 1, l.who(), l.ko)).size(15).into()),
        )
        .spacing(4);

        let editor = text_editor(&self.editor)
            .placeholder("영어로 써보세요…")
            .on_action(Msg::Edit)
            .font(Font::MONOSPACE)
            .height(200)
            .padding(12);

        let can = idle && !self.editor.text().trim().is_empty();
        let mut actions =
            row![button(text("첨삭 받기")).on_press_maybe(can.then_some(Msg::AskReview)).padding([10, 24])].spacing(10);
        if let Some((v, i)) = self.video()
            && i + 1 < v.chunks.len()
        {
            actions = actions.push(
                button(text(format!("다음 세트 ({}/{}) ▶", i + 2, v.chunks.len())))
                    .style(button::success)
                    .on_press_maybe(idle.then(|| Msg::OpenSet(v.id.clone(), i + 1)))
                    .padding([10, 24]),
            );
        }
        actions = actions.push(button(text("처음 화면")).style(button::secondary).on_press(Msg::Home).padding([10, 24]));

        let mut page = column![
            summary,
            section(
                "이제 직접 써보세요",
                "한국어를 보고 방금 친 영어를 떠올려 써보세요. 똑같지 않아도 됩니다. (건너뛰어도 됩니다)",
                column![prompts, editor, actions].spacing(12),
            ),
        ]
        .spacing(16);

        if let Some(r) = &self.review {
            let fixes = Column::with_children(r.errors.iter().map(|f| {
                column![
                    rich_text![
                        span::<(), Font>(&f.original).color(RED).strikethrough(true),
                        span("  →  ").color(MUTED),
                        span(&f.fix).color(GREEN),
                    ],
                    text(&f.why_ko).size(13).color(MUTED),
                ]
                .spacing(2)
                .into()
            }))
            .spacing(10);
            let model = Column::with_children(
                self.lines
                    .iter()
                    .map(|l| text(format!("{}{}", l.who(), l.en)).font(Font::MONOSPACE).size(14).color(MUTED).into()),
            )
            .spacing(2);
            page = page.push(card(
                column![
                    text("교정문").size(13).color(MUTED),
                    text(&r.corrected).color(GREEN),
                    rule::horizontal(1),
                    fixes,
                    rule::horizontal(1),
                    text(&r.feedback_ko),
                    rule::horizontal(1),
                    text("모범 답안 (원문)").size(13).color(MUTED),
                    model,
                ]
                .spacing(12),
            ));
        }
        page.into()
    }
}

/// 이어할 세트: 다 끝났으면 처음부터.
fn resume_set(v: &Video) -> usize {
    if v.progress >= v.chunks.len() { 0 } else { v.progress }
}

// ---- 작은 UI 조각 ----

fn card<'a>(content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content).padding(20).width(Fill).style(container::rounded_box).into()
}

fn banner<'a>(content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content).padding([10, 16]).width(Fill).style(container::bordered_box).into()
}

/// 제목 + 설명이 붙은 카드.
fn section<'a>(title: &'a str, desc: &'a str, content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut head = column![text(title).size(18)].spacing(4);
    if !desc.is_empty() {
        head = head.push(text(desc).size(13).color(MUTED));
    }
    card(column![head, content.into()].spacing(14))
}

/// 큰 숫자 + 작은 라벨.
fn stat(value: String, label: &str) -> Element<'_, Msg> {
    container(column![text(value).size(22).color(ACCENT), text(label).size(12).color(MUTED)].align_x(Center))
        .padding([8, 16])
        .style(container::bordered_box)
        .into()
}

fn library_row(title: String, meta: String, open: &str, on_open: Option<Msg>, on_delete: Msg) -> Element<'_, Msg> {
    container(
        row![
            column![text(title), text(meta).size(12).color(MUTED)].spacing(2).width(Fill),
            button(text(open).size(14)).on_press_maybe(on_open).padding([6, 16]),
            button(text("삭제").size(13)).style(button::text).on_press(on_delete),
        ]
        .spacing(8)
        .align_y(Center),
    )
    .padding([8, 12])
    .style(container::bordered_box)
    .into()
}

fn toggle(label: &str, on: bool, msg: Msg) -> Element<'_, Msg> {
    button(text(label))
        .style(if on { button::primary } else { button::secondary })
        .on_press(msg)
        .padding([6, 16])
        .into()
}

fn labeled<'a>(label: &'a str, content: Element<'a, Msg>) -> Element<'a, Msg> {
    row![text(label).width(48).color(MUTED), content].spacing(12).align_y(Center).into()
}

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title("typingEng")
        .theme(Theme::TokyoNight)
        .window_size((960.0, 800.0))
        .run()
}
