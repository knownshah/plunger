//! A workflow drawn as a flow: one card per step on a rail of connected dots, with what each step
//! hands to the later ones written on the connector, and the state of the last or running run shown
//! on the dots. A diamond is a step that can be skipped, a square one that can end the run.

use crate::ui::theme::palette;
use crate::workflow::{Step, StoredStep};
use eframe::egui;

/// Where a step stands in a run, for the dot on the rail.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum NodeState {
    /// Not run (or not in the run being shown).
    Idle,
    Running,
    Passed { status: Option<u16>, ms: Option<u64> },
    Skipped,
    Failed { status: Option<u16>, error: Option<String> },
}

impl NodeState {
    pub(super) fn from_stored(step: &StoredStep) -> Self {
        if step.skipped {
            NodeState::Skipped
        } else if step.ok {
            NodeState::Passed { status: step.status, ms: step.elapsed_ms }
        } else {
            NodeState::Failed { status: step.status, error: step.error.clone() }
        }
    }
}

/// One value handed on: `name` is set by step `from` (0-based) and used by the steps in `to`.
#[derive(Debug, PartialEq)]
pub(super) struct Carry {
    pub from: usize,
    pub name: String,
    pub to: Vec<usize>,
    /// `scope: run`: lives for this run only.
    pub run_only: bool,
}

/// The values steps hand on, and the names the workflow needs from outside (given to the run or already
/// set): every placeholder the steps use that no earlier step extracts.
pub(super) fn flow_of(steps: &[Step]) -> (Vec<Carry>, Vec<String>) {
    let texts: Vec<String> = steps
        .iter()
        .map(|s| {
            let mut text = serde_json::to_string(&s.request).unwrap_or_default();
            for c in [&s.skip_if, &s.stop_if].into_iter().flatten() {
                text.push(' ');
                text.push_str(c);
            }
            text
        })
        .collect();
    let used_in = |text: &str, name: &str| {
        let mut rest = text;
        while let Some(start) = rest.find("{{") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else { break };
            if crate::domain::vars::parse_placeholder(&after[..end]).name == name {
                return true;
            }
            rest = &after[end + 2..];
        }
        false
    };
    let mut carries = Vec::new();
    let mut produced: Vec<String> = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        for e in &step.request.extract {
            let name = e.name.trim().to_string();
            let to: Vec<usize> = (i + 1..steps.len()).filter(|j| used_in(&texts[*j], &name)).collect();
            carries.push(Carry { from: i, name: name.clone(), to, run_only: e.scope.as_deref() == Some("run") });
            produced.push(name);
        }
    }
    // inputs: placeholders used by a step that no earlier step produced, and that are not built in
    let mut inputs: Vec<String> = Vec::new();
    for (i, text) in texts.iter().enumerate() {
        let mut rest = text.as_str();
        while let Some(start) = rest.find("{{") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else { break };
            let p = crate::domain::vars::parse_placeholder(&after[..end]);
            let name = p.name.to_string();
            let earlier = carries.iter().any(|c| c.name == name && c.from < i);
            let builtin = name.starts_with('$') || name == "previous_status";
            if !name.is_empty() && !earlier && !builtin && !p.optional && p.default.is_none() && !inputs.contains(&name) {
                inputs.push(name);
            }
            rest = &after[end + 2..];
        }
    }
    (carries, inputs)
}

fn method_color(method: &str, p: &crate::ui::theme::Palette) -> egui::Color32 {
    match method {
        "GET" => p.ok,
        "POST" => p.amber,
        "DELETE" => p.error,
        _ => p.accent_text,
    }
}

