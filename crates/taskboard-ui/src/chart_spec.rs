//! The chart spec: the small, strict subset of Vega-Lite that ` ```chart `
//! blocks (and dashboard files) are written in. See `docs/charts.md`.
//!
//! A spec is YAML, parsed with `serde_norway`; Vega-Lite's own JSON parses
//! too, since JSON is YAML. Anything outside the subset is an error rather
//! than silently dropped: unknown keys, marks and field types come back with
//! the line they're on, and a spec that parses but can't be drawn (a field
//! no row has, a word where a number belongs) names the key at fault.

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime};
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};

/// A parsed, checked chart.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// Accepted so Vega-Lite pasted whole still parses; not used.
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// Plot size in logical pixels; the widget picks one when unset.
    #[serde(default)]
    pub width: Option<f32>,
    #[serde(default)]
    pub height: Option<f32>,
    pub mark: Mark,
    pub encoding: Encoding,
    pub data: Data,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Bar,
    Line,
    Area,
    Point,
}

const MARKS: &[&str] = &["bar", "line", "area", "point"];

impl Mark {
    fn from_name<E: de::Error>(name: &str) -> Result<Self, E> {
        match name {
            "bar" => Ok(Mark::Bar),
            "line" => Ok(Mark::Line),
            "area" => Ok(Mark::Area),
            "point" => Ok(Mark::Point),
            other => Err(E::unknown_variant(other, MARKS)),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mark::Bar => "bar",
            Mark::Line => "line",
            Mark::Area => "area",
            Mark::Point => "point",
        }
    }
}

/// `mark: bar`, or Vega-Lite's object form with only a type: `mark: {type: bar}`.
impl<'de> Deserialize<'de> for Mark {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct MarkVisitor;
        impl<'de> Visitor<'de> for MarkVisitor {
            type Value = Mark;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a mark name (bar, line, area, point) or {type: <name>}")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Mark, E> {
                Mark::from_name(v)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Mark, A::Error> {
                let mut mark = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key != "type" {
                        return Err(de::Error::unknown_field(&key, &["type"]));
                    }
                    if mark.is_some() {
                        return Err(de::Error::duplicate_field("type"));
                    }
                    mark = Some(Mark::from_name(&map.next_value::<String>()?)?);
                }
                mark.ok_or_else(|| de::Error::missing_field("type"))
            }
        }
        d.deserialize_any(MarkVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Encoding {
    pub x: Channel,
    pub y: Channel,
    /// Splits the data into series, each drawn in its own colour.
    #[serde(default)]
    pub color: Option<Channel>,
}

/// One encoding channel: which field of each row it shows, and how.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub field: String,
    #[serde(rename = "type")]
    pub kind: FieldType,
    /// Axis or legend title; the field name when unset.
    #[serde(default)]
    pub title: Option<String>,
}

impl Channel {
    pub fn title(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.field)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Quantitative,
    Nominal,
    Ordinal,
    Temporal,
}

impl FieldType {
    fn name(self) -> &'static str {
        match self {
            FieldType::Quantitative => "quantitative",
            FieldType::Nominal => "nominal",
            FieldType::Ordinal => "ordinal",
            FieldType::Temporal => "temporal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    pub values: Vec<Row>,
}

/// One data row: field name to value.
pub type Row = BTreeMap<String, Datum>;

/// A value in a data row. Rows are flat: lists and maps are rejected.
#[derive(Debug, Clone, PartialEq)]
pub enum Datum {
    Number(f64),
    Text(String),
    Bool(bool),
    Null,
}

impl<'de> Deserialize<'de> for Datum {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct DatumVisitor;
        impl<'de> Visitor<'de> for DatumVisitor {
            type Value = Datum;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a number, string, boolean or null")
            }
            fn visit_bool<E>(self, v: bool) -> Result<Datum, E> {
                Ok(Datum::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Datum, E> {
                Ok(Datum::Number(v as f64))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Datum, E> {
                Ok(Datum::Number(v as f64))
            }
            fn visit_f64<E>(self, v: f64) -> Result<Datum, E> {
                Ok(Datum::Number(v))
            }
            fn visit_str<E>(self, v: &str) -> Result<Datum, E> {
                Ok(Datum::Text(v.to_owned()))
            }
            fn visit_unit<E>(self) -> Result<Datum, E> {
                Ok(Datum::Null)
            }
            fn visit_none<E>(self) -> Result<Datum, E> {
                Ok(Datum::Null)
            }
        }
        d.deserialize_any(DatumVisitor)
    }
}

impl Datum {
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Datum::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// The point in time a temporal value names: `2024`, `2024-03`,
    /// `2024-03-15`, `2024-03-15T09:30[:00]` (or with a space for the `T`),
    /// or RFC 3339 with an offset, taken as UTC. Numbers aren't times: write
    /// a bare year as `"2024"`.
    pub fn as_time(&self) -> Option<NaiveDateTime> {
        let Datum::Text(s) = self else { return None };
        let s = s.trim();
        if let Ok(t) = DateTime::parse_from_rfc3339(s) {
            return Some(t.naive_utc());
        }
        for fmt in [
            "%Y-%m-%dT%H:%M:%S",
            "%Y-%m-%dT%H:%M",
            "%Y-%m-%d %H:%M:%S",
            "%Y-%m-%d %H:%M",
        ] {
            if let Ok(t) = NaiveDateTime::parse_from_str(s, fmt) {
                return Some(t);
            }
        }
        let date = match s.len() {
            4 => NaiveDate::parse_from_str(&format!("{s}-01-01"), "%Y-%m-%d"),
            7 => NaiveDate::parse_from_str(&format!("{s}-01"), "%Y-%m-%d"),
            _ => NaiveDate::parse_from_str(s, "%Y-%m-%d"),
        };
        date.ok().map(|d| d.and_time(NaiveTime::MIN))
    }

