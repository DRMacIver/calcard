//! The lossless document model.
//!
//! This layer represents a vobject document exactly as written: property and
//! subcomponent order is preserved (including interleaving), unknown
//! properties and parameters are kept, and property values are stored in
//! their raw (escaped, unfolded) textual form. Typed interpretation of
//! values lives in [`crate::value`].
//!
//! Names (component, property, parameter) are matched case-insensitively
//! everywhere, per the RFCs, but stored as parsed.

use std::fmt;

/// Case-insensitive ASCII comparison used for all vobject names.
pub(crate) fn name_eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// A parameter on a property, e.g. `TZID=Europe/London` or vCard 2.1's bare
/// `HOME`.
///
/// Values are stored in decoded form: surrounding quotes removed and
/// RFC 6868 caret escapes (`^^`, `^n`, `^'`) decoded. The serializer
/// re-quotes and re-encodes as required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    /// Decoded values. Empty for a bare vCard 2.1 parameter like `HOME` in
    /// `TEL;HOME:...`.
    pub values: Vec<String>,
}

impl Param {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Param {
        Param {
            name: name.into(),
            values: vec![value.into()],
        }
    }

    pub fn bare(name: impl Into<String>) -> Param {
        Param {
            name: name.into(),
            values: Vec::new(),
        }
    }

    /// The single value of this parameter, if it has exactly one.
    pub fn value(&self) -> Option<&str> {
        match self.values.as_slice() {
            [v] => Some(v),
            _ => None,
        }
    }
}

/// A single property (content line), e.g.
/// `item1.EMAIL;TYPE=work:alice@example.com`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    /// vCard property group (`item1` in `item1.EMAIL:...`), if any.
    pub group: Option<String>,
    pub name: String,
    pub params: Vec<Param>,
    /// The raw value: unfolded, but still in its escaped on-the-wire form.
    /// Use [`crate::value`] or the escape helpers to interpret it.
    pub value: String,
}

impl Property {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Property {
        Property {
            group: None,
            name: name.into(),
            params: Vec::new(),
            value: value.into(),
        }
    }

    /// All parameters with the given name (case-insensitive).
    pub fn params_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Param> {
        self.params.iter().filter(move |p| name_eq(&p.name, name))
    }

    /// The first parameter with the given name (case-insensitive).
    pub fn param(&self, name: &str) -> Option<&Param> {
        self.params.iter().find(|p| name_eq(&p.name, name))
    }

    /// The first value of the first parameter with the given name.
    pub fn param_value(&self, name: &str) -> Option<&str> {
        self.param(name)
            .and_then(|p| p.values.first().map(|s| s.as_str()))
    }

    /// All values of all parameters with the given name, flattened.
    /// `TYPE=a,b` and `TYPE=a;TYPE=b` both yield `["a", "b"]`.
    pub fn param_values<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> {
        self.params_named(name)
            .flat_map(|p| p.values.iter().map(|s| s.as_str()))
    }
}

/// An ordered child of a component: either a property or a nested component.
///
/// Keeping properties and subcomponents in one ordered list preserves
/// interleaving exactly as it appeared in the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Child {
    Property(Property),
    Component(Component),
}

/// A component: `BEGIN:NAME` ... `END:NAME` with its ordered children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    pub children: Vec<Child>,
}

impl Component {
    pub fn new(name: impl Into<String>) -> Component {
        Component {
            name: name.into(),
            children: Vec::new(),
        }
    }

    pub fn push_property(&mut self, p: Property) {
        self.children.push(Child::Property(p));
    }

    pub fn push_component(&mut self, c: Component) {
        self.children.push(Child::Component(c));
    }

    /// All direct child properties, in order.
    pub fn properties(&self) -> impl Iterator<Item = &Property> {
        self.children.iter().filter_map(|c| match c {
            Child::Property(p) => Some(p),
            Child::Component(_) => None,
        })
    }

    pub fn properties_mut(&mut self) -> impl Iterator<Item = &mut Property> {
        self.children.iter_mut().filter_map(|c| match c {
            Child::Property(p) => Some(p),
            Child::Component(_) => None,
        })
    }

    /// All direct child components, in order.
    pub fn components(&self) -> impl Iterator<Item = &Component> {
        self.children.iter().filter_map(|c| match c {
            Child::Component(k) => Some(k),
            Child::Property(_) => None,
        })
    }

    pub fn components_mut(&mut self) -> impl Iterator<Item = &mut Component> {
        self.children.iter_mut().filter_map(|c| match c {
            Child::Component(k) => Some(k),
            Child::Property(_) => None,
        })
    }

    /// Direct child properties with the given name (case-insensitive).
    pub fn props<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Property> {
        self.properties().filter(move |p| name_eq(&p.name, name))
    }

    /// First direct child property with the given name.
    pub fn prop(&self, name: &str) -> Option<&Property> {
        self.properties().find(|p| name_eq(&p.name, name))
    }

    /// Direct child components with the given name (case-insensitive).
    pub fn comps<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Component> {
        self.components().filter(move |c| name_eq(&c.name, name))
    }

    /// First direct child component with the given name.
    pub fn comp(&self, name: &str) -> Option<&Component> {
        self.components().find(|c| name_eq(&c.name, name))
    }

    /// Is this component named `name` (case-insensitive)?
    pub fn is(&self, name: &str) -> bool {
        name_eq(&self.name, name)
    }
}

impl fmt::Display for Component {
    /// Serializes with default options (CRLF, folded at 75 octets).
    /// A model that cannot be written (see [`crate::write::WriteError`])
    /// surfaces as `fmt::Error`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let out = crate::write::write_component(self, &crate::write::WriteOptions::default())
            .map_err(|_| fmt::Error)?;
        f.write_str(&out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_lookup_is_case_insensitive() {
        let mut p = Property::new("DTSTART", "20260722T120000");
        p.params.push(Param::new("TZID", "Europe/London"));
        assert_eq!(p.param_value("tzid"), Some("Europe/London"));
        assert_eq!(p.param_value("TZID"), Some("Europe/London"));
        assert_eq!(p.param_value("VALUE"), None);
    }

    #[test]
    fn param_values_flattens_repeats_and_lists() {
        let mut p = Property::new("TEL", "+441234567890");
        p.params.push(Param {
            name: "TYPE".into(),
            values: vec!["home".into(), "voice".into()],
        });
        p.params.push(Param::new("type", "cell"));
        let vals: Vec<&str> = p.param_values("TYPE").collect();
        assert_eq!(vals, vec!["home", "voice", "cell"]);
    }

    #[test]
    fn component_accessors() {
        let mut cal = Component::new("VCALENDAR");
        cal.push_property(Property::new("VERSION", "2.0"));
        let mut ev = Component::new("VEVENT");
        ev.push_property(Property::new("SUMMARY", "Test"));
        cal.push_component(ev);
        cal.push_property(Property::new("PRODID", "-//x//y//EN"));

        assert_eq!(cal.prop("version").unwrap().value, "2.0");
        assert!(cal.comp("vevent").is_some());
        assert_eq!(cal.properties().count(), 2);
        assert_eq!(cal.components().count(), 1);
        // Interleaved order is preserved.
        assert!(matches!(cal.children[1], Child::Component(_)));
    }
}