/// Draws the flow. `states` has one entry per step (shorter is fine); `open_step` is set when a card's
/// "Open" is clicked.
pub(super) fn render(ui: &mut egui::Ui, steps: &[Step], states: &[NodeState], open_step: &mut Option<usize>) {
    let p = palette();
    let (carries, inputs) = flow_of(steps);
    let weak = ui.visuals().weak_text_color();

    if !inputs.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Needs").small().color(weak));
            for name in &inputs {
                chip(ui, &format!("{{{{{name}}}}}"), p.accent_soft, p.accent_text);
            }
        });
        ui.add_space(6.0);
    }

    const RAIL: f32 = 34.0;
    const NODE_R: f32 = 8.0;
    let mut previous_node: Option<egui::Pos2> = None;
    for (i, step) in steps.iter().enumerate() {
        let state = states.get(i).cloned().unwrap_or(NodeState::Idle);
        let top = ui.cursor().min;
        let (method, target) = super::workflows::step_title(step);
        let x0 = ui.min_rect().left();
        ui.horizontal_top(|ui| {
            ui.add_space(RAIL);
            // the card's text width: what is left, less the card's own margins and border
            let inner = (ui.available_width() - 28.0).max(160.0);
            egui::Frame::NONE
                .fill(p.card)
                .stroke(egui::Stroke::new(1.0_f32, p.border))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(10, 7))
                .show(ui, |ui| {
                  ui.vertical(|ui| {
                    ui.set_width(inner);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("{}", i + 1)).weak().small());
                        if let Some(label) = step.label.as_deref().filter(|l| !l.is_empty()) {
                            ui.label(egui::RichText::new(label).strong());
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Open").on_hover_text("Open this step as a request in a tab, to try it by hand").clicked() {
                                *open_step = Some(i);
                            }
                            match &state {
                                NodeState::Passed { status: Some(code), ms } => {
                                    crate::ui::theme::status_badge(ui, *code, "");
                                    if let Some(ms) = ms {
                                        ui.label(egui::RichText::new(format!("{ms} ms")).weak().small());
                                    }
                                }
                                NodeState::Failed { status: Some(code), .. } => {
                                    crate::ui::theme::status_badge(ui, *code, "");
                                }
                                NodeState::Skipped => {
                                    ui.label(egui::RichText::new("skipped").weak().small());
                                }
                                NodeState::Running => {
                                    ui.add(egui::Spinner::new().size(12.0));
                                }
                                _ => {}
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&method).monospace().color(method_color(&method, p)));
                        ui.add(egui::Label::new(egui::RichText::new(&target).monospace()).truncate());
                    });
                    let has_body = step.request.json.is_some() || step.request.body.is_some() || step.request.form.is_some();
                    let has_chips = step.expect_status.is_some() || step.skip_if.is_some() || step.stop_if.is_some() || has_body;
                    if has_chips {
                    ui.horizontal(|ui| {
                        if let Some(want) = &step.expect_status {
                            chip(ui, &format!("expects {}", want.describe()), p.hover, p.text);
                        }
                        if let Some(c) = &step.skip_if {
                            chip(ui, &format!("skip if {c}"), p.hover, p.amber);
                        }
                        if let Some(c) = &step.stop_if {
                            chip(ui, &format!("stop if {c}"), p.hover, p.error);
                        }
                        if has_body {
                            chip(ui, "body", p.hover, weak);
                        }
                    });
                    }
                    if let NodeState::Failed { error: Some(error), .. } = &state {
                        ui.colored_label(p.error, error);
                    }
                  });
                });
        });
        let bottom = ui.min_rect().bottom();

        // the dot, and the line down to the previous one
        let node = egui::pos2(x0 + RAIL / 2.0 - 2.0, top.y + 16.0);
        let painter = ui.painter();
        if let Some(prev) = previous_node {
            painter.line_segment([prev + egui::vec2(0.0, NODE_R), node - egui::vec2(0.0, NODE_R + 3.0)], egui::Stroke::new(2.0_f32, p.border));
            // an arrowhead into this node
            let tip = node - egui::vec2(0.0, NODE_R + 1.0);
            painter.add(egui::Shape::convex_polygon(
                vec![tip, tip + egui::vec2(-4.0, -6.0), tip + egui::vec2(4.0, -6.0)],
                p.border,
                egui::Stroke::NONE,
            ));
        }
        draw_node(painter, node, NODE_R, step, &state, p);
        previous_node = Some(node);
        let _ = bottom;

        // what this step hands on, written on the connector to the next
        let handed: Vec<&Carry> = carries.iter().filter(|c| c.from == i && !c.to.is_empty()).collect();
        if i + 1 < steps.len() {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(RAIL);
                if handed.is_empty() {
                    ui.add_space(2.0);
                    ui.allocate_space(egui::vec2(1.0, 10.0));
                }
                for c in handed {
                    let to: Vec<String> = c.to.iter().map(|j| format!("{}", j + 1)).collect();
                    let note = format!("{} feeds step {}{}", c.name, to.join(", "), if c.run_only { " (this run only)" } else { "" });
                    chip(ui, &note, p.accent_soft, p.accent_text);
                }
            });
        }
    }
}

fn chip(ui: &mut egui::Ui, text: &str, fill: egui::Color32, color: egui::Color32) {
    egui::Frame::NONE
        .fill(fill)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).small().color(color));
        });
}

