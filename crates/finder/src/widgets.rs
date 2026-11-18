//! Small custom-drawn widgets in the theme's colors.

use fltk::button::{Button, CheckButton};
use fltk::draw;
use fltk::enums::{Align, Font, FrameType};
use fltk::frame::Frame;
use fltk::prelude::*;

use crate::theme::Palette;

/// A flat filled area.
pub fn fill(x: i32, y: i32, w: i32, h: i32, color: fltk::enums::Color) -> Frame {
    let mut f = Frame::new(x, y, w, h, None);
    f.set_frame(FrameType::FlatBox);
    f.set_color(color);
    f
}

pub fn label(x: i32, y: i32, w: i32, h: i32, text: &str, size: i32, bold: bool) -> Frame {
    let mut f = Frame::new(x, y, w, h, None);
    f.set_label(text);
    f.set_label_size(size);
    f.set_label_font(if bold { Font::HelveticaBold } else { Font::Helvetica });
    f.set_align(Align::Left | Align::Inside | Align::Clip);
    f
}

/// Magnifier icon.
pub fn search_icon(x: i32, y: i32, p: Palette) -> Frame {
    let mut f = Frame::new(x, y, 24, 24, None);
    f.super_draw(false);
    f.draw(move |f| {
        let (x, y) = (f.x(), f.y());
        draw::set_draw_color(p.muted);
        draw::set_line_style(draw::LineStyle::Solid, 2);
        draw::draw_circle(f64::from(x + 10), f64::from(y + 10), 7.0);
        draw::draw_line(x + 15, y + 15, x + 21, y + 21);
        draw::set_line_style(draw::LineStyle::Solid, 0);
    });
    f
}

/// A filled pill button in the accent color.
pub fn pill(x: i32, y: i32, w: i32, h: i32, text: &'static str, p: Palette) -> Button {
    let mut b = Button::new(x, y, w, h, None);
    b.clear_visible_focus();
    b.super_draw(false);
    b.draw(move |b| {
        let (x, y, w, h) = (b.x(), b.y(), b.w(), b.h());
        draw::draw_rect_fill(x, y, w, h, p.surface);
        draw::set_draw_color(p.accent);
        draw::draw_rounded_rectf(x, y, w, h, h / 2);
        draw::set_font(Font::HelveticaBold, 13);
        draw::set_draw_color(p.on_accent);
        draw::draw_text2(text, x, y, w, h, Align::Center);
    });
    b
}

/// A check box with its label after it.
pub fn check(x: i32, y: i32, w: i32, text: &str, p: Palette) -> CheckButton {
    let mut b = CheckButton::new(x, y, w, 26, None);
    b.set_label(text);
    b.set_label_size(14);
    b.clear_visible_focus();
    b.super_draw(false);
    b.draw(move |b| {
        let (x, y, w, h) = (b.x(), b.y(), b.w(), b.h());
        let s = 18;
        let (bx, by) = (x + 2, y + (h - s) / 2);
        draw::draw_rect_fill(x, y, w, h, p.surface);
        if b.value() {
            draw::set_draw_color(p.accent);
            draw::draw_rounded_rectf(bx, by, s, s, 4);
            draw::set_draw_color(p.on_accent);
            draw::set_line_style(draw::LineStyle::Solid, 2);
            draw::draw_line(bx + 4, by + 9, bx + 8, by + 13);
            draw::draw_line(bx + 8, by + 13, bx + 14, by + 5);
            draw::set_line_style(draw::LineStyle::Solid, 0);
        } else {
            draw::set_draw_color(p.border);
            draw::draw_rounded_rectf(bx, by, s, s, 4);
            draw::set_draw_color(p.surface);
            draw::draw_rounded_rectf(bx + 2, by + 2, s - 4, s - 4, 3);
        }
        draw::set_font(Font::Helvetica, b.label_size());
        draw::set_draw_color(p.text);
        draw::draw_text2(&b.label(), x + 30, y, w - 30, h, Align::Left | Align::Inside);
    });
    b
}
