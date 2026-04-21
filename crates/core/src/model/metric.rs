//! Metric data models.
//!
//! Represents metric query results normalized from any backend provider.
//! Labels are always sorted alphabetically (`BTreeMap`). Points use the
//! compact `[[timestamp, value], ...]` format for ~75% token savings.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A single time series with labels and data points.
///
/// Represents a metric series as returned by any backend provider,
/// normalized to the obz unified format. Labels are always sorted
/// alphabetically by key (`BTreeMap`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricSeries {
    /// Metric name (e.g., `cpu_usage`, `http_requests_total`).
    pub name: String,

    /// Label key-value pairs, sorted alphabetically by key.
    pub labels: BTreeMap<String, String>,

    /// Time series data points as `[[timestamp, value], ...]`.
    ///
    /// Timestamps are Unix seconds (i64). Values are f64.
    /// NaN is serialized as `null` in Agent View, `"NaN"` in Full View.
    /// +Inf/-Inf are serialized as `"+Inf"` / `"-Inf"` strings.
    pub points: Vec<DataPoint>,

    /// Pre-computed statistics for this series.
    ///
    /// Present for matrix/vector results. Allows AI Agents to analyze
    /// series without iterating over all points.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<SeriesStats>,

    /// Provider-specific metadata (Full View only).
    ///
    /// Keys use provider prefix (e.g., `"<provider>.step"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// A single data point as `[timestamp, value]`.
///
/// Uses a two-element array format for ~75% token savings compared to
/// `{"timestamp": ..., "value": ...}` object format.
#[derive(Debug, Clone, PartialEq)]
pub struct DataPoint {
    /// Unix timestamp in seconds.
    pub timestamp: i64,

    /// Metric value. May be NaN, +Inf, or -Inf.
    pub value: f64,
}

// NOTE: Current serialization always uses Agent View semantics (NaN → null).
// Full View requires NaN → "NaN" (string), which will be handled by the
// view projection layer in the output module (Step 2-3).

impl Serialize for DataPoint {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeTuple;
        let mut tup = serializer.serialize_tuple(2)?;
        tup.serialize_element(&self.timestamp)?;
        // NaN → null, +/-Inf → string representation
        if self.value.is_nan() {
            tup.serialize_element(&None::<f64>)?;
        } else if self.value.is_infinite() {
            if self.value.is_sign_positive() {
                tup.serialize_element(&"+Inf")?;
            } else {
                tup.serialize_element(&"-Inf")?;
            }
        } else {
            tup.serialize_element(&self.value)?;
        }
        tup.end()
    }
}

impl<'de> Deserialize<'de> for DataPoint {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let arr: (i64, serde_json::Value) = Deserialize::deserialize(deserializer)?;
        let value = match &arr.1 {
            serde_json::Value::Null => f64::NAN,
            serde_json::Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
            serde_json::Value::String(s) => match s.as_str() {
                "+Inf" => f64::INFINITY,
                "-Inf" => f64::NEG_INFINITY,
                "NaN" => f64::NAN,
                other => other.parse::<f64>().unwrap_or(f64::NAN),
            },
            _ => f64::NAN,
        };
        Ok(DataPoint {
            timestamp: arr.0,
            value,
        })
    }
}

/// Pre-computed statistics for a metric series.
///
/// Allows AI Agents to analyze series without iterating over all points.
/// All fields exclude only NaN values from computation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SeriesStats {
    /// Minimum value (excluding NaN; may be `-Inf`). `None` if all points are NaN.
    pub min: Option<f64>,

    /// Maximum value (excluding NaN; may be `+Inf`). `None` if all points are NaN.
    pub max: Option<f64>,

    /// Arithmetic mean of non-NaN points. `None` if all points are NaN.
    /// May return `Some(NaN)` when the series contains both `+Inf` and `-Inf`,
    /// or `Some(Inf)` when it contains `Inf` values in one direction only.
    pub avg: Option<f64>,

    /// Number of valid (non-NaN) data points.
    pub count: usize,
}

