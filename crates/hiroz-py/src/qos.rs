use hiroz::qos::{
    QosDurability, QosDuration as Duration, QosHistory, QosLiveliness, QosProfile, QosReliability,
};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::num::NonZeroUsize;

// ROS 2 QoS presets
pub const QOS_DEFAULT: QosProfile = QosProfile {
    reliability: QosReliability::Reliable,
    durability: QosDurability::Volatile,
    history: QosHistory::KeepLast(NonZeroUsize::new(10).unwrap()),
    deadline: Duration::INFINITE,
    lifespan: Duration::INFINITE,
    liveliness: QosLiveliness::Automatic,
    liveliness_lease_duration: Duration::INFINITE,
};

pub const QOS_SENSOR_DATA: QosProfile = QosProfile {
    reliability: QosReliability::BestEffort,
    durability: QosDurability::Volatile,
    history: QosHistory::KeepLast(NonZeroUsize::new(5).unwrap()),
    deadline: Duration::INFINITE,
    lifespan: Duration::INFINITE,
    liveliness: QosLiveliness::Automatic,
    liveliness_lease_duration: Duration::INFINITE,
};

pub const QOS_PARAMETERS: QosProfile = QosProfile {
    reliability: QosReliability::Reliable,
    durability: QosDurability::Volatile,
    history: QosHistory::KeepLast(NonZeroUsize::new(1000).unwrap()),
    deadline: Duration::INFINITE,
    lifespan: Duration::INFINITE,
    liveliness: QosLiveliness::Automatic,
    liveliness_lease_duration: Duration::INFINITE,
};

pub const QOS_SERVICES: QosProfile = QosProfile {
    reliability: QosReliability::Reliable,
    durability: QosDurability::Volatile,
    history: QosHistory::KeepLast(NonZeroUsize::new(10).unwrap()),
    deadline: Duration::INFINITE,
    lifespan: Duration::INFINITE,
    liveliness: QosLiveliness::Automatic,
    liveliness_lease_duration: Duration::INFINITE,
};

/// Python QoS profile class with type-safe construction and presets.
///
/// Example:
///     qos = QosProfile(reliability="best_effort", history="keep_last", depth=5)
///     pub = node.create_publisher("/topic", String, qos=qos)
///
///     # Or use presets:
///     pub = node.create_publisher("/topic", String, qos=QosProfile.sensor_data())
#[pyclass(name = "QosProfile")]
#[derive(Clone)]
pub struct PyQosProfile {
    pub(crate) inner: QosProfile,
}

