use rpresent_config::AnnotationTool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stroke {
    pub tool: AnnotationTool,
    pub points: Vec<(i32, i32)>,
}

impl Stroke {
    pub fn new(tool: AnnotationTool, points: Vec<(i32, i32)>) -> Self {
        Self { tool, points }
    }
}

#[derive(Debug, Clone)]
pub struct StrokeHistory {
    strokes: Vec<Stroke>,
    redo_stack: Vec<Stroke>,
    capacity: usize,
}

impl StrokeHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            strokes: Vec::new(),
            redo_stack: Vec::new(),
            capacity,
        }
    }

    pub fn push(&mut self, stroke: Stroke) {
        self.redo_stack.clear();
        self.strokes.push(stroke);
        if self.strokes.len() > self.capacity {
            self.strokes.remove(0);
        }
    }

    pub fn clear(&mut self) {
        self.strokes.clear();
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) -> Option<Stroke> {
        let stroke = self.strokes.pop()?;
        self.redo_stack.push(stroke.clone());
        Some(stroke)
    }

    pub fn redo(&mut self) -> Option<Stroke> {
        let stroke = self.redo_stack.pop()?;
        self.strokes.push(stroke.clone());
        Some(stroke)
    }

    pub fn strokes(&self) -> &[Stroke] {
        &self.strokes
    }

    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }
}
