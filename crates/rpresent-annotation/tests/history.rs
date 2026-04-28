use rpresent_annotation::{AnnotationTool, Stroke, StrokeHistory};

#[test]
fn undo_moves_the_last_stroke_to_redo() {
    let mut history = StrokeHistory::new(64);
    history.push(Stroke::new(AnnotationTool::Pen, vec![(0, 0), (10, 10)]));
    history.push(Stroke::new(AnnotationTool::Line, vec![(10, 10), (20, 20)]));

    let undone = history.undo().unwrap();
    assert_eq!(undone.tool, AnnotationTool::Line);
    assert_eq!(history.strokes().len(), 1);
    assert_eq!(history.redo_len(), 1);
}

#[test]
fn clear_drops_strokes_and_redo_entries() {
    let mut history = StrokeHistory::new(64);
    history.push(Stroke::new(AnnotationTool::Pen, vec![(0, 0)]));
    history.undo();
    history.clear();

    assert!(history.strokes().is_empty());
    assert_eq!(history.redo_len(), 0);
}

#[test]
fn push_clears_redo_and_trims_oldest_strokes() {
    let mut history = StrokeHistory::new(2);
    history.push(Stroke::new(AnnotationTool::Pen, vec![(0, 0)]));
    history.push(Stroke::new(AnnotationTool::Line, vec![(1, 1)]));
    history.undo();
    assert_eq!(history.redo_len(), 1);

    history.push(Stroke::new(AnnotationTool::Eraser, vec![(2, 2)]));

    assert_eq!(history.redo_len(), 0);
    assert_eq!(history.strokes().len(), 2);
    assert_eq!(history.strokes()[0].tool, AnnotationTool::Pen);
    assert_eq!(history.strokes()[1].tool, AnnotationTool::Eraser);
}
