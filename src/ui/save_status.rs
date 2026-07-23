use std::{cell::RefCell, rc::Rc};

use ratatui::{Frame, layout::Rect, style::Style};
use tuicore::{LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, RenderCtx, TuiNode};

use crate::app::AppMsg;

#[derive(Clone)]
pub(crate) struct SaveStatusLine {
    value: Rc<RefCell<(String, bool)>>,
}

impl SaveStatusLine {
    pub(crate) fn new(error: Option<&str>) -> Self {
        let line = Self {
            value: Rc::new(RefCell::new((String::new(), false))),
        };
        line.set_error(error);
        line
    }

    pub(crate) fn set_error(&self, error: Option<&str>) {
        *self.value.borrow_mut() = match error {
            Some(error) => (error.to_string(), true),
            None => (String::new(), false),
        };
    }
}

impl TuiNode<AppMsg> for SaveStatusLine {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let value = self.value.borrow();
        let height = u16::from(!value.0.is_empty());
        LayoutSizeHint::content(value.0.chars().count() as u16, height).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, _ctx: &mut LayoutCtx) -> LayoutResult {
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, _ctx: &mut RenderCtx<'a>) {
        let value = self.value.borrow();
        let color = if value.1 {
            tuicore::theme().error_fg()
        } else {
            tuicore::theme().muted_fg()
        };
        frame.render_widget(
            ratatui::widgets::Paragraph::new(value.0.as_str()).style(Style::default().fg(color)),
            area,
        );
    }
}