    /// The value as a category label, for nominal and ordinal fields.
    pub fn label(&self) -> String {
        match self {
            Datum::Number(n) => n.to_string(),
            Datum::Text(s) => s.clone(),
            Datum::Bool(b) => b.to_string(),
            Datum::Null => String::new(),
        }
    }
}

/// Why a spec was rejected. `line` is 1-based, when the problem has one.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartError {
    pub message: String,
    pub line: Option<usize>,
}

impl fmt::Display for ChartError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ChartError {}

impl ChartError {
    fn new(message: impl Into<String>) -> Self {
        ChartError {
            message: message.into(),
            line: None,
        }
    }
}

/// Parses and checks a chart spec.
pub fn parse(src: &str) -> Result<Spec, ChartError> {
    let spec: Spec = serde_norway::from_str(src).map_err(|e| ChartError {
        // serde_norway's message names the key path and ends with
        // "at line L column C".
        message: e.to_string(),
        line: e.location().map(|l| l.line()),
    })?;
    spec.check()?;
    Ok(spec)
}

impl Spec {
    /// The channels in use, with the key each is written under.
    pub fn channels(&self) -> impl Iterator<Item = (&'static str, &Channel)> {
        [
            ("x", Some(&self.encoding.x)),
            ("y", Some(&self.encoding.y)),
            ("color", self.encoding.color.as_ref()),
        ]
        .into_iter()
        .filter_map(|(name, c)| c.map(|c| (name, c)))
    }