#[pymethods]
impl PyQosProfile {
    /// Create a QoS profile with the given parameters.
    ///
    /// All parameters are optional and default to QOS_DEFAULT values.
    #[new]
    #[pyo3(signature = (
        reliability=None,
        durability=None,
        history=None,
        depth=None,
        liveliness=None,
        deadline=None,
        lifespan=None,
        liveliness_lease_duration=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        reliability: Option<String>,
        durability: Option<String>,
        history: Option<String>,
        depth: Option<usize>,
        liveliness: Option<String>,
        deadline: Option<f64>,
        lifespan: Option<f64>,
        liveliness_lease_duration: Option<f64>,
    ) -> PyResult<Self> {
        let mut qos = QOS_DEFAULT;

        if let Some(rel) = reliability {
            qos.reliability = parse_reliability(&rel)?;
        }
        if let Some(dur) = durability {
            qos.durability = parse_durability(&dur)?;
        }
        if let Some(hist) = history {
            qos.history = parse_history(&hist, depth)?;
        } else if let Some(d) = depth {
            // depth without history implies keep_last
            let non_zero = NonZeroUsize::new(d).ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err("depth must be greater than 0")
            })?;
            qos.history = QosHistory::KeepLast(non_zero);
        }
        if let Some(live) = liveliness {
            qos.liveliness = parse_liveliness(&live)?;
        }
        if let Some(secs) = deadline {
            qos.deadline = duration_from_secs(secs);
        }
        if let Some(secs) = lifespan {
            qos.lifespan = duration_from_secs(secs);
        }
        if let Some(secs) = liveliness_lease_duration {
            qos.liveliness_lease_duration = duration_from_secs(secs);
        }

        Ok(Self { inner: qos })
    }

    /// Default QoS: Reliable, Volatile, KeepLast(10)
    #[staticmethod]
    fn default() -> Self {
        Self { inner: QOS_DEFAULT }
    }

    /// Sensor data QoS: BestEffort, Volatile, KeepLast(5)
    #[staticmethod]
    fn sensor_data() -> Self {
        Self {
            inner: QOS_SENSOR_DATA,
        }
    }

    /// Parameters QoS: Reliable, Volatile, KeepLast(1000)
    #[staticmethod]
    fn parameters() -> Self {
        Self {
            inner: QOS_PARAMETERS,
        }
    }

    /// Services QoS: Reliable, Volatile, KeepLast(10)
    #[staticmethod]
    fn services() -> Self {
        Self {
            inner: QOS_SERVICES,
        }
    }

    #[getter]
    fn reliability(&self) -> &str {
        match self.inner.reliability {
            QosReliability::Reliable => "reliable",
            QosReliability::BestEffort => "best_effort",
            _ => "reliable",
        }
    }

    #[getter]
    fn durability(&self) -> &str {
        match self.inner.durability {
            QosDurability::Volatile => "volatile",
            QosDurability::TransientLocal => "transient_local",
            _ => "volatile",
        }
    }

    #[getter]
    fn history(&self) -> &str {
        match self.inner.history {
            QosHistory::KeepLast(_) => "keep_last",
            QosHistory::KeepAll => "keep_all",
        }
    }

    #[getter]
    fn depth(&self) -> usize {
        match self.inner.history {
            QosHistory::KeepLast(n) => n.get(),
            QosHistory::KeepAll => 0,
        }
    }

    fn __repr__(&self) -> String {
        let history = match self.inner.history {
            QosHistory::KeepLast(n) => format!("keep_last({})", n),
            QosHistory::KeepAll => "keep_all".to_string(),
        };
        format!(
            "QosProfile(reliability={}, durability={}, history={})",
            self.reliability(),
            self.durability(),
            history
        )
    }
}

// -- Parsing helpers --

fn parse_reliability(s: &str) -> PyResult<QosReliability> {
    match s {
        "reliable" => Ok(QosReliability::Reliable),
        "best_effort" => Ok(QosReliability::BestEffort),
        _ => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "Invalid reliability: '{}'. Use 'reliable' or 'best_effort'",
            s
        ))),
    }
}

fn parse_durability(s: &str) -> PyResult<QosDurability> {
    match s {
        "volatile" => Ok(QosDurability::Volatile),
        "transient_local" => Ok(QosDurability::TransientLocal),
        _ => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "Invalid durability: '{}'. Use 'volatile' or 'transient_local'",
            s
        ))),
    }
}

fn parse_history(s: &str, depth: Option<usize>) -> PyResult<QosHistory> {
    match s {
        "keep_last" => {
            let d = depth.unwrap_or(10);
            let non_zero = NonZeroUsize::new(d).ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err("depth must be greater than 0")
            })?;
            Ok(QosHistory::KeepLast(non_zero))
        }
        "keep_all" => Ok(QosHistory::KeepAll),
        _ => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "Invalid history: '{}'. Use 'keep_last' or 'keep_all'",
            s
        ))),
    }
}

fn parse_liveliness(s: &str) -> PyResult<QosLiveliness> {
    match s {
        "automatic" => Ok(QosLiveliness::Automatic),
        "manual_by_node" => Ok(QosLiveliness::ManualByNode),
        "manual_by_topic" => Ok(QosLiveliness::ManualByTopic),
        _ => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "Invalid liveliness: '{}'. Use 'automatic', 'manual_by_node', or 'manual_by_topic'",
            s
        ))),
    }
}

/// Convert float seconds to Duration
fn duration_from_secs(secs: f64) -> Duration {
    let sec = secs.trunc() as u64;
    let nsec = ((secs.fract() * 1_000_000_000.0).round() as u64).min(999_999_999);
    Duration { sec, nsec }
}

