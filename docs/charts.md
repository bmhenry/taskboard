# Charts

A fenced code block with the language `chart` holds a chart. The chart is written in a
small, strict subset of [Vega-Lite](https://vega.github.io/vega-lite/), in YAML.
Vega-Lite's JSON also works, because JSON is valid YAML. Other Markdown viewers, such as
GitHub, show the block as code, so the file stays readable everywhere.

````markdown
```chart
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
    - {quarter: Q2, region: south, sales: 11}
```
````

Dashboard files use the same spec for their charts.

## Keys

| Key           | Required | Value |
|---------------|----------|-------|
| `mark`        | yes      | `bar`, `line`, `area` or `point`. Vega-Lite's `{type: bar}` form works too, but only with `type` in it. |
| `encoding`    | yes      | Which field goes on which channel. Takes `x` and `y` (both required) and an optional `color`. |
| `data`        | yes      | `values`: a list of rows. Each row is a flat map of field name to number, string, boolean or null. |
| `title`       | no       | Text shown above the chart. |
| `width`, `height` | no   | Plot size in pixels, as a positive number. The viewer picks a size when these are left out. |
| `description` | no       | A note for readers of the source. It isn't drawn. |
| `$schema`     | no       | Ignored, so a pasted Vega-Lite spec still parses. |

Each channel (`x`, `y`, `color`) takes these keys:

| Key     | Required | Value |
|---------|----------|-------|
| `field` | yes      | The name of a field in the data rows. At least one row must have it. |
| `type`  | yes      | `quantitative` (numbers), `nominal` (unordered categories), `ordinal` (ordered categories) or `temporal` (dates and times). |
| `title` | no       | The axis or legend title. Defaults to the field name. |

## Rules

- **Unknown keys are errors.** A key that isn't listed above is rejected, and the error
  names its line. It isn't silently dropped. This applies to keys such as `transform`,
  `data.url`, `size`, `sort`, `aggregate` and `layer`.
- `bar`, `line` and `area` charts need `x` or `y` to be `quantitative`. A `point` chart can
  plot any two types against each other.
- `color` splits the rows into series, one colour per value. Its type must be `nominal` or
  `ordinal`.
- Every value of a `quantitative` field must be a number.
- Every value of a `temporal` field must be a string in one of these forms: `"2024"`,
  `"2024-03"`, `2024-03-15`, `2024-03-15T09:30`, `2024-03-15 09:30:00`, or RFC 3339 with
  an offset (`2024-03-15T09:30:00+02:00`), which is converted to UTC. Quote a bare year:
  YAML reads `2024` as a number, and a number is not a date.
- A `null` value, or a field left out of a row, means that row has no value for the field.
- Data is given inline. There is no aggregation, so give each bar or point its final value.

## Errors

When a spec is invalid, the viewer shows the error and the block's source instead of a
chart. Some examples:

```text
mark: unknown variant `arc`, expected one of `bar`, `line`, `area`, `point` at line 1 column 7
encoding.x: unknown field `sort`, expected one of `field`, `type`, `title` at line 3 column 32
encoding.y.field: no row in data.values has a `revenue` field
data.values[2].sales: "lots" isn't a number, but encoding.y is quantitative
```

## More examples

A line chart over time, with two series:

```chart
mark: line
encoding:
  x: {field: day, type: temporal}
  y: {field: visits, type: quantitative}
  color: {field: site, type: nominal}
data:
  values:
    - {day: 2024-03-01, site: docs, visits: 120}
    - {day: 2024-03-02, site: docs, visits: 135}
    - {day: 2024-03-03, site: docs, visits: 98}
    - {day: 2024-03-01, site: app, visits: 80}
    - {day: 2024-03-02, site: app, visits: 91}
    - {day: 2024-03-03, site: app, visits: 102}
```

A horizontal bar chart. Put the quantitative field on `x`:

```chart
mark: bar
encoding:
  x: {field: open, type: quantitative, title: Open tasks}
  y: {field: status, type: ordinal}
data:
  values:
    - {status: Backlog, open: 4}
    - {status: In progress, open: 2}
    - {status: Needs review, open: 1}
```

The same kind of spec written as Vega-Lite JSON:

```chart
{
  "mark": "point",
  "encoding": {
    "x": {"field": "estimate", "type": "quantitative"},
    "y": {"field": "actual", "type": "quantitative"}
  },
  "data": {"values": [
    {"estimate": 2, "actual": 3},
    {"estimate": 5, "actual": 4},
    {"estimate": 8, "actual": 13}
  ]}
}
```