/// The dot of a step: a circle (a diamond when it can be skipped, a square when it can end the run), filled
/// and marked by how the step went.
fn draw_node(painter: &egui::Painter, c: egui::Pos2, r: f32, step: &Step, state: &NodeState, p: &crate::ui::theme::Palette) {
    let (fill, ring, mark) = match state {
        NodeState::Idle => (p.card, p.border, None),
        NodeState::Running => (p.accent_soft, p.accent_text, None),
        NodeState::Passed { .. } => (p.ok, p.ok, Some('v')),
        NodeState::Skipped => (p.card, p.border, Some('-')),
        NodeState::Failed { .. } => (p.error, p.error, Some('x')),
    };
    let stroke = egui::Stroke::new(2.0_f32, ring);
    if step.stop_if.is_some() {
        painter.rect(egui::Rect::from_center_size(c, egui::vec2(r * 2.0, r * 2.0)), egui::CornerRadius::same(2), fill, stroke, egui::StrokeKind::Inside);
    } else if step.skip_if.is_some() {
        let d = r + 2.0;
        painter.add(egui::Shape::convex_polygon(
            vec![c + egui::vec2(0.0, -d), c + egui::vec2(d, 0.0), c + egui::vec2(0.0, d), c + egui::vec2(-d, 0.0)],
            fill,
            stroke,
        ));
    } else {
        painter.circle(c, r, fill, stroke);
    }
    let white = egui::Stroke::new(1.8_f32, if matches!(state, NodeState::Skipped) { ring } else { egui::Color32::WHITE });
    match mark {
        Some('v') => {
            painter.line_segment([c + egui::vec2(-3.5, 0.0), c + egui::vec2(-1.0, 3.0)], white);
            painter.line_segment([c + egui::vec2(-1.0, 3.0), c + egui::vec2(4.0, -3.0)], white);
        }
        Some('x') => {
            painter.line_segment([c + egui::vec2(-3.0, -3.0), c + egui::vec2(3.0, 3.0)], white);
            painter.line_segment([c + egui::vec2(-3.0, 3.0), c + egui::vec2(3.0, -3.0)], white);
        }
        Some('-') => {
            painter.line_segment([c + egui::vec2(-3.5, 0.0), c + egui::vec2(3.5, 0.0)], white);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(json: &str) -> Vec<Step> {
        crate::workflow::parse_steps(json).unwrap()
    }

    #[test]
    fn the_flow_names_what_each_step_hands_to_the_ones_after_it() {
        let s = steps(
            r#"[
              {"label": "log in", "url": "{{base}}/login", "json": {"u": "{{user}}"},
               "extract": [{"name": "token", "from": "json:$.token"}, {"name": "unused", "from": "status"}]},
              {"label": "who", "url": "{{base}}/me", "headers": {"Authorization": "Bearer {{token}}"},
               "extract": [{"name": "uid", "from": "json:$.id", "scope": "run"}]},
              {"label": "act", "url": "{{base}}/u/{{uid}}", "headers": {"Authorization": "Bearer {{token}}"},
               "skip_if": "{{uid}} == 0"}
            ]"#,
        );
        let (carries, inputs) = flow_of(&s);
        let find = |n: &str| carries.iter().find(|c| c.name == n).unwrap();
        assert_eq!((find("token").from, find("token").to.clone()), (0, vec![1, 2]));
        assert_eq!((find("uid").from, find("uid").to.clone(), find("uid").run_only), (1, vec![2], true));
        assert!(find("unused").to.is_empty());
        // base and user are needed from outside; token and uid are produced by earlier steps
        assert_eq!(inputs, vec!["base".to_string(), "user".to_string()]);
    }

    #[test]
    fn optional_defaulted_and_built_in_placeholders_are_not_inputs() {
        let s = steps(r#"[{"url": "{{base}}/x/{{id?}}/{{n|3}}/{{$uuid}}", "skip_if": "{{previous_status}} == 1"}]"#);
        let (_, inputs) = flow_of(&s);
        assert_eq!(inputs, vec!["base".to_string()]);
    }

    #[test]
    fn a_stored_step_becomes_a_dot_state() {
        let mut stored = StoredStep { step: 1, label: "a".into(), ok: true, skipped: false, status: Some(200), elapsed_ms: Some(9), set: vec![], error: None };
        assert_eq!(NodeState::from_stored(&stored), NodeState::Passed { status: Some(200), ms: Some(9) });
        stored.skipped = true;
        assert_eq!(NodeState::from_stored(&stored), NodeState::Skipped);
        stored.skipped = false;
        stored.ok = false;
        stored.error = Some("expected 200, got 500".into());
        assert!(matches!(NodeState::from_stored(&stored), NodeState::Failed { .. }));
    }
}
