//! PyO3 bindings for vobject-core.
//!
//! This module (`vobject._core`) exposes the lossless document model and the
//! strict/lenient parser and serializer. The user-facing Python API lives in
//! the pure-Python `vobject` package on top of these primitives.
//!
//! The model classes hold shared references (`Py<T>`) to their children, so
//! Python code can mutate a tree in place naturally.

use pyo3::create_exception;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use vobject_core as core;

create_exception!(
    vobject._core,
    ParseError,
    PyValueError,
    "The input could not be parsed as a vobject document."
);

/// A property parameter: a name and zero or more decoded values.
#[pyclass(module = "vobject._core")]
struct Param {
    #[pyo3(get, set)]
    name: String,
    #[pyo3(get, set)]
    values: Vec<String>,
}

#[pymethods]
impl Param {
    #[new]
    #[pyo3(signature = (name, values = Vec::new()))]
    fn new(name: String, values: Vec<String>) -> Param {
        Param { name, values }
    }

    fn __repr__(&self) -> String {
        format!("Param(name={:?}, values={:?})", self.name, self.values)
    }

    fn __eq__(&self, other: &Param) -> bool {
        self.name == other.name && self.values == other.values
    }
}

/// A single content line: group, name, parameters, and the raw value.
#[pyclass(module = "vobject._core")]
struct Property {
    #[pyo3(get, set)]
    group: Option<String>,
    #[pyo3(get, set)]
    name: String,
    #[pyo3(get, set)]
    params: Vec<Py<Param>>,
    /// The raw (escaped, unfolded) value text.
    #[pyo3(get, set)]
    value: String,
}

#[pymethods]
impl Property {
    #[new]
    #[pyo3(signature = (name, value, params = Vec::new(), group = None))]
    fn new(name: String, value: String, params: Vec<Py<Param>>, group: Option<String>) -> Property {
        Property {
            group,
            name,
            params,
            value,
        }
    }

    fn __repr__(&self, py: Python<'_>) -> String {
        let params: Vec<String> = self
            .params
            .iter()
            .map(|p| p.borrow(py).__repr__())
            .collect();
        format!(
            "Property(name={:?}, value={:?}, params=[{}], group={:?})",
            self.name,
            self.value,
            params.join(", "),
            self.group
        )
    }

    fn __eq__(&self, other: &Property, py: Python<'_>) -> bool {
        self.group == other.group
            && self.name == other.name
            && self.value == other.value
            && self.params.len() == other.params.len()
            && self
                .params
                .iter()
                .zip(&other.params)
                .all(|(a, b)| a.borrow(py).__eq__(&b.borrow(py)))
    }
}

/// A component: a name plus an ordered list of children, each of which is
/// either a Property or a Component.
#[pyclass(module = "vobject._core")]
struct Component {
    #[pyo3(get, set)]
    name: String,
    #[pyo3(get, set)]
    children: Vec<Py<PyAny>>,
}

#[pymethods]
impl Component {
    #[new]
    #[pyo3(signature = (name, children = Vec::new()))]
    fn new(name: String, children: Vec<Py<PyAny>>) -> Component {
        Component { name, children }
    }

    fn __repr__(&self) -> String {
        format!(
            "Component(name={:?}, <{} children>)",
            self.name,
            self.children.len()
        )
    }

    /// All direct child properties, in order.
    fn properties(&self, py: Python<'_>) -> Vec<Py<Property>> {
        self.children
            .iter()
            .filter_map(|c| c.cast_bound::<Property>(py).ok())
            .map(|p| p.clone().unbind())
            .collect()
    }

    /// All direct child components, in order.
    fn components(&self, py: Python<'_>) -> Vec<Py<Component>> {
        self.children
            .iter()
            .filter_map(|c| c.cast_bound::<Component>(py).ok())
            .map(|c| c.clone().unbind())
            .collect()
    }

    /// Direct child properties with the given name (case-insensitive).
    fn props(&self, py: Python<'_>, name: &str) -> Vec<Py<Property>> {
        self.properties(py)
            .into_iter()
            .filter(|p| p.borrow(py).name.eq_ignore_ascii_case(name))
            .collect()
    }