impl SeriesStats {
    /// Compute statistics from a slice of data points.
    ///
    /// NaN values are excluded from all computations. Infinity values
    /// participate normally: `min` may be `-Inf`, `max` may be `+Inf`,
    /// and `avg` may be `Inf` or `NaN` (when both `+Inf` and `-Inf` are present).
    ///
    /// # Examples
    ///
    /// ```
    /// use obz_core::model::metric::{DataPoint, SeriesStats};
    ///
    /// let points = vec![
    ///     DataPoint { timestamp: 1, value: 10.0 },
    ///     DataPoint { timestamp: 2, value: 20.0 },
    ///     DataPoint { timestamp: 3, value: f64::NAN },
    /// ];
    /// let stats = SeriesStats::from_points(&points);
    /// assert_eq!(stats.count, 2);
    /// assert_eq!(stats.min, Some(10.0));
    /// assert_eq!(stats.avg, Some(15.0));
    /// ```
    pub fn from_points(points: &[DataPoint]) -> Self {
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        let mut sum = 0.0_f64;
        let mut count = 0_usize;

        for p in points {
            if !p.value.is_nan() {
                min = min.min(p.value);
                max = max.max(p.value);
                sum += p.value;
                count += 1;
            }
        }

        if count == 0 {
            Self {
                min: None,
                max: None,
                avg: None,
                count: 0,
            }
        } else {
            Self {
                min: Some(min),
                max: Some(max),
                avg: Some(sum / count as f64),
                count,
            }
        }
    }
}

/// Metric type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricType {
    /// Instantaneous measurement that can go up or down.
    Gauge,
    /// Monotonically increasing counter.
    Counter,
    /// Distribution of values (percentiles).
    Histogram,
    /// Type could not be determined.
    Unknown,
}

