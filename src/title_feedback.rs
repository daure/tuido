use std::{cell::RefCell, rc::Rc};

use ratatui::{Frame, layout::Rect, style::Style, text::Line, widgets::Paragraph};
use tuicore::{LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, RenderCtx, TuiNode};

use crate::{
    app::AppMsg,
    task_title::{TitleLevel, evaluate_title},
};

pub(crate) struct TitleFeedback {
    title: Rc<RefCell<String>>,
}

impl TitleFeedback {
    pub(crate) fn new(title: Rc<RefCell<String>>) -> Self {
        Self { title }
    }
}

fn level_label(level: TitleLevel) -> &'static str {
    match level {
        TitleLevel::Bad => "Bad",
        TitleLevel::Okay => "Okay",
        TitleLevel::Good => "Good",
        TitleLevel::Perfect => "Perfect",
    }
}

fn level_icon(level: TitleLevel) -> &'static str {
    match level {
        TitleLevel::Bad => "\u{f0a9f}",
        TitleLevel::Okay => "\u{f0aa1}",
        TitleLevel::Good => "\u{f0aa3}",
        TitleLevel::Perfect => "\u{f0aa5}",
    }
}

impl TuiNode<AppMsg> for TitleFeedback {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let width = evaluate_title(&self.title.borrow())
            .iter()
            .map(|check| check.label.chars().count() as u16 + 10)
            .max()
            .unwrap_or_default();
        LayoutSizeHint::content(width, 3).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, _ctx: &mut LayoutCtx) -> LayoutResult {
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, _ctx: &mut RenderCtx<'a>) {
        let theme = tuicore::theme();
        let lines = evaluate_title(&self.title.borrow()).map(|check| {
            let color = match check.level {
                TitleLevel::Bad => theme.error_fg(),
                TitleLevel::Okay => theme.warning_fg(),
                TitleLevel::Good => theme.success_fg(),
                TitleLevel::Perfect => theme.accent_fg(),
            };
            Line::styled(
                format!(
                    "{} {:<7} {}",
                    level_icon(check.level),
                    level_label(check.level),
                    check.label
                ),
                Style::default().fg(color),
            )
        });
        frame.render_widget(Paragraph::new(lines.to_vec()), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn renders_all_guidance_rows_from_current_title() {
        let title = Rc::new(RefCell::new(String::new()));
        let feedback = TitleFeedback::new(Rc::clone(&title));
        *title.borrow_mut() = "Fix login redirect".to_string();
        let measured = feedback.measure(LayoutProposal::unbounded()).preferred;
        let area = Rect::new(0, 0, measured.width, measured.height);
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();

        terminal
            .draw(|frame| feedback.render(frame, area, &mut RenderCtx::new()))
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Starts with a verb"));
        assert!(rendered.contains("No second action detected"));
        assert!(rendered.contains("3-8 words for quick scanning"));
        assert_eq!(rendered.matches("Perfect").count(), 3);
    }

    #[test]
    fn renders_semantic_label_for_each_level() {
        let title = Rc::new(RefCell::new(String::new()));
        let feedback = TitleFeedback::new(Rc::clone(&title));

        let render = |value: &str| {
            *title.borrow_mut() = value.to_string();
            let measured = feedback.measure(LayoutProposal::unbounded()).preferred;
            let mut terminal =
                Terminal::new(TestBackend::new(measured.width, measured.height)).unwrap();
            terminal
                .draw(|frame| {
                    feedback.render(
                        frame,
                        Rect::new(0, 0, measured.width, measured.height),
                        &mut RenderCtx::new(),
                    )
                })
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };

        assert!(render("").contains("Bad"));
        assert!(render("Login redirect").contains("Okay"));
        assert!(render("Login redirect").contains("Good"));
        assert!(render("Fix login redirect").contains("Perfect"));
    }
}
