use std::cell::{Cell, RefCell};
use std::fs::File;
use std::io::Write;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, LayoutId,
    Pixels, Window,
};

pub const FRAME_LOG_VARIABLE: &str = "CROWBOT_FRAME_LOG";

pub struct FrameLog {
    file: RefCell<File>,
    started: Cell<Option<Instant>>,
}

impl FrameLog {
    pub fn from_environment() -> Option<Rc<Self>> {
        let path = std::env::var_os(FRAME_LOG_VARIABLE)?;
        let file = File::create(path).ok()?;
        Some(Rc::new(Self {
            file: RefCell::new(file),
            started: Cell::new(None),
        }))
    }

    pub fn start(&self) {
        self.started.set(Some(Instant::now()));
    }

    fn finish(&self) {
        if let Some(started) = self.started.take() {
            writeln!(self.file.borrow_mut(), "{}", started.elapsed().as_micros()).ok();
        }
    }
}

pub struct Timed {
    child: AnyElement,
    log: Rc<FrameLog>,
}

pub fn timed(child: AnyElement, log: Rc<FrameLog>) -> Timed {
    Timed { child, log }
}

impl Element for Timed {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
        self.log.finish();
    }
}