/// Convert QosProfile to Python dict (for backward-compat presets)
pub fn qos_to_pydict(py: Python, qos: &QosProfile) -> PyResult<Py<PyDict>> {
    let dict = PyDict::new_bound(py);

    let reliability = match qos.reliability {
        QosReliability::Reliable => "reliable",
        QosReliability::BestEffort => "best_effort",
        _ => "reliable",
    };
    dict.set_item("reliability", reliability)?;

    let durability = match qos.durability {
        QosDurability::Volatile => "volatile",
        QosDurability::TransientLocal => "transient_local",
        _ => "volatile",
    };
    dict.set_item("durability", durability)?;

    match qos.history {
        QosHistory::KeepLast(depth) => {
            dict.set_item("history", "keep_last")?;
            dict.set_item("depth", depth.get())?;
        }
        QosHistory::KeepAll => {
            dict.set_item("history", "keep_all")?;
        }
    }

    let liveliness = match qos.liveliness {
        QosLiveliness::Automatic => "automatic",
        QosLiveliness::ManualByNode => "manual_by_node",
        QosLiveliness::ManualByTopic => "manual_by_topic",
        _ => "automatic",
    };
    dict.set_item("liveliness", liveliness)?;

    if qos.deadline != Duration::INFINITE {
        let secs = qos.deadline.sec as f64 + (qos.deadline.nsec as f64 / 1_000_000_000.0);
        dict.set_item("deadline", secs)?;
    }

    if qos.lifespan != Duration::INFINITE {
        let secs = qos.lifespan.sec as f64 + (qos.lifespan.nsec as f64 / 1_000_000_000.0);
        dict.set_item("lifespan", secs)?;
    }

    if qos.liveliness_lease_duration != Duration::INFINITE {
        let secs = qos.liveliness_lease_duration.sec as f64
            + (qos.liveliness_lease_duration.nsec as f64 / 1_000_000_000.0);
        dict.set_item("liveliness_lease_duration", secs)?;
    }

    Ok(dict.unbind())
}

/// Convert Python dict to QosProfile (backward compat)
pub fn qos_from_pydict(dict: &Bound<'_, PyDict>) -> PyResult<QosProfile> {
    let mut qos = QOS_DEFAULT;

    if let Ok(Some(rel)) = dict.get_item("reliability") {
        let rel_str: String = rel.extract()?;
        qos.reliability = parse_reliability(&rel_str)?;
    }
    if let Ok(Some(dur)) = dict.get_item("durability") {
        let dur_str: String = dur.extract()?;
        qos.durability = parse_durability(&dur_str)?;
    }
    if let Ok(Some(hist)) = dict.get_item("history") {
        let hist_str: String = hist.extract()?;
        let depth: Option<usize> = dict.get_item("depth")?.map(|d| d.extract()).transpose()?;
        qos.history = parse_history(&hist_str, depth)?;
    }
    if let Ok(Some(live)) = dict.get_item("liveliness") {
        let live_str: String = live.extract()?;
        qos.liveliness = parse_liveliness(&live_str)?;
    }
    if let Ok(Some(deadline)) = dict.get_item("deadline") {
        let secs: f64 = deadline.extract()?;
        qos.deadline = duration_from_secs(secs);
    }
    if let Ok(Some(lifespan)) = dict.get_item("lifespan") {
        let secs: f64 = lifespan.extract()?;
        qos.lifespan = duration_from_secs(secs);
    }
    if let Ok(Some(lease)) = dict.get_item("liveliness_lease_duration") {
        let secs: f64 = lease.extract()?;
        qos.liveliness_lease_duration = duration_from_secs(secs);
    }

    Ok(qos)
}

/// Extract QoS from either PyQosProfile or dict (backward compat).
/// Used by node.rs create_publisher/create_subscriber.
pub fn extract_qos(qos: Option<&Bound<'_, PyAny>>) -> PyResult<QosProfile> {
    match qos {
        None => Ok(QOS_DEFAULT),
        Some(obj) => {
            // Try PyQosProfile first
            if let Ok(profile) = obj.extract::<PyRef<PyQosProfile>>() {
                return Ok(profile.inner);
            }
            // Fall back to dict
            if let Ok(dict) = obj.downcast::<PyDict>() {
                return qos_from_pydict(dict);
            }
            Err(pyo3::exceptions::PyTypeError::new_err(
                "qos must be a QosProfile or dict",
            ))
        }
    }
}
