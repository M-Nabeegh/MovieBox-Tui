use serde::{Deserialize, Serialize};

/// Transfer protocol selected by a catalog provider.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum SourceTransport {
    #[default]
    HttpFile,
    Dash {
        maximum_height: u16,
        expected_duration_seconds: Option<f64>,
    },
}
