//! PyO3 bindings for vobject-core.
//!
//! This module (`calcard._core`) exposes the lossless document model and the
//! strict/lenient parser and serializer. The user-facing Python API lives in
//! the pure-Python `calcard` package on top of these primitives.
//!
//! The model is mutable in place: a child object obtained from the tree is
//! the tree's child, not a copy, and the list-valued attributes
//! (`children`, `params`, `values`) are live Python lists — the getter
//! returns the list itself, so `comp.children.append(x)` is respected.
//! Assigning to such an attribute copies the assigned sequence into a
//! fresh list owned by the object (so later mutation of the source
//! sequence does not leak in).

use pyo3::create_exception;
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyList, PyString};
use vobject_core as core;

/// What `parse`/`parse_bytes` hand back to Python: (components, repairs).
type ParsedDocument = (Vec<Py<Component>>, Vec<Py<Repair>>);

create_exception!(
    calcard._core,
    ParseError,
    PyValueError,
    "The input could not be parsed as a vobject document."
);

/// Copy a sequence of strings into a fresh Python list, type-checking
/// each element.
fn str_list(py: Python<'_>, values: Option<Bound<'_, PyAny>>) -> PyResult<Py<PyList>> {
    let list = PyList::empty(py);
    if let Some(values) = values {
        for item in values.try_iter()? {
            let item = item?;
            if !item.is_instance_of::<PyString>() {
                return Err(PyTypeError::new_err(format!(
                    "parameter values must be str, not {}",
                    item.get_type().name()?
                )));
            }
            list.append(item)?;
        }
    }
    Ok(list.unbind())
}

/// Copy a sequence of Params into a fresh Python list, type-checking
/// each element.
fn param_list(py: Python<'_>, params: Option<Bound<'_, PyAny>>) -> PyResult<Py<PyList>> {
    let list = PyList::empty(py);
    if let Some(params) = params {
        for item in params.try_iter()? {
            let item = item?;
            if !item.is_instance_of::<Param>() {
                return Err(PyTypeError::new_err(format!(
                    "property params must be Param, not {}",
                    item.get_type().name()?
                )));
            }
            list.append(item)?;
        }
    }
    Ok(list.unbind())
}

/// Copy an arbitrary sequence into a fresh Python list. Child types are
/// checked at use (serialization/conversion), not on insertion.
fn any_list(py: Python<'_>, items: Option<Bound<'_, PyAny>>) -> PyResult<Py<PyList>> {
    let list = PyList::empty(py);
    if let Some(items) = items {
        for item in items.try_iter()? {
            list.append(item?)?;
        }
    }
    Ok(list.unbind())
}

/// A property parameter: a name and zero or more decoded values.
#[pyclass(module = "calcard._core")]
struct Param {
    #[pyo3(get, set)]
    name: String,
    values: Py<PyList>,
}

#[pymethods]
impl Param {
    #[new]
    #[pyo3(signature = (name, values = None))]
    fn new(py: Python<'_>, name: String, values: Option<Bound<'_, PyAny>>) -> PyResult<Param> {
        Ok(Param {
            name,
            values: str_list(py, values)?,
        })
    }

    /// The live list of decoded values: mutating it mutates the parameter.
    #[getter]
    fn values(&self, py: Python<'_>) -> Py<PyList> {
        self.values.clone_ref(py)
    }

    #[setter]
    fn set_values(&mut self, py: Python<'_>, values: Bound<'_, PyAny>) -> PyResult<()> {
        self.values = str_list(py, Some(values))?;
        Ok(())
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!(
            "Param(name={:?}, values={})",
            self.name,
            self.values.bind(py).repr()?
        ))
    }

    fn __eq__(&self, other: &Param, py: Python<'_>) -> PyResult<bool> {
        Ok(self.name == other.name && self.values.bind(py).eq(other.values.bind(py))?)
    }
}

/// A single content line: group, name, parameters, and the raw value.
#[pyclass(module = "calcard._core")]
struct Property {
    #[pyo3(get, set)]
    group: Option<String>,
    #[pyo3(get, set)]
    name: String,
    params: Py<PyList>,
    /// The raw (escaped, unfolded) value text.
    #[pyo3(get, set)]
    value: String,
}

#[pymethods]
impl Property {
    #[new]
    #[pyo3(signature = (name, value, params = None, group = None))]
    fn new(
        py: Python<'_>,
        name: String,
        value: String,
        params: Option<Bound<'_, PyAny>>,
        group: Option<String>,
    ) -> PyResult<Property> {
        Ok(Property {
            group,
            name,
            params: param_list(py, params)?,
            value,
        })
    }