    /// What serde can't: sizes, which channels fit which marks, and whether
    /// the data has the fields the encoding names, with values of their type.
    fn check(&self) -> Result<(), ChartError> {
        for (key, size) in [("width", self.width), ("height", self.height)] {
            if let Some(size) = size
                && !(size.is_finite() && size > 0.0)
            {
                return Err(ChartError::new(format!(
                    "{key} must be a positive number, not {size}"
                )));
            }
        }

        if let Some(color) = &self.encoding.color
            && !matches!(color.kind, FieldType::Nominal | FieldType::Ordinal)
        {
            return Err(ChartError::new(format!(
                "encoding.color: type must be nominal or ordinal, not {}",
                color.kind.name()
            )));
        }

        if self.mark != Mark::Point
            && self.encoding.x.kind != FieldType::Quantitative
            && self.encoding.y.kind != FieldType::Quantitative
        {
            return Err(ChartError::new(format!(
                "a {} chart needs encoding.x or encoding.y to be quantitative",
                self.mark.name()
            )));
        }

        if self.data.values.is_empty() {
            return Err(ChartError::new(
                "data.values is empty: a chart needs at least one row",
            ));
        }

        for (name, channel) in self.channels() {
            if !self
                .data
                .values
                .iter()
                .any(|row| row.contains_key(&channel.field))
            {
                return Err(ChartError::new(format!(
                    "encoding.{name}.field: no row in data.values has a `{}` field",
                    channel.field
                )));
            }
            for (i, row) in self.data.values.iter().enumerate() {
                let Some(value) = row.get(&channel.field) else {
                    continue;
                };
                let fits = match (channel.kind, value) {
                    (_, Datum::Null) => true,
                    (FieldType::Quantitative, v) => v.as_number().is_some_and(f64::is_finite),
                    (FieldType::Temporal, v) => v.as_time().is_some(),
                    (FieldType::Nominal | FieldType::Ordinal, _) => true,
                };
                if !fits {
                    let wanted = match channel.kind {
                        FieldType::Temporal => "a date or time like \"2024-03-15\"",
                        _ => "a number",
                    };
                    return Err(ChartError::new(format!(
                        "data.values[{i}].{}: {} isn't {wanted}, but encoding.{name} is {}",
                        channel.field,
                        show(value),
                        channel.kind.name()
                    )));
                }
            }
        }
        Ok(())
    }
}

fn show(value: &Datum) -> String {
    match value {
        Datum::Text(s) => format!("{s:?}"),
        other => other.label(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SALES: &str = "\
title: Sales by quarter
mark: bar
encoding:
  x: {field: quarter, type: ordinal}
  y: {field: sales, type: quantitative, title: Sales ($k)}
  color: {field: region, type: nominal}
data:
  values:
    - {quarter: Q1, region: north, sales: 12}
    - {quarter: Q1, region: south, sales: 9.5}
    - {quarter: Q2, region: north, sales: 15}
";

    fn err(src: &str) -> ChartError {
        parse(src).expect_err("spec should be rejected")
    }

    #[test]
    fn parses_yaml() {
        let spec = parse(SALES).unwrap();
        assert_eq!(spec.title.as_deref(), Some("Sales by quarter"));
        assert_eq!(spec.mark, Mark::Bar);
        assert_eq!(spec.encoding.x.field, "quarter");
        assert_eq!(spec.encoding.x.kind, FieldType::Ordinal);
        assert_eq!(spec.encoding.x.title(), "quarter");
        assert_eq!(spec.encoding.y.title(), "Sales ($k)");
        assert_eq!(spec.encoding.color.as_ref().unwrap().field, "region");
        assert_eq!(spec.data.values.len(), 3);
        assert_eq!(spec.data.values[1]["sales"], Datum::Number(9.5));
        assert_eq!(spec.data.values[1]["region"], Datum::Text("south".into()));
        assert_eq!(spec.width, None);
    }

    #[test]
    fn parses_vega_lite_json() {
        let spec = parse(
            r#"{
              "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
              "description": "Daily visits",
              "width": 400, "height": 200,
              "mark": {"type": "line"},
              "encoding": {
                "x": {"field": "day", "type": "temporal"},
                "y": {"field": "visits", "type": "quantitative"}
              },
              "data": {"values": [
                {"day": "2024-03-01", "visits": 120},
                {"day": "2024-03-02", "visits": null}
              ]}
            }"#,
        )
        .unwrap();
        assert_eq!(spec.mark, Mark::Line);
        assert_eq!((spec.width, spec.height), (Some(400.0), Some(200.0)));
        assert_eq!(spec.data.values[1]["visits"], Datum::Null);
        assert_eq!(
            spec.data.values[0]["day"].as_time(),
            NaiveDate::from_ymd_opt(2024, 3, 1).map(|d| d.and_time(NaiveTime::MIN))
        );
    }

    #[test]
    fn every_mark_parses() {
        for mark in MARKS {
            let spec = parse(&SALES.replace("mark: bar", &format!("mark: {mark}"))).unwrap();
            assert_eq!(spec.mark.name(), *mark);
        }
    }

    #[test]
    fn unknown_top_level_key_names_its_line() {
        let e = err(&SALES.replace("mark: bar", "mark: bar\ntransform: []"));
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("unknown field `transform`"), "{e}");
        assert!(e.message.contains("line 3"), "{e}");
    }

    #[test]
    fn unknown_channel_key_names_its_line() {
        let e = err(&SALES.replace("type: ordinal}", "type: ordinal, sort: descending}"));
        assert_eq!(e.line, Some(4));
        assert!(e.message.contains("unknown field `sort`"), "{e}");
    }

    #[test]
    fn unsupported_channel_is_rejected() {
        let e = err(&SALES.replace(
            "  color:",
            "  size: {field: sales, type: quantitative}\n  color:",
        ));
        assert_eq!(e.line, Some(6));
        assert!(e.message.contains("unknown field `size`"), "{e}");
    }

    #[test]
    fn unsupported_mark_is_rejected() {
        let e = err(&SALES.replace("mark: bar", "mark: arc"));
        assert_eq!(e.line, Some(2));
        assert!(e.message.contains("unknown variant `arc`"), "{e}");

        let e = err(&SALES.replace("mark: bar", "mark: {type: bar, tooltip: true}"));
        assert!(e.message.contains("unknown field `tooltip`"), "{e}");
    }

    #[test]
    fn unsupported_field_type_is_rejected() {
        let e = err(&SALES.replace("type: ordinal", "type: geojson"));
        assert_eq!(e.line, Some(4));
        assert!(e.message.contains("unknown variant `geojson`"), "{e}");
    }

    #[test]
    fn data_url_is_rejected() {
        let e = err(&SALES.replace("  values:", "  url: data.csv\n  values:"));
        assert!(e.message.contains("unknown field `url`"), "{e}");
    }

    #[test]
    fn missing_keys_are_reported() {
        let e = err(&SALES.replace("mark: bar\n", ""));
        assert!(e.message.contains("missing field `mark`"), "{e}");
        let e = err(&SALES.replace("  x: {field: quarter, type: ordinal}\n", ""));
        assert!(e.message.contains("missing field `x`"), "{e}");
        let e = err(&SALES.replace("{field: quarter, type: ordinal}", "{field: quarter}"));
        assert!(e.message.contains("missing field `type`"), "{e}");
    }

    #[test]
    fn nested_row_values_are_rejected() {
        let e = err(&SALES.replace("sales: 15}", "sales: [1, 2]}"));
        assert_eq!(e.line, Some(11));
        assert!(
            e.message.contains("a number, string, boolean or null"),
            "{e}"
        );
    }

    #[test]
    fn invalid_yaml_is_reported() {
        let e = err("mark: bar\nencoding: {x: [\n");
        assert!(e.line.is_some(), "{e}");
    }

    #[test]
    fn field_missing_from_data() {
        let e = err(&SALES.replace("field: sales", "field: revenue"));
        assert_eq!(e.line, None);
        assert_eq!(
            e.message,
            "encoding.y.field: no row in data.values has a `revenue` field"
        );
    }

    #[test]
    fn quantitative_field_must_hold_numbers() {
        let e = err(&SALES.replace("sales: 15}", "sales: lots}"));
        assert_eq!(
            e.message,
            "data.values[2].sales: \"lots\" isn't a number, but encoding.y is quantitative"
        );
    }

    #[test]
    fn temporal_field_must_hold_dates() {
        let src = "\
mark: line
encoding:
  x: {field: day, type: temporal}
  y: {field: n, type: quantitative}
data:
  values:
    - {day: 2024-03-01, n: 1}
    - {day: 2024, n: 2}
";
        let e = err(src);
        assert_eq!(
            e.message,
            "data.values[1].day: 2024 isn't a date or time like \"2024-03-15\", but encoding.x is temporal"
        );
        assert!(parse(&src.replace("day: 2024,", "day: \"2024\",")).is_ok());
    }

    #[test]
    fn color_must_be_categorical() {
        let e = err(&SALES.replace(
            "{field: region, type: nominal}",
            "{field: sales, type: quantitative}",
        ));
        assert_eq!(
            e.message,
            "encoding.color: type must be nominal or ordinal, not quantitative"
        );
    }

    #[test]
    fn bar_needs_a_quantitative_axis() {
        let e = err(&SALES.replace("type: quantitative, title", "type: nominal, title"));
        assert_eq!(
            e.message,
            "a bar chart needs encoding.x or encoding.y to be quantitative"
        );
        assert!(
            parse(
                &SALES
                    .replace("mark: bar", "mark: point")
                    .replace("type: quantitative, title", "type: nominal, title")
            )
            .is_ok()
        );
    }

    #[test]
    fn empty_data_and_bad_sizes() {
        let empty = "mark: point\nencoding: {x: {field: a, type: nominal}, y: {field: b, type: nominal}}\ndata: {values: []}\n";
        assert_eq!(
            err(empty).message,
            "data.values is empty: a chart needs at least one row"
        );
        assert_eq!(
            err(&format!("width: 0\n{SALES}")).message,
            "width must be a positive number, not 0"
        );
        assert_eq!(
            err(&format!("height: -5\n{SALES}")).message,
            "height must be a positive number, not -5"
        );
    }

    #[test]
    fn temporal_formats() {
        let t = |s: &str| Datum::Text(s.into()).as_time().map(|t| t.to_string());
        assert_eq!(t("2024").as_deref(), Some("2024-01-01 00:00:00"));
        assert_eq!(t("2024-03").as_deref(), Some("2024-03-01 00:00:00"));
        assert_eq!(t("2024-03-15").as_deref(), Some("2024-03-15 00:00:00"));
        assert_eq!(
            t("2024-03-15T09:30").as_deref(),
            Some("2024-03-15 09:30:00")
        );
        assert_eq!(
            t("2024-03-15 09:30:05").as_deref(),
            Some("2024-03-15 09:30:05")
        );
        assert_eq!(
            t("2024-03-15T09:30:00+02:00").as_deref(),
            Some("2024-03-15 07:30:00")
        );
        assert_eq!(t("March 15"), None);
        assert_eq!(Datum::Number(2024.0).as_time(), None);
    }

    #[test]
    fn doc_examples_parse() {
        let doc = include_str!("../../../docs/charts.md");
        let mut blocks = 0;
        let mut lines = doc.lines();
        while let Some(line) = lines.next() {
            if line == "```chart" {
                let src: Vec<&str> = lines.by_ref().take_while(|l| *l != "```").collect();
                if let Err(e) = parse(&src.join("\n")) {
                    panic!("docs/charts.md example {} doesn't parse: {e}", blocks + 1);
                }
                blocks += 1;
            }
        }
        assert_eq!(blocks, 4);
    }
}