    /// First direct child property with the given name, or None.
    fn prop(&self, py: Python<'_>, name: &str) -> Option<Py<Property>> {
        self.props(py, name).into_iter().next()
    }

    /// Direct child components with the given name (case-insensitive).
    fn comps(&self, py: Python<'_>, name: &str) -> Vec<Py<Component>> {
        self.components(py)
            .into_iter()
            .filter(|c| c.borrow(py).name.eq_ignore_ascii_case(name))
            .collect()
    }

    /// First direct child component with the given name, or None.
    fn comp(&self, py: Python<'_>, name: &str) -> Option<Py<Component>> {
        self.comps(py, name).into_iter().next()
    }

    fn __eq__(&self, other: &Component, py: Python<'_>) -> PyResult<bool> {
        if self.name != other.name || self.children.len() != other.children.len() {
            return Ok(false);
        }
        for (a, b) in self.children.iter().zip(&other.children) {
            if let (Ok(pa), Ok(pb)) = (
                a.cast_bound::<Property>(py),
                b.cast_bound::<Property>(py),
            ) {
                if !pa.borrow().__eq__(&pb.borrow(), py) {
                    return Ok(false);
                }
            } else if let (Ok(ca), Ok(cb)) = (
                a.cast_bound::<Component>(py),
                b.cast_bound::<Component>(py),
            ) {
                if !ca.borrow().__eq__(&cb.borrow(), py)? {
                    return Ok(false);
                }
            } else {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// One recovery made by the lenient parser.
#[pyclass(module = "vobject._core", frozen)]
struct Repair {
    #[pyo3(get)]
    line: usize,
    #[pyo3(get)]
    message: String,
}

#[pymethods]
impl Repair {
    fn __repr__(&self) -> String {
        format!("Repair(line={}, message={:?})", self.line, self.message)
    }
}

// ---------------------------------------------------------------------------
// Conversions between the core model and the Python classes.

fn param_to_py(py: Python<'_>, p: &core::Param) -> PyResult<Py<Param>> {
    Py::new(
        py,
        Param {
            name: p.name.clone(),
            values: p.values.clone(),
        },
    )
}

fn property_to_py(py: Python<'_>, p: &core::Property) -> PyResult<Py<Property>> {
    Ok(Py::new(
        py,
        Property {
            group: p.group.clone(),
            name: p.name.clone(),
            params: p
                .params
                .iter()
                .map(|param| param_to_py(py, param))
                .collect::<PyResult<Vec<_>>>()?,
            value: p.value.clone(),
        },
    )?)
}

fn component_to_py(py: Python<'_>, c: &core::Component) -> PyResult<Py<Component>> {
    let mut children: Vec<Py<PyAny>> = Vec::with_capacity(c.children.len());
    for child in &c.children {
        match child {
            core::Child::Property(p) => children.push(property_to_py(py, p)?.into_any()),
            core::Child::Component(k) => children.push(component_to_py(py, k)?.into_any()),
        }
    }
    Py::new(
        py,
        Component {
            name: c.name.clone(),
            children,
        },
    )
}

fn py_to_property(py: Python<'_>, p: &Property) -> core::Property {
    core::Property {
        group: p.group.clone(),
        name: p.name.clone(),
        params: p
            .params
            .iter()
            .map(|param| {
                let param = param.borrow(py);
                core::Param {
                    name: param.name.clone(),
                    values: param.values.clone(),
                }
            })
            .collect(),
        value: p.value.clone(),
    }
}

fn py_to_component(py: Python<'_>, c: &Component) -> PyResult<core::Component> {
    let mut out = core::Component::new(c.name.clone());
    for child in &c.children {
        if let Ok(p) = child.cast_bound::<Property>(py) {
            out.push_property(py_to_property(py, &p.borrow()));
        } else if let Ok(k) = child.cast_bound::<Component>(py) {
            out.push_component(py_to_component(py, &k.borrow())?);
        } else {
            return Err(PyValueError::new_err(format!(
                "component children must be Property or Component, not {}",
                child.bind(py).get_type().name()?
            )));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Module functions

/// Parse a document. Returns (components, repairs).
#[pyfunction]
#[pyo3(signature = (text, strict = false, max_depth = 512))]
fn parse(
    py: Python<'_>,
    text: &str,
    strict: bool,
    max_depth: usize,
) -> PyResult<(Vec<Py<Component>>, Vec<Py<Repair>>)> {
    let options = core::ParseOptions {
        strictness: if strict {
            core::Strictness::Strict
        } else {
            core::Strictness::Lenient
        },
        max_depth,
    };
    let parsed = core::parse(text, &options).map_err(|e| {
        let err = ParseError::new_err(e.to_string());
        // Attach the line number for programmatic access.
        Python::attach(|py| {
            let _ = err.value(py).setattr("line", e.location.line);
        });
        err
    })?;

    let components = parsed
        .components
        .iter()
        .map(|c| component_to_py(py, c))
        .collect::<PyResult<Vec<_>>>()?;
    let repairs = parsed
        .repairs
        .iter()
        .map(|r| {
            Py::new(
                py,
                Repair {
                    line: r.location.line,
                    message: r.kind.to_string(),
                },
            )
        })
        .collect::<PyResult<Vec<_>>>()?;
    Ok((components, repairs))
}

/// Serialize components to wire format.
#[pyfunction]
#[pyo3(signature = (components, line_ending = "\r\n", fold_width = Some(75)))]
fn serialize(
    py: Python<'_>,
    components: Vec<Py<Component>>,
    line_ending: &str,
    fold_width: Option<usize>,
) -> PyResult<String> {
    let core_components: Vec<core::Component> = components
        .iter()
        .map(|c| py_to_component(py, &c.borrow(py)))
        .collect::<PyResult<Vec<_>>>()?;
    let options = core::WriteOptions {
        line_ending: line_ending.to_string(),
        fold_width,
    };
    Ok(core::write_document(&core_components, &options))
}

/// Unescape a TEXT value (leniently: invalid escapes are kept verbatim).
#[pyfunction]
fn unescape_text(text: &str) -> String {
    let mut repairs = Vec::new();
    core::escape::unescape_text(text, Some(&mut repairs), 1)
        .expect("lenient unescape is total")
}

/// Escape a string as a TEXT value.
#[pyfunction]
fn escape_text(text: &str) -> String {
    core::escape::escape_text(text)
}

/// Fold one logical line to physical lines (75-octet default), returning
/// the folded text including the trailing line ending.
#[pyfunction]
#[pyo3(signature = (line, width = 75, line_ending = "\r\n"))]
fn fold_line(line: &str, width: usize, line_ending: &str) -> String {
    let mut out = String::new();
    vobject_core::fold::fold_into(&mut out, line, width, line_ending);
    out
}

/// Split a raw value on an unescaped separator (e.g. ',' or ';').
#[pyfunction]
fn split_unescaped(text: &str, separator: char) -> Vec<String> {
    core::escape::split_unescaped(text, separator)
        .into_iter()
        .map(|s| s.to_string())
        .collect()
}

/// Parse a property's typed value, returning a (kind, payload) pair that
/// the Python layer converts to rich native objects:
///
/// - ("text", [str]) / ("structured", [[str]])
/// - ("date", [(y, m, d)])
/// - ("datetime", [(y, m, d, h, mi, s, utc) or (y, m, d)]) — mixed shapes
/// - ("time", [(h, mi, s, utc)])
/// - ("duration", [seconds])
/// - ("period", [(start_tuple, end_kind, end_payload)])
/// - ("recur", {parts}) / ("integer", [int]) / ("float", [float])
/// - ("boolean", bool) / ("binary", bytes) / ("uri"|"cal-address", str)
/// - ("utc-offset", seconds) / ("unknown", str)
#[pyfunction]
#[pyo3(signature = (prop, dialect = "icalendar"))]
fn typed_value(py: Python<'_>, prop: &Property, dialect: &str) -> PyResult<(String, Py<PyAny>)> {
    use pyo3::conversion::IntoPyObjectExt;
    use vobject_core::value as v;

    let dialect = parse_dialect(dialect)?;
    let core_prop = py_to_property(py, prop);
    let value = core_prop
        .typed_value(dialect)
        .map_err(|e| ParseError::new_err(e.to_string()))?;

    fn date_tuple(d: v::Date) -> (i32, u8, u8) {
        (d.year, d.month, d.day)
    }
    fn dt_tuple(dt: v::DateTime) -> (i32, u8, u8, u8, u8, u8, bool) {
        (
            dt.date.year,
            dt.date.month,
            dt.date.day,
            dt.time.hour,
            dt.time.minute,
            dt.time.second,
            dt.time.utc,
        )
    }

    let text_kind = match core_prop.type_info(dialect).multiplicity {
        v::Multiplicity::CommaList => "text-list",
        _ => "text",
    };
    let (kind, payload): (&str, Py<PyAny>) = match value {
        v::Value::Text(items) => (text_kind, items.into_py_any(py)?),
        v::Value::Structured(c) => ("structured", c.into_py_any(py)?),
        v::Value::Date(items) => (
            "date",
            items
                .into_iter()
                .map(date_tuple)
                .collect::<Vec<_>>()
                .into_py_any(py)?,
        ),
        v::Value::DateTime(items) => {
            let mixed: Vec<Py<PyAny>> = items
                .into_iter()
                .map(|d| match d {
                    v::DateOrDateTime::Date(d) => date_tuple(d).into_py_any(py),
                    v::DateOrDateTime::DateTime(dt) => dt_tuple(dt).into_py_any(py),
                })
                .collect::<PyResult<_>>()?;
            ("datetime", mixed.into_py_any(py)?)
        }
        v::Value::Time(items) => (
            "time",
            items
                .into_iter()
                .map(|t| (t.hour, t.minute, t.second, t.utc))
                .collect::<Vec<_>>()
                .into_py_any(py)?,
        ),
        v::Value::Duration(items) => (
            "duration",
            items
                .into_iter()
                .map(|d| d.total_seconds())
                .collect::<Vec<_>>()
                .into_py_any(py)?,
        ),
        v::Value::Period(items) => {
            let out: Vec<Py<PyAny>> = items
                .into_iter()
                .map(|p| match p.end {
                    v::PeriodEnd::End(e) => {
                        (dt_tuple(p.start), "end", dt_tuple(e).into_py_any(py)?)
                            .into_py_any(py)
                    }
                    v::PeriodEnd::Duration(d) => {
                        (dt_tuple(p.start), "duration", d.total_seconds())
                            .into_py_any(py)
                    }
                })
                .collect::<PyResult<_>>()?;
            ("period", out.into_py_any(py)?)
        }
        v::Value::Recur(r) => ("recur", r.to_string().into_py_any(py)?),
        v::Value::Integer(items) => ("integer", items.into_py_any(py)?),
        v::Value::Float(items) => ("float", items.into_py_any(py)?),
        v::Value::Boolean(b) => ("boolean", b.into_py_any(py)?),
        v::Value::Binary(data) => ("binary", data.into_py_any(py)?),
        v::Value::Uri(s) => ("uri", s.into_py_any(py)?),
        v::Value::CalAddress(s) => ("cal-address", s.into_py_any(py)?),
        v::Value::UtcOffset(o) => ("utc-offset", o.seconds.into_py_any(py)?),
        v::Value::Unknown(s) => ("unknown", s.into_py_any(py)?),
    };
    Ok((kind.to_string(), payload))
}

fn parse_dialect(dialect: &str) -> PyResult<vobject_core::value::Dialect> {
    match dialect {
        "icalendar" => Ok(vobject_core::value::Dialect::ICalendar),
        "vcard4" => Ok(vobject_core::value::Dialect::VCard4),
        "vcard3" => Ok(vobject_core::value::Dialect::VCard3),
        other => Err(PyValueError::new_err(format!("unknown dialect {other:?}"))),
    }
}

/// Expand an RRULE. Returns up to `limit` instances as tuples:
/// (y, m, d) for date starts, (y, m, d, h, mi, s, utc) otherwise.
#[pyfunction]
#[pyo3(signature = (rule, dtstart, limit = 1000))]
fn expand_rrule(py: Python<'_>, rule: &str, dtstart: &str, limit: usize) -> PyResult<Py<PyAny>> {
    use pyo3::conversion::IntoPyObjectExt;
    use vobject_core::rrule::{expand, ExpandLimits};
    use vobject_core::value::{DateOrDateTime, Recur};

    let recur = Recur::parse(rule).map_err(|e| ParseError::new_err(e.to_string()))?;
    let start =
        DateOrDateTime::parse(dtstart).map_err(|e| ParseError::new_err(e.to_string()))?;
    let iter =
        expand(&recur, start, ExpandLimits::default()).map_err(|e| ParseError::new_err(e.to_string()))?;
    let out: Vec<Py<PyAny>> = iter
        .take(limit)
        .map(|d| match d {
            DateOrDateTime::Date(d) => (d.year, d.month, d.day).into_py_any(py),
            DateOrDateTime::DateTime(dt) => (
                dt.date.year,
                dt.date.month,
                dt.date.day,
                dt.time.hour,
                dt.time.minute,
                dt.time.second,
                dt.time.utc,
            )
                .into_py_any(py),
        })
        .collect::<PyResult<_>>()?;
    out.into_py_any(py)
}

/// Convert a component to jCal/jCard, returned as a JSON string.
#[pyfunction]
#[pyo3(signature = (component, dialect = None))]
fn to_jcal_json(py: Python<'_>, component: &Component, dialect: Option<&str>) -> PyResult<String> {
    let core = py_to_component(py, component)?;
    let value = match dialect {
        None => vobject_core::jcal::to_jcal(&core),
        Some(d) => vobject_core::jcal::component_to_jcal(&core, parse_dialect(d)?),
    };
    Ok(value.to_string())
}

/// Serialize components to an xCal/xCard XML document.
#[pyfunction]
fn to_xcal_xml(py: Python<'_>, components: Vec<Py<Component>>) -> PyResult<String> {
    let core: Vec<vobject_core::Component> = components
        .iter()
        .map(|c| py_to_component(py, &c.borrow(py)))
        .collect::<PyResult<Vec<_>>>()?;
    vobject_core::xcal::to_xml(&core).map_err(|e| ParseError::new_err(e.to_string()))
}

/// Parse an xCal/xCard XML document into components.
#[pyfunction]
fn from_xcal_xml(py: Python<'_>, xml: &str) -> PyResult<Vec<Py<Component>>> {
    let comps =
        vobject_core::xcal::from_xml(xml).map_err(|e| ParseError::new_err(e.to_string()))?;
    comps.iter().map(|c| component_to_py(py, c)).collect()
}

#[pymodule]
fn _core(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Component>()?;
    m.add_class::<Property>()?;
    m.add_class::<Param>()?;
    m.add_class::<Repair>()?;
    m.add_function(wrap_pyfunction!(parse, m)?)?;
    m.add_function(wrap_pyfunction!(serialize, m)?)?;
    m.add_function(wrap_pyfunction!(escape_text, m)?)?;
    m.add_function(wrap_pyfunction!(unescape_text, m)?)?;
    m.add_function(wrap_pyfunction!(split_unescaped, m)?)?;
    m.add_function(wrap_pyfunction!(fold_line, m)?)?;
    m.add_function(wrap_pyfunction!(typed_value, m)?)?;
    m.add_function(wrap_pyfunction!(expand_rrule, m)?)?;
    m.add_function(wrap_pyfunction!(to_jcal_json, m)?)?;
    m.add_function(wrap_pyfunction!(to_xcal_xml, m)?)?;
    m.add_function(wrap_pyfunction!(from_xcal_xml, m)?)?;
    m.add("ParseError", py.get_type::<ParseError>())?;
    Ok(())
}