    /// The live parameter list: mutating it mutates the property.
    #[getter]
    fn params(&self, py: Python<'_>) -> Py<PyList> {
        self.params.clone_ref(py)
    }

    #[setter]
    fn set_params(&mut self, py: Python<'_>, params: Bound<'_, PyAny>) -> PyResult<()> {
        self.params = param_list(py, Some(params))?;
        Ok(())
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!(
            "Property(name={:?}, value={:?}, params={}, group={:?})",
            self.name,
            self.value,
            self.params.bind(py).repr()?,
            self.group
        ))
    }

    fn __eq__(&self, other: &Property, py: Python<'_>) -> PyResult<bool> {
        Ok(self.group == other.group
            && self.name == other.name
            && self.value == other.value
            && self.params.bind(py).eq(other.params.bind(py))?)
    }
}

/// A component: a name plus an ordered list of children, each of which is
/// either a Property or a Component.
#[pyclass(module = "calcard._core")]
struct Component {
    #[pyo3(get, set)]
    name: String,
    children: Py<PyList>,
}

impl Drop for Component {
    /// Deallocating a deeply nested tree through the default recursive
    /// reference-count cascade overflows the C stack (a segfault, not an
    /// exception): CPython's trashcan bounds chains of *list* deallocs,
    /// but the interleaved pyo3 Component deallocs are not trashcan-aware.
    /// Flatten it: while this component holds the only reference to its
    /// child list, drain the list, and steal the child list of any
    /// component the drained list held the only reference to. Shared
    /// lists/children (refcount > 1) are left untouched — someone else
    /// still observes them.
    fn drop(&mut self) {
        Python::attach(|py| {
            // Deallocation can run while an exception is being propagated
            // (temporaries dropped during unwinding); the Python calls
            // below must not observe or clobber it.
            let pending = PyErr::take(py);
            // SAFETY (both Py_REFCNT calls): the pointers are valid, owned
            // object pointers for the duration of the call (Py_REFCNT only
            // reads the refcount field). This is the replacement pyo3
            // recommends for its deprecated safe get_refcnt wrappers.
            let mut stack: Vec<Py<PyAny>> = Vec::new();
            fn steal(list: &Bound<'_, PyList>, stack: &mut Vec<Py<PyAny>>) {
                if unsafe { pyo3::ffi::Py_REFCNT(list.as_ptr()) } == 1 {
                    for child in list.iter() {
                        stack.push(child.unbind());
                    }
                    let _ = list.call_method0("clear");
                }
            }
            steal(self.children.bind(py), &mut stack);
            while let Some(obj) = stack.pop() {
                if unsafe { pyo3::ffi::Py_REFCNT(obj.as_ptr()) } == 1 {
                    if let Ok(comp) = obj.cast_bound::<Component>(py) {
                        if let Ok(inner) = comp.try_borrow_mut() {
                            steal(inner.children.bind(py), &mut stack);
                        }
                    }
                }
                drop(obj);
            }
            if let Some(err) = pending {
                err.restore(py);
            }
        });
    }
}

#[pymethods]
impl Component {
    #[new]
    #[pyo3(signature = (name, children = None))]
    fn new(
        py: Python<'_>,
        name: String,
        children: Option<Bound<'_, PyAny>>,
    ) -> PyResult<Component> {
        Ok(Component {
            name,
            children: any_list(py, children)?,
        })
    }

    /// The live child list: mutating it mutates the component.
    #[getter]
    fn children(&self, py: Python<'_>) -> Py<PyList> {
        self.children.clone_ref(py)
    }

    #[setter]
    fn set_children(&mut self, py: Python<'_>, children: Bound<'_, PyAny>) -> PyResult<()> {
        self.children = any_list(py, Some(children))?;
        Ok(())
    }

    fn __repr__(&self, py: Python<'_>) -> String {
        format!(
            "Component(name={:?}, <{} children>)",
            self.name,
            self.children.bind(py).len()
        )
    }

    /// All direct child properties, in order.
    fn properties(&self, py: Python<'_>) -> Vec<Py<Property>> {
        self.children
            .bind(py)
            .iter()
            .filter_map(|c| c.cast_into::<Property>().ok())
            .map(|p| p.unbind())
            .collect()
    }

    /// All direct child components, in order.
    fn components(&self, py: Python<'_>) -> Vec<Py<Component>> {
        self.children
            .bind(py)
            .iter()
            .filter_map(|c| c.cast_into::<Component>().ok())
            .map(|c| c.unbind())
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
        self.eq_at(other, py, 0)
    }
}

