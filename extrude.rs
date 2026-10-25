//! EXTRUDE (oblique projection): gives a 2D object a height by drawing it again, shifted by the
//! height along a projection angle, and joining the two with lines. The result is ordinary
//! lines, circles, arcs and polylines, so it draws, saves to DXF and plots like anything else.
//! It is a wireframe drawing of a solid, not a 3D model (no hidden lines, no orbit).

use cadcraft_doc::{EntityKind, Handle};
use cadcraft_geom::Vec2;
use serde_json::{Value, json};

use super::curves;
use super::helpers::*;
use super::machines::number;
use super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

/// Objects extruded by one command.
const MAX_OBJECTS: usize = 2000;
/// Vertices in one polyline.
const MAX_VERTICES: usize = 5000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("extrude", "Extrude", run_extrude)
            .menu(&["Draw", "Extrude"])
            .params("{handle | handles (default: the selection), height, angle? (degrees, default 45)} → {created, handles, message}")
            .interactive(|s| Ok(Box::new(ExtrudeM::new(s)))),
    ]
}

fn other(m: impl Into<String>) -> EngineError {
    EngineError::Other(m.into())
}

/// The objects that make up the extrusion of `k` by the vector `d` (the bottom stays as it is).
fn parts(k: &EntityKind, d: Vec2) -> Result<Vec<EntityKind>> {
    let mut out: Vec<EntityKind> = Vec::new();
    match k {
        EntityKind::Circle(c) => {
            let ctr = c.center.xy();
            let r = c.radius;
            let len = (d.x * d.x + d.y * d.y).sqrt();
            // The two sides of the cylinder: tangent to both circles, parallel to the shift.
            let off = Vec2::new(-d.y / len * r, d.x / len * r);
            out.push(circle(ctr + d, r));
            out.push(line(ctr + off, ctr + off + d));
            out.push(line(ctr - off, ctr - off + d));
        }
        EntityKind::Arc(a) => {
            let ctr = a.center.xy();
            out.push(EntityKind::Arc(cadcraft_doc::Arc { center: v3(ctr + d), radius: a.radius, start: a.start, end: a.end }));
            for ang in [a.start, a.end] {
                let p = Vec2::polar(ctr, a.radius, ang);
                out.push(line(p, p + d));
            }
        }
        EntityKind::Line(l) => {
            let a = l.a.xy();
            let b = l.b.xy();
            out.push(line(a + d, b + d));
            out.push(line(a, a + d));
            out.push(line(b, b + d));
        }
        EntityKind::LwPolyline(p) => {
            if p.vertices.len() > MAX_VERTICES {
                return Err(other("The polyline has too many vertices to extrude."));
            }
            let mut top = p.clone();
            for v in top.vertices.iter_mut() {
                v.p = v.p + d;
            }
            for v in &p.vertices {
                out.push(line(v.p, v.p + d));
            }
            out.push(EntityKind::LwPolyline(top));
        }
        unsupported => {
            return Err(other(format!("EXTRUDE does not support this object type ({}). Use a line, arc, circle or polyline.", unsupported.type_name())));
        }
    }
    Ok(out)
}

fn run_extrude(s: &mut Session, p: &Value) -> Result<Value> {
    let height = f64_req("extrude", p, "height")?;
    if height == 0.0 {
        return Err(bad("extrude", "`height` can't be zero"));
    }
    let angle = f64_or(p, "angle", 45.0).to_radians();
    let d = Vec2::new(height * angle.cos(), height * angle.sin());
    if !d.x.is_finite() || !d.y.is_finite() {
        return Err(bad("extrude", "`height` and `angle` must be finite numbers"));
    }
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad("extrude", "select the objects to extrude, or give `handle` / `handles`"));
    }
    if hs.len() > MAX_OBJECTS {
        return Err(bad("extrude", format!("too many objects (at most {MAX_OBJECTS})")));
    }
    let mut made: Vec<String> = Vec::new();
    for h in hs {
        let e = curves::entity(s, h)?;
        let layer = e.common.layer.clone();
        for k in parts(&e.kind, d)? {
            let nh = s.add_entity(k)?;
            let l = layer.clone();
            s.doc_mut()?.modify_entity(nh, |x| x.common.layer = l.clone())?;
            made.push(nh.hex());
        }
    }
    let message = format!("Extruded: {} objects created.", made.len());
    Ok(json!({ "created": made.len(), "handles": made, "message": message }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ph {
    Object,
    Height,
    Angle,
}

/// EXTRUDE at the command line: pick an object (or use the selection), then the height.
struct ExtrudeM {
    ph: Ph,
    objs: Vec<Handle>,
    base: Option<Vec2>,
    angle: f64,
}

impl ExtrudeM {
    fn new(s: &Session) -> Self {
        let sel = s.selection();
        let ph = if sel.is_empty() { Ph::Object } else { Ph::Height };
        ExtrudeM { ph, objs: sel, base: None, angle: 45.0 }
    }
}

impl Interactive for ExtrudeM {
    fn name(&self) -> &'static str {
        "EXTRUDE"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.ph {
            Ph::Object => Prompt::new("Select object to extrude", Accept::POINT),
            Ph::Height => Prompt::new("Specify height of extrusion", Accept::POINT_OR_NUMBER).kw(&["Angle"]).base_opt(self.base),
            Ph::Angle => Prompt::new("Specify projection angle in degrees", Accept::NUMBER),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (self.ph, i) {
            (Ph::Object, Input::Point(p)) => {
                let h = curves::pick_at(s, p).ok_or_else(|| other("*Invalid selection*"))?;
                self.objs = vec![h];
                self.base = Some(p);
                self.ph = Ph::Height;
                Ok(Step::Continue)
            }
            (Ph::Height, Input::Keyword(k)) if k == "Angle" => {
                self.ph = Ph::Angle;
                Ok(Step::Continue)
            }
            (Ph::Angle, Input::Text(t)) => {
                self.angle = number(&t).ok_or_else(|| other("Requires a number."))?;
                self.ph = Ph::Height;
                Ok(Step::Continue)
            }
            (Ph::Height, inp @ (Input::Text(_) | Input::Point(_))) => {
                let height = match inp {
                    Input::Text(t) => number(&t).ok_or_else(|| other("Requires a number."))?,
                    Input::Point(q) => match self.base {
                        Some(b) => b.dist(q),
                        None => return Err(other("Type the height as a number.")),
                    },
                    _ => return Ok(Step::Continue),
                };
                let handles: Vec<String> = self.objs.iter().map(|h| h.hex()).collect();
                let r = run_extrude(s, &json!({ "handles": handles, "height": height, "angle": self.angle }))?;
                if let Some(m) = r.get("message").and_then(Value::as_str) {
                    s.echo(m.to_string());
                }
                Ok(Step::Done)
            }
            (_, Input::Enter) => Ok(Step::Cancel),
            _ => Err(other("Invalid input.")),
        }
    }
}