/// Metric metadata returned by `metric info`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricInfoDetail {
    /// Metric name.
    pub name: String,

    /// Metric type.
    #[serde(rename = "type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metric_type: Option<MetricType>,

    /// Human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Unit of measurement (e.g., "seconds", "bytes", "percent").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_point_serialization_normal() {
        let point = DataPoint {
            timestamp: 1711234800,
            value: 75.3,
        };
        let json = serde_json::to_string(&point).unwrap();
        assert_eq!(json, "[1711234800,75.3]");
    }

    #[test]
    fn test_data_point_serialization_nan() {
        let point = DataPoint {
            timestamp: 1711234800,
            value: f64::NAN,
        };
        let json = serde_json::to_string(&point).unwrap();
        assert_eq!(json, "[1711234800,null]");
    }

    #[test]
    fn test_data_point_serialization_infinity() {
        let point = DataPoint {
            timestamp: 1711234800,
            value: f64::INFINITY,
        };
        let json = serde_json::to_string(&point).unwrap();
        assert_eq!(json, r#"[1711234800,"+Inf"]"#);

        let neg = DataPoint {
            timestamp: 1711234800,
            value: f64::NEG_INFINITY,
        };
        let json = serde_json::to_string(&neg).unwrap();
        assert_eq!(json, r#"[1711234800,"-Inf"]"#);
    }

    #[test]
    fn test_data_point_deserialization_normal() {
        let point: DataPoint = serde_json::from_str("[1711234800,75.3]").unwrap();
        assert_eq!(point.timestamp, 1711234800);
        assert!((point.value - 75.3).abs() < f64::EPSILON);
    }

    #[test]
    fn test_data_point_deserialization_null_is_nan() {
        let point: DataPoint = serde_json::from_str("[1711234800,null]").unwrap();
        assert!(point.value.is_nan());
    }

    #[test]
    fn test_data_point_deserialization_inf_strings() {
        let pos: DataPoint = serde_json::from_str(r#"[1711234800,"+Inf"]"#).unwrap();
        assert!(pos.value.is_infinite() && pos.value.is_sign_positive());

        let neg: DataPoint = serde_json::from_str(r#"[1711234800,"-Inf"]"#).unwrap();
        assert!(neg.value.is_infinite() && neg.value.is_sign_negative());
    }

    #[test]
    fn test_series_stats_from_points() {
        let points = vec![
            DataPoint {
                timestamp: 1,
                value: 10.0,
            },
            DataPoint {
                timestamp: 2,
                value: 20.0,
            },
            DataPoint {
                timestamp: 3,
                value: 30.0,
            },
        ];
        let stats = SeriesStats::from_points(&points);
        assert_eq!(stats.min, Some(10.0));
        assert_eq!(stats.max, Some(30.0));
        assert_eq!(stats.avg, Some(20.0));
        assert_eq!(stats.count, 3);
    }

    #[test]
    fn test_series_stats_all_nan() {
        let points = vec![
            DataPoint {
                timestamp: 1,
                value: f64::NAN,
            },
            DataPoint {
                timestamp: 2,
                value: f64::NAN,
            },
        ];
        let stats = SeriesStats::from_points(&points);
        assert_eq!(stats.min, None);
        assert_eq!(stats.max, None);
        assert_eq!(stats.avg, None);
        assert_eq!(stats.count, 0);
    }

    #[test]
    fn test_series_stats_mixed_nan() {
        let points = vec![
            DataPoint {
                timestamp: 1,
                value: f64::NAN,
            },
            DataPoint {
                timestamp: 2,
                value: 50.0,
            },
            DataPoint {
                timestamp: 3,
                value: 100.0,
            },
        ];
        let stats = SeriesStats::from_points(&points);
        assert_eq!(stats.min, Some(50.0));
        assert_eq!(stats.max, Some(100.0));
        assert_eq!(stats.avg, Some(75.0));
        assert_eq!(stats.count, 2);
    }

    #[test]
    fn test_metric_type_serialization() {
        assert_eq!(
            serde_json::to_string(&MetricType::Gauge).unwrap(),
            r#""gauge""#
        );
        assert_eq!(
            serde_json::to_string(&MetricType::Counter).unwrap(),
            r#""counter""#
        );
        assert_eq!(
            serde_json::to_string(&MetricType::Histogram).unwrap(),
            r#""histogram""#
        );
        assert_eq!(
            serde_json::to_string(&MetricType::Unknown).unwrap(),
            r#""unknown""#
        );
    }

    #[test]
    fn test_metric_series_json_roundtrip() {
        let series = MetricSeries {
            name: "cpu_usage".to_string(),
            labels: BTreeMap::from([
                ("env".to_string(), "prod".to_string()),
                ("host".to_string(), "web01".to_string()),
            ]),
            points: vec![
                DataPoint {
                    timestamp: 1711234800,
                    value: 75.3,
                },
                DataPoint {
                    timestamp: 1711234860,
                    value: 82.1,
                },
            ],
            stats: Some(SeriesStats {
                min: Some(75.3),
                max: Some(82.1),
                avg: Some(78.7),
                count: 2,
            }),
            extensions: None,
        };

        let json = serde_json::to_string_pretty(&series).unwrap();
        let deserialized: MetricSeries = serde_json::from_str(&json).unwrap();

        assert_eq!(series.name, deserialized.name);
        assert_eq!(series.labels, deserialized.labels);
        assert_eq!(series.points.len(), deserialized.points.len());
        assert_eq!(series.stats, deserialized.stats);
    }

    #[test]
    fn test_series_stats_empty_points() {
        let stats = SeriesStats::from_points(&[]);
        assert_eq!(stats.min, None);
        assert_eq!(stats.max, None);
        assert_eq!(stats.avg, None);
        assert_eq!(stats.count, 0);
    }

    #[test]
    fn test_series_stats_includes_infinity() {
        let points = vec![
            DataPoint {
                timestamp: 1,
                value: 10.0,
            },
            DataPoint {
                timestamp: 2,
                value: f64::INFINITY,
            },
            DataPoint {
                timestamp: 3,
                value: 20.0,
            },
            DataPoint {
                timestamp: 4,
                value: f64::NEG_INFINITY,
            },
        ];
        let stats = SeriesStats::from_points(&points);
        assert_eq!(stats.min, Some(f64::NEG_INFINITY));
        assert_eq!(stats.max, Some(f64::INFINITY));
        assert!(stats.avg.unwrap().is_nan());
        assert_eq!(stats.count, 4);
    }

    #[test]
    fn test_series_stats_positive_infinity_only() {
        let points = vec![
            DataPoint {
                timestamp: 1,
                value: 10.0,
            },
            DataPoint {
                timestamp: 2,
                value: f64::INFINITY,
            },
            DataPoint {
                timestamp: 3,
                value: 20.0,
            },
        ];
        let stats = SeriesStats::from_points(&points);
        assert_eq!(stats.min, Some(10.0));
        assert!(stats.max.unwrap().is_infinite() && stats.max.unwrap().is_sign_positive());
        assert!(stats.avg.unwrap().is_infinite() && stats.avg.unwrap().is_sign_positive());
        assert_eq!(stats.count, 3);
    }
}
