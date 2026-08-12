use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardEntry, ClipboardItem, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, TextRun, UTF16Selection,
    UnderlineStyle, Window, actions, div, fill, point, prelude::*, px, relative, size,
};
use theme::ActiveTheme;
use unicode_segmentation::UnicodeSegmentation;

actions!(
    chat_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        Submit,
        Newline,
        Up,
        Down,
        Undo,
        Redo,
    ]
);

#[derive(Clone, Debug)]
pub enum InputEvent {
    Changed,
    Submitted(String),
    PastedImage {
        mime: String,
        data: std::sync::Arc<[u8]>,
    },
    PastedPaths(Vec<std::path::PathBuf>),
}

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Vec<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    presentation: InputPresentation,
    undo_stack: Vec<(SharedString, Range<usize>)>,
    redo_stack: Vec<(SharedString, Range<usize>)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputPresentation {
    #[default]
    Field,
    Search,
    Composer,
}

impl TextInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: Vec::new(),
            last_bounds: None,
            is_selecting: false,
            presentation: InputPresentation::Field,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    pub fn with_presentation(mut self, presentation: InputPresentation) -> Self {
        self.presentation = presentation;
        self
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_text("", cx);
    }

    pub fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, text, window, cx);
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.content.trim().to_string();
        cx.emit(InputEvent::Submitted(text));
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        if self.presentation == InputPresentation::Composer {
            self.replace_text_in_range(None, "\n", window, cx);
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, cx);
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((content, selection)) = self.undo_stack.pop() {
            self.redo_stack
                .push((self.content.clone(), self.selected_range.clone()));
            self.content = content;
            self.selected_range = selection;
            self.selection_reversed = false;
            self.marked_range = None;
            cx.emit(InputEvent::Changed);
            cx.notify();
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((content, selection)) = self.redo_stack.pop() {
            self.undo_stack
                .push((self.content.clone(), self.selected_range.clone()));
            self.content = content;
            self.selected_range = selection;
            self.selection_reversed = false;
            self.marked_range = None;
            cx.emit(InputEvent::Changed);
            cx.notify();
        }
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let previous = self.previous_boundary(self.cursor_offset());
            if previous == self.cursor_offset() {
                window.play_system_bell();
                return;
            }
            self.select_to(previous, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if next == self.cursor_offset() {
                window.play_system_bell();
                return;
            }
            self.select_to(next, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        if let Some(image) = item.entries.iter().find_map(|entry| match entry {
            ClipboardEntry::Image(image) => Some(image),
            _ => None,
        }) {
            cx.emit(InputEvent::PastedImage {
                mime: image.format.mime_type().to_string(),
                data: image.bytes.clone().into(),
            });
            return;
        }
        if let Some(paths) = item.entries.iter().find_map(|entry| match entry {
            ClipboardEntry::ExternalPaths(paths) => Some(paths.paths()),
            _ => None,
        }) {
            cx.emit(InputEvent::PastedPaths(paths.to_vec()));
            return;
        }
        if let Some(text) = item.text() {
            let text = if self.presentation == InputPresentation::Composer {
                text.replace("\r\n", "\n").replace('\r', "\n")
            } else {
                text.replace(['\r', '\n'], " ")
            };
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            self.copy(&Copy, window, cx);
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        let offset = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn remember_undo(&mut self) {
        self.undo_stack
            .push((self.content.clone(), self.selected_range.clone()));
        if self.undo_stack.len() > 100 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };
        if position.y < bounds.top() {
            0
        } else if position.y > bounds.bottom() {
            self.content.len()
        } else {
            let line_height = if self.last_layout.is_empty() {
                bounds.size.height
            } else {
                bounds.size.height / self.last_layout.len() as f32
            };
            let line_index = (((position.y - bounds.top()) / line_height) as usize)
                .min(self.last_layout.len().saturating_sub(1));
            let line_start = self.line_start(line_index);
            line_start
                + self.last_layout[line_index].closest_index_for_x(position.x - bounds.left())
        }
    }

    fn line_start(&self, line_index: usize) -> usize {
        self.content
            .match_indices('\n')
            .take(line_index)
            .last()
            .map_or(0, |(index, _)| index + 1)
    }

    fn cursor_line_and_offset(&self, offset: usize) -> (usize, usize) {
        let prefix = &self.content[..offset.min(self.content.len())];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
        (line, offset.saturating_sub(self.line_start(line)))
    }

    fn move_vertical(&mut self, delta: isize, cx: &mut Context<Self>) {
        let (line, offset) = self.cursor_line_and_offset(self.cursor_offset());
        let target = line.saturating_add_signed(delta);
        let line_count = self.content.split('\n').count();
        if target >= line_count || target == line {
            return;
        }
        let start = self.line_start(target);
        let len = self.content[start..]
            .find('\n')
            .unwrap_or(self.content.len() - start);
        let mut target_offset = start + offset.min(len);
        while target_offset > start && !self.content.is_char_boundary(target_offset) {
            target_offset -= 1;
        }
        self.move_to(target_offset, cx);
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for character in self.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += character.len_utf16();
            utf8 += character.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for character in self.content.chars() {
            if utf8 >= offset {
                break;
            }
            utf8 += character.len_utf8();
            utf16 += character.len_utf16();
        }
        utf16
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl EventEmitter<InputEvent> for TextInput {}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.remember_undo();
        self.content =
            (self.content[..range.start].to_owned() + new_text + &self.content[range.end..]).into();
        let cursor = range.start + new_text.len();
        self.selected_range = cursor..cursor;
        self.marked_range = None;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_marking = self.marked_range.is_some();
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        if !was_marking {
            self.remember_undo();
        }
        self.content =
            (self.content[..range.start].to_owned() + new_text + &self.content[range.end..]).into();
        self.marked_range =
            (!new_text.is_empty()).then_some(range.start..range.start + new_text.len());
        self.selected_range = new_selection
            .as_ref()
            .map(|selection| self.range_from_utf16(selection))
            .map(|selection| range.start + selection.start..range.start + selection.end)
            .unwrap_or_else(|| {
                let cursor = range.start + new_text.len();
                cursor..cursor
            });
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range);
        let (start_line, start_offset) = self.cursor_line_and_offset(range.start);
        let (end_line, end_offset) = self.cursor_line_and_offset(range.end);
        let line_height = bounds.size.height / self.last_layout.len().max(1) as f32;
        let start_layout = self.last_layout.get(start_line)?;
        let end_layout = self.last_layout.get(end_line)?;
        Some(Bounds::from_corners(
            point(
                bounds.left() + start_layout.x_for_index(start_offset),
                bounds.top() + line_height * start_line as f32,
            ),
            point(
                bounds.left() + end_layout.x_for_index(end_offset),
                bounds.top() + line_height * (end_line + 1) as f32,
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let local = bounds.localize(&point)?;
        let line_height = bounds.size.height / self.last_layout.len().max(1) as f32;
        let line_index =
            ((local.y / line_height) as usize).min(self.last_layout.len().saturating_sub(1));
        let line = self.last_layout.get(line_index)?;
        let index = self.line_start(line_index) + line.index_for_x(local.x)?;
        Some(self.offset_to_utf16(index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    lines: Vec<ShapedLine>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let line_count = self.input.read(cx).content.split('\n').count().clamp(1, 6);
        style.size.height = (window.line_height() * line_count as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();
        let colors = cx.theme().colors();
        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.to_string(), colors.text_placeholder)
        } else {
            (content.to_string(), style.color)
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let mut byte_start = 0;
        let lines = display_text
            .split('\n')
            .take(6)
            .map(|line_text| {
                let line: SharedString = line_text.to_string().into();
                let line_end = byte_start + line.len();
                let base_run = TextRun {
                    len: line.len(),
                    font: style.font(),
                    color: text_color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let runs = if let Some(marked) = input.marked_range.as_ref() {
                    let start = marked.start.clamp(byte_start, line_end) - byte_start;
                    let end = marked.end.clamp(byte_start, line_end) - byte_start;
                    vec![
                        TextRun {
                            len: start,
                            ..base_run.clone()
                        },
                        TextRun {
                            len: end.saturating_sub(start),
                            underline: Some(UnderlineStyle {
                                color: Some(base_run.color),
                                thickness: px(1.),
                                wavy: false,
                            }),
                            ..base_run.clone()
                        },
                        TextRun {
                            len: line.len().saturating_sub(end),
                            ..base_run
                        },
                    ]
                    .into_iter()
                    .filter(|run| run.len > 0)
                    .collect::<Vec<_>>()
                } else {
                    vec![base_run]
                };
                byte_start = line_end + 1;
                window
                    .text_system()
                    .shape_line(line, font_size, &runs, None)
            })
            .collect::<Vec<_>>();
        let line_height = window.line_height();
        let (cursor_line, cursor_in_line) = input.cursor_line_and_offset(cursor);
        let cursor_x = lines
            .get(cursor_line.min(lines.len().saturating_sub(1)))
            .map_or(px(0.), |line| line.x_for_index(cursor_in_line));
        let (selections, cursor) = if selected_range.is_empty() {
            (
                Vec::new(),
                Some(fill(
                    Bounds::new(
                        point(
                            bounds.left() + cursor_x,
                            bounds.top()
                                + line_height
                                    * cursor_line.min(lines.len().saturating_sub(1)) as f32,
                        ),
                        size(px(1.), line_height),
                    ),
                    colors.text_accent,
                )),
            )
        } else {
            let selections = lines
                .iter()
                .enumerate()
                .filter_map(|(line_index, line)| {
                    let start = input.line_start(line_index);
                    let len = input.content[start..]
                        .find('\n')
                        .unwrap_or(input.content.len() - start);
                    let end = start + len;
                    let selected_start = selected_range.start.clamp(start, end) - start;
                    let selected_end = selected_range.end.clamp(start, end) - start;
                    (selected_start < selected_end).then(|| {
                        fill(
                            Bounds::from_corners(
                                point(
                                    bounds.left() + line.x_for_index(selected_start),
                                    bounds.top() + line_height * line_index as f32,
                                ),
                                point(
                                    bounds.left() + line.x_for_index(selected_end),
                                    bounds.top() + line_height * (line_index + 1) as f32,
                                ),
                            ),
                            colors.element_selected.opacity(0.55),
                        )
                    })
                })
                .collect();
            (selections, None)
        };
        PrepaintState {
            lines,
            cursor,
            selections,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for selection in prepaint.selections.drain(..) {
            window.paint_quad(selection);
        }
        let line_height = window.line_height();
        for (index, line) in prepaint.lines.iter().enumerate() {
            line.paint(
                point(bounds.left(), bounds.top() + line_height * index as f32),
                line_height,
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            )
            .unwrap();
        }
        if focus.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input.update(cx, |input, _| {
            input.last_layout = std::mem::take(&mut prepaint.lines);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let is_search = self.presentation == InputPresentation::Search;
        let is_composer = self.presentation == InputPresentation::Composer;
        div()
            .key_context("ChatInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .flex()
            .items_center()
            .w_full()
            .min_h(px(if is_composer {
                44.
            } else if is_search {
                36.
            } else {
                38.
            }))
            .when(is_composer, |element| element.max_h(px(144.)))
            .px(if is_composer { px(16.) } else { px(12.) })
            .when(is_composer, |element| element.rounded_full())
            .when(!is_composer, |element| element.rounded_md())
            .border_1()
            .border_color(if is_composer {
                colors.border_variant
            } else {
                colors.border
            })
            .bg(colors.editor_background)
            .text_color(colors.text)
            .text_size(px(if is_composer { 15. } else { 14. }))
            .line_height(px(22.))
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
