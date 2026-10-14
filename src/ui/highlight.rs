//! Syntax highlighting for the places Plunger shows code: pasted and exported commands, request
//! fields that hold `{{variables}}`, and request bodies. Hand-written and small on purpose (no
//! highlighting engine), each function returns a `LayoutJob` that draws exactly the text it was given.

use crate::domain::model::Variable;
use crate::ui::theme::palette;
use eframe::egui::{self, text::LayoutJob, Color32, FontId, TextFormat};
use std::sync::Arc;

fn append(job: &mut LayoutJob, text: &str, font: &FontId, color: Color32) {
    if !text.is_empty() {
        job.append(text, 0.0, TextFormat { font_id: font.clone(), color, ..Default::default() });
    }
}

/// Adds `text` in `color`, with each `{{placeholder}}` in the placeholder colour.
fn append_with_placeholders(job: &mut LayoutJob, text: &str, font: &FontId, color: Color32, placeholder: Color32) {
    let mut rest = text;
    while let Some(start) = rest.find("{{{") {
        let Some(len) = rest[start..].find("}}") else { break };
        append(job, &rest[..start], font, color);
        append(job, &rest[start..start + len + 2], font, placeholder);
        rest = &rest[start + len + 2..];
    }
    append(job, rest, font, color);
}