impl Component {
    /// Recursive equality with the same depth cap as tree conversion, so
    /// pathologically deep (or cyclic) trees raise instead of overflowing
    /// the C stack.
    fn eq_at(&self, other: &Component, py: Python<'_>, depth: usize) -> PyResult<bool> {
        if depth >= MAX_TREE_DEPTH {
            return Err(PyValueError::new_err(
                "component nesting exceeds the supported depth limit (is the tree cyclic?)",
            ));
        }
        let ours = self.children.bind(py);
        let theirs = other.children.bind(py);
        if self.name != other.name || ours.len() != theirs.len() {
            return Ok(false);
        }
        for (a, b) in ours.iter().zip(theirs.iter()) {
            if let (Ok(pa), Ok(pb)) = (a.cast::<Property>(), b.cast::<Property>()) {
                if !pa.borrow().__eq__(&pb.borrow(), py)? {
                    return Ok(false);
                }
            } else if let (Ok(ca), Ok(cb)) = (a.cast::<Component>(), b.cast::<Component>()) {
                if !ca.borrow().eq_at(&cb.borrow(), py, depth + 1)? {
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
#[pyclass(module = "calcard._core", frozen)]
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
            values: PyList::new(py, &p.values)?.unbind(),
        },
    )
}

fn property_to_py(py: Python<'_>, p: &core::Property) -> PyResult<Py<Property>> {
    let params = PyList::empty(py);
    for param in &p.params {
        params.append(param_to_py(py, param)?)?;
    }
    Py::new(
        py,
        Property {
            group: p.group.clone(),
            name: p.name.clone(),
            params: params.unbind(),
            value: p.value.clone(),
        },
    )
}

fn component_to_py(py: Python<'_>, c: &core::Component) -> PyResult<Py<Component>> {
    let children = PyList::empty(py);
    for child in &c.children {
        match child {
            core::Child::Property(p) => children.append(property_to_py(py, p)?)?,
            core::Child::Component(k) => children.append(component_to_py(py, k)?)?,
        }
    }
    Py::new(
        py,
        Component {
            name: c.name.clone(),
            children: children.unbind(),
        },
    )
}

fn py_to_property(py: Python<'_>, p: &Property) -> PyResult<core::Property> {
    let mut params = Vec::new();
    for item in p.params.bind(py).iter() {
        let param = item.cast::<Param>().map_err(|_| {
            PyTypeError::new_err(format!(
                "property params must be Param, not {}",
                item.get_type()
                    .name()
                    .map(|n| n.to_string())
                    .unwrap_or_default()
            ))
        })?;
        let param = param.borrow();
        let mut values = Vec::new();
        for v in param.values.bind(py).iter() {
            values.push(v.extract::<String>().map_err(|_| {
                PyTypeError::new_err(format!(
                    "parameter values must be str, not {}",
                    v.get_type()
                        .name()
                        .map(|n| n.to_string())
                        .unwrap_or_default()
                ))
            })?);
        }
        params.push(core::Param {
            name: param.name.clone(),
            values,
        });
    }
    Ok(core::Property {
        group: p.group.clone(),
        name: p.name.clone(),
        params,
        value: p.value.clone(),
    })
}

/// Depth cap matching the parser's default `max_depth`. Python code can
/// build arbitrarily deep (or, via shared child lists, cyclic) trees; an
/// uncapped recursive conversion would abort the process with a stack
/// overflow instead of raising.
const MAX_TREE_DEPTH: usize = 512;

fn py_to_component(py: Python<'_>, c: &Component) -> PyResult<core::Component> {
    py_to_component_at(py, c, 0)
}

fn py_to_component_at(py: Python<'_>, c: &Component, depth: usize) -> PyResult<core::Component> {
    if depth >= MAX_TREE_DEPTH {
        return Err(PyValueError::new_err(
            "component nesting exceeds the supported depth limit (is the tree cyclic?)",
        ));
    }
    let mut out = core::Component::new(c.name.clone());
    for child in c.children.bind(py).iter() {
        if let Ok(p) = child.cast::<Property>() {
            out.push_property(py_to_property(py, &p.borrow())?);
        } else if let Ok(k) = child.cast::<Component>() {
            out.push_component(py_to_component_at(py, &k.borrow(), depth + 1)?);
        } else {
            return Err(PyValueError::new_err(format!(
                "component children must be Property or Component, not {}",
                child.get_type().name()?
            )));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Module functions

/// Validate parse options. `max_depth` may be lowered but never raised
/// above `MAX_TREE_DEPTH`: deeper trees would parse and then crash the
/// recursive conversion, comparison, and serialization paths.
fn parse_options(strict: bool, max_depth: usize) -> PyResult<core::ParseOptions> {
    if max_depth > MAX_TREE_DEPTH {
        return Err(PyValueError::new_err(format!(
            "max_depth may be lowered but not raised above {MAX_TREE_DEPTH}: \
             deeper trees cannot be safely processed"
        )));
    }
    Ok(core::ParseOptions {
        strictness: if strict {
            core::Strictness::Strict
        } else {
            core::Strictness::Lenient
        },
        max_depth,
    })
}

/// Parse a document. Returns (components, repairs).
#[pyfunction]
#[pyo3(signature = (text, strict = false, max_depth = 512))]
fn parse(py: Python<'_>, text: &str, strict: bool, max_depth: usize) -> PyResult<ParsedDocument> {
    let options = parse_options(strict, max_depth)?;
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

/// Parse a document from raw bytes (strict: UTF-8 only; lenient: Latin-1
/// fallback recorded as a repair). Returns (components, repairs).
#[pyfunction]
#[pyo3(signature = (data, strict = false, max_depth = 512))]
fn parse_bytes(
    py: Python<'_>,
    data: &[u8],
    strict: bool,
    max_depth: usize,
) -> PyResult<ParsedDocument> {
    let options = parse_options(strict, max_depth)?;
    let parsed = core::parse_bytes(data, &options).map_err(|e| {
        let err = ParseError::new_err(e.to_string());
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
    core::write_document(&core_components, &options)
        .map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Unescape a TEXT value (leniently: invalid escapes are kept verbatim).
#[pyfunction]
fn unescape_text(text: &str) -> String {
    let mut repairs = Vec::new();
    core::escape::unescape_text(text, Some(&mut repairs), 1).expect("lenient unescape is total")
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
/// - ("recur", str) — the canonicalized rule text / ("integer", [int]) / ("float", [float])
/// - ("boolean", bool) / ("binary", bytes) / ("uri"|"cal-address", str)
/// - ("utc-offset", seconds) / ("unknown", str)
#[pyfunction]
#[pyo3(signature = (prop, dialect = "icalendar"))]
fn typed_value(py: Python<'_>, prop: &Property, dialect: &str) -> PyResult<(String, Py<PyAny>)> {
    use pyo3::conversion::IntoPyObjectExt;
    use vobject_core::value as v;

    let dialect = parse_dialect(dialect)?;
    let core_prop = py_to_property(py, prop)?;
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
                        (dt_tuple(p.start), "end", dt_tuple(e).into_py_any(py)?).into_py_any(py)
                    }
                    v::PeriodEnd::Duration(d) => {
                        (dt_tuple(p.start), "duration", d.total_seconds()).into_py_any(py)
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
/// `max_years` caps how many years past DTSTART are scanned;
/// `max_empty_periods` caps consecutive periods yielding no instance
/// (defaults are the core's ExpandLimits defaults).
#[pyfunction]
#[pyo3(signature = (rule, dtstart, limit = 1000, max_years = None, max_empty_periods = None))]
fn expand_rrule(
    py: Python<'_>,
    rule: &str,
    dtstart: &str,
    limit: usize,
    max_years: Option<i32>,
    max_empty_periods: Option<usize>,
) -> PyResult<Py<PyAny>> {
    use pyo3::conversion::IntoPyObjectExt;
    use vobject_core::rrule::{expand, ExpandLimits};
    use vobject_core::value::{DateOrDateTime, Recur};

    let recur = Recur::parse(rule).map_err(|e| ParseError::new_err(e.to_string()))?;
    let start = DateOrDateTime::parse(dtstart).map_err(|e| ParseError::new_err(e.to_string()))?;
    let mut limits = ExpandLimits::default();
    if let Some(y) = max_years {
        limits.max_years = y;
    }
    if let Some(n) = max_empty_periods {
        limits.max_empty_periods = n;
    }
    let iter = expand(&recur, start, limits).map_err(|e| ParseError::new_err(e.to_string()))?;
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

/// Parse a jCal/jCard JSON document into components.
#[pyfunction]
fn from_jcal_json(py: Python<'_>, json: &str) -> PyResult<Vec<Py<Component>>> {
    let comps =
        vobject_core::jcal::from_jcal(json).map_err(|e| ParseError::new_err(e.to_string()))?;
    comps.iter().map(|c| component_to_py(py, c)).collect()
}

#[pymodule]
fn _core(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Component>()?;
    m.add_class::<Property>()?;
    m.add_class::<Param>()?;
    m.add_class::<Repair>()?;
    m.add_function(wrap_pyfunction!(parse, m)?)?;
    m.add_function(wrap_pyfunction!(parse_bytes, m)?)?;
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
    m.add_function(wrap_pyfunction!(from_jcal_json, m)?)?;
    m.add("ParseError", py.get_type::<ParseError>())?;
    Ok(())
}
