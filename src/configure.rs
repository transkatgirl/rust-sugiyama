//! Configuration of the layout algorithm.
//!
//! The central type is [`Config`], which is consumed by the crate's layout
//! entry points ([`crate::from_edges`], [`crate::from_graph`],
//! [`crate::from_vertices_and_edges`] and [`crate::algorithm::start`]).
//! A configuration can be built with [`Config::default`], with struct update
//! syntax, or from environment variables via [`Config::new_from_env`].

use std::env;

use log::error;

/// Default for [`Config::minimum_length`].
pub const MINIMUM_LENGTH_DEFAULT: u32 = 1;
/// Default for [`Config::vertex_spacing`].
pub const VERTEX_SPACING_DEFAULT: f64 = 10.0;
/// Default for [`Config::dummy_vertices`].
pub const DUMMY_VERTICES_DEFAULT: bool = true;
/// Default for [`Config::ranking_type`].
pub const RANKING_TYPE_DEFAULT: RankingType = RankingType::MinimizeEdgeLength;
/// Default for [`Config::c_minimization`].
pub const C_MINIMIZATION_DEFAULT: CrossingMinimization = CrossingMinimization::Barycenter;
/// Default for [`Config::transpose`].
pub const TRANSPOSE_DEFAULT: bool = true;
/// Default for [`Config::dummy_size`].
pub const DUMMY_SIZE_DEFAULT: f64 = 1.0;
/// Default for [`Config::divide_components`].
pub const DIVIDE_COMPONENTS_DEFAULT: bool = true;
/// Default for [`Config::per_pair_separation`].
pub const PER_PAIR_SEPARATION_DEFAULT: Option<PairSeparation> = None;

const ENV_MINIMUM_LENGTH: &str = "RUST_GRAPH_MIN_LEN";
const ENV_VERTEX_SPACING: &str = "RUST_GRAPH_V_SPACING";
const ENV_DUMMY_VERTICES: &str = "RUST_GRAPH_DUMMIES";
const ENV_RANKING_TYPE: &str = "RUST_GRAPH_R_TYPE";
const ENV_CROSSING_MINIMIZATION: &str = "RUST_GRAPH_CROSS_MIN";
const ENV_TRANSPOSE: &str = "RUST_GRAPH_TRANSPOSE";
const ENV_DUMMY_SIZE: &str = "RUST_GRAPH_DUMMY_SIZE";
const ENV_DIVIDE_COMPONENTS: &str = "RUST_GRAPH_DIVIDE_COMPONENTS";
const ENV_PER_PAIR_SEPARATION: &str = "RUST_GRAPH_PER_PAIR_SEPARATION";

macro_rules! read_env {
    ($field:expr, $cb:tt, $env:ident) => {
        #[allow(unused_parens)]
        match env::var($env).map($cb) {
            Ok(Ok(v)) => $field = v,
            Ok(Err(e)) => {
                error!(target: "initialization", "{}: {e}", $env);
            }
            _ => (),
        }
    };
}

/// The explicit horizontal gaps used by per-pair separation
/// ([`Config::per_pair_separation`]).
///
/// Gaps are expressed in the same units as the vertex sizes and are measured
/// between the borders of two horizontally adjacent vertices (their minimum
/// center-to-center distance is half the sum of their widths plus the gap).
/// Gaps must be non-negative finite numbers — the layout entry points panic
/// otherwise. Note that with a gap of zero, vertices of width zero on the
/// same layer (e.g. from [`crate::from_edges`]) may be placed on the same
/// coordinate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PairSeparation {
    /// The minimum horizontal gap between two adjacent real vertices on a
    /// layer.
    pub vertex_gap: f64,
    /// The minimum horizontal gap when at least one of the two adjacent
    /// vertices is a dummy vertex, i.e. an edge routed through the layer
    /// (pairs of two dummies also use this gap). Has no effect when
    /// [`Config::dummy_vertices`] is disabled. See also
    /// [`Config::dummy_size`], which still contributes half its value to the
    /// clearance around a routed edge.
    pub edge_gap: f64,
}

/// Used to configure parameters of the graph layout.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// The minimum number of layers (ranks) an edge spans, i.e. the minimum
    /// rank difference between an edge's endpoints. Must be at least 1 and
    /// at most [`i32::MAX`] — the layout entry points panic otherwise.
    ///
    /// Note that ranks accumulate along paths: the rank of a vertex grows by
    /// this value per edge along the longest path leading to it, and every
    /// rank must fit in an [`i32`]. Values large enough that `graph depth ×
    /// minimum_length` exceeds [`i32::MAX`] overflow the rank computation
    /// even though they pass validation (a panic in debug builds, wrapped
    /// ranks and a broken layout in release builds).
    ///
    /// This setting is structural only: values greater than 1 route edges
    /// across additional intermediate ranks, but do **not** stretch the
    /// drawing vertically. The vertical gap between adjacent ranks is derived
    /// from the heights of the real vertices in each rank (plus
    /// [`Self::vertex_spacing`]); intermediate ranks that hold only dummy
    /// vertices, or no vertices at all, contribute no height. So the
    /// y-distance between two connected vertices stays the same as with a
    /// minimum length of 1. Use [`Self::vertex_spacing`] or the vertex sizes
    /// to control vertical distance.
    pub minimum_length: u32,
    /// The minimum spacing between vertices on the same layer and between
    /// layers. Must be a non-negative finite number — the layout entry
    /// points panic otherwise. Zero is allowed and means the vertex sizes
    /// alone determine the spacing.
    ///
    /// With [`Self::per_pair_separation`] set, this only controls the
    /// vertical spacing between layers; horizontal spacing then comes from
    /// the explicit [`PairSeparation`] gaps.
    pub vertex_spacing: f64,
    /// Whether to include dummy vertices when calculating the layout.
    pub dummy_vertices: bool,
    /// The absolute width of each dummy vertex, in layout units. Unlike real
    /// vertices, dummies do not get [`Self::vertex_spacing`] added as padding,
    /// so small values let long edges pass close to neighboring vertices,
    /// squishing the graph horizontally. With [`Self::per_pair_separation`]
    /// set, the clearance between a routed edge and a neighboring vertex
    /// border is [`PairSeparation::edge_gap`] plus half this width instead.
    /// Must be a non-negative finite number (validated even when
    /// [`Self::dummy_vertices`] is disabled) — the layout entry points panic
    /// otherwise. Zero is allowed and lets long edges take no horizontal
    /// space at all.
    pub dummy_size: f64,
    /// Defines how vertices are placed vertically.
    pub ranking_type: RankingType,
    /// Which heuristic to use when minimizing edge crossings.
    pub c_minimization: CrossingMinimization,
    /// Whether to attempt to further reduce crossings by swapping vertices in a
    /// layer. This may increase runtime significantly.
    pub transpose: bool,
    /// Whether to divide the graph into its weakly connected components and
    /// lay out each component in its own coordinate space (one layout per
    /// component is returned).
    ///
    /// When disabled, the graph is laid out as a whole and exactly one layout
    /// is returned (none for an empty graph). With the default
    /// [`Self::ranking_type`] ([`RankingType::MinimizeEdgeLength`]) the input
    /// must be weakly connected, or the layout code will panic. The other
    /// ranking types ([`RankingType::Up`], [`RankingType::Down`],
    /// [`RankingType::Original`]) also accept disconnected input, placing all
    /// components in one shared coordinate space.
    pub divide_components: bool,
    /// When `Some`, separates each pair of horizontally adjacent vertices by
    /// half the sum of their own widths plus an explicit gap
    /// ([`PairSeparation::vertex_gap`], or [`PairSeparation::edge_gap`] when
    /// at least one of the pair is a dummy vertex), instead of the average of
    /// the maximum vertex widths of their alignment blocks.
    ///
    /// With the default (`None`), every vertex of a block (a group of
    /// vertices on consecutive layers that are aligned vertically) is spaced
    /// as if it were as wide as the block's widest member, which keeps the
    /// spacing between two blocks uniform but can make the layout wider than
    /// necessary; horizontal clearance comes from [`Self::vertex_spacing`],
    /// which is added to every real vertex's width as padding.
    ///
    /// When set, vertices are packed tighter and [`Self::vertex_spacing`]
    /// only controls the vertical spacing (it is no longer added to vertex
    /// widths); all horizontal clearance comes from the explicit gaps. Note
    /// that an edge routed through dummy vertices then passes a real vertex
    /// at `edge_gap + dummy_size / 2` (see [`Self::dummy_size`]), and a
    /// narrow vertex on an adjacent layer may sit horizontally within the
    /// span of a wide vertex.
    pub per_pair_separation: Option<PairSeparation>,
}

impl Config {
    /// Read in configuration values from environment variables.
    ///
    /// Envs that can be set include:
    ///
    /// | ENV | values | default | description |
    /// | --- | ------ | ------- | ----------- |
    /// | RUST_GRAPH_MIN_LEN    | integer, 1..=2147483647 | 1       | minimum number of layers an edge spans (structural only, does not affect vertical spacing) |
    /// | RUST_GRAPH_V_SPACING  | float, >= 0          | 10         | minimum spacing between vertices on the same layer |
    /// | RUST_GRAPH_DUMMIES    | y \| n               | y          | if dummy vertices are included when calculating the layout (dummies never appear in the final layout; disabling them lets long edges take no horizontal space) |
    /// | RUST_GRAPH_R_TYPE     | original \| minimize \| up \| down | minimize   | defines how vertices are places vertically |
    /// | RUST_GRAPH_CROSS_MIN  | barycenter \| median \| none | barycenter | which heuristic to use for crossing reduction, or none to disable it |
    /// | RUST_GRAPH_TRANSPOSE  | y \| n               | y          | if transpose function is used to further try to reduce crossings (may increase runtime significally for large graphs) |
    /// | RUST_GRAPH_DUMMY_SIZE | float, >= 0          | 1.0        | absolute width of dummy vertices, if dummy vertices are included. small values squish the graph horizontally |
    /// | RUST_GRAPH_DIVIDE_COMPONENTS | y \| n        | y          | if the graph is divided into its connected components before layout. if disabled, the default ranking type requires a connected graph |
    /// | RUST_GRAPH_PER_PAIR_SEPARATION | n \| `<vertex_gap>,<edge_gap>` | n | separate adjacent vertices by their own widths plus an explicit border-to-border gap (the edge gap when a dummy vertex is involved) instead of by block-max widths; vertex spacing then only affects vertical spacing. produces tighter layouts. the gaps must be non-negative finite numbers |
    pub fn new_from_env() -> Self {
        let mut config = Self::default();

        let parse_bool = |x: String| match x.as_str() {
            "y" => Ok(true),
            "n" => Ok(false),
            v => Err(format!("invalid value (expected 'y' or 'n'): {v}")),
        };

        read_env!(
            config.minimum_length,
            parse_minimum_length,
            ENV_MINIMUM_LENGTH
        );

        read_env!(
            config.c_minimization,
            (TryFrom::try_from),
            ENV_CROSSING_MINIMIZATION
        );

        read_env!(config.ranking_type, (TryFrom::try_from), ENV_RANKING_TYPE);

        read_env!(
            config.vertex_spacing,
            (|x| parse_non_negative_float("vertex spacing", x)),
            ENV_VERTEX_SPACING
        );

        read_env!(config.dummy_vertices, parse_bool, ENV_DUMMY_VERTICES);

        read_env!(
            config.dummy_size,
            (|x| parse_non_negative_float("dummy size", x)),
            ENV_DUMMY_SIZE
        );

        read_env!(config.transpose, parse_bool, ENV_TRANSPOSE);

        read_env!(config.divide_components, parse_bool, ENV_DIVIDE_COMPONENTS);

        read_env!(
            config.per_pair_separation,
            parse_pair_separation,
            ENV_PER_PAIR_SEPARATION
        );

        config
    }

    /// Panics with a descriptive message if the configuration violates an
    /// invariant the layout code relies on: [`Self::minimum_length`] must be
    /// between 1 and [`i32::MAX`], and [`Self::vertex_spacing`],
    /// [`Self::dummy_size`] and the
    /// [`PairSeparation`] gaps must be non-negative finite numbers. Called by
    /// the layout entry points, so configurations built with struct update
    /// syntax are checked too (the environment variable parsers of
    /// [`Self::new_from_env`] share these checks).
    pub(crate) fn validate(&self) {
        if let Err(e) = check_minimum_length(self.minimum_length) {
            panic!("Config::minimum_length {e}");
        }
        if let Err(e) = check_non_negative_float(self.vertex_spacing) {
            panic!("Config::vertex_spacing {e}");
        }
        if let Err(e) = check_non_negative_float(self.dummy_size) {
            panic!("Config::dummy_size {e}");
        }
        if let Some(gaps) = self.per_pair_separation {
            for (name, gap) in [("vertex_gap", gaps.vertex_gap), ("edge_gap", gaps.edge_gap)] {
                if let Err(e) = check_non_negative_float(gap) {
                    panic!("PairSeparation::{name} {e}");
                }
            }
        }
    }
}

/// The bound shared by [`parse_minimum_length`] and [`Config::validate`]:
/// an integer between 1 and [`i32::MAX`].
fn check_minimum_length(value: u32) -> Result<(), String> {
    if value < 1 {
        Err(format!("must be at least 1, got {value}"))
    } else if value > i32::MAX as u32 {
        Err(format!("must be at most {}, got {value}", i32::MAX))
    } else {
        Ok(())
    }
}

/// The bound shared by [`parse_non_negative_float`],
/// [`parse_pair_separation`] and [`Config::validate`]: a non-negative
/// finite number.
fn check_non_negative_float(value: f64) -> Result<(), String> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(format!("must be a non-negative finite number, got {value}"))
    }
}

/// Parses the value of [`ENV_MINIMUM_LENGTH`]: an integer between 1 and
/// [`i32::MAX`].
fn parse_minimum_length(value: String) -> Result<u32, String> {
    match value.parse::<u32>() {
        Ok(v) => check_minimum_length(v)
            .map(|()| v)
            .map_err(|e| format!("Invalid minimum length env: {e}")),
        Err(e) => Err(format!("Invalid minimum length env: {e}")),
    }
}

/// Parses the value of [`ENV_VERTEX_SPACING`] and [`ENV_DUMMY_SIZE`]: a
/// non-negative finite number.
fn parse_non_negative_float(name: &str, value: String) -> Result<f64, String> {
    match value.parse::<f64>() {
        Ok(v) => check_non_negative_float(v)
            .map(|()| v)
            .map_err(|e| format!("Invalid {name} env: {e}")),
        Err(e) => Err(format!("Invalid {name} env: {e}")),
    }
}

/// Parses the value of [`ENV_PER_PAIR_SEPARATION`]: `n` disables per-pair
/// separation, `<vertex_gap>,<edge_gap>` (e.g. `10,5`) enables it with the
/// given gaps. The gaps must be non-negative finite numbers.
fn parse_pair_separation(value: String) -> Result<Option<PairSeparation>, String> {
    if value == "n" {
        return Ok(None);
    }
    let (vertex_gap, edge_gap) = value.split_once(',').ok_or_else(|| {
        format!(
            "Invalid argument for per pair separation env \
             (expected 'n' or '<vertex_gap>,<edge_gap>'): {value}"
        )
    })?;
    let vertex_gap = vertex_gap
        .trim()
        .parse::<f64>()
        .map_err(|e| format!("Invalid vertex gap for per pair separation env: {e}"))?;
    let edge_gap = edge_gap
        .trim()
        .parse::<f64>()
        .map_err(|e| format!("Invalid edge gap for per pair separation env: {e}"))?;
    for (name, gap) in [("vertex", vertex_gap), ("edge", edge_gap)] {
        if let Err(e) = check_non_negative_float(gap) {
            return Err(format!(
                "Invalid {name} gap for per pair separation env: {e}"
            ));
        }
    }
    Ok(Some(PairSeparation {
        vertex_gap,
        edge_gap,
    }))
}

impl Default for Config {
    fn default() -> Self {
        Self {
            minimum_length: MINIMUM_LENGTH_DEFAULT,
            vertex_spacing: VERTEX_SPACING_DEFAULT,
            dummy_vertices: DUMMY_VERTICES_DEFAULT,
            ranking_type: RANKING_TYPE_DEFAULT,
            c_minimization: C_MINIMIZATION_DEFAULT,
            transpose: TRANSPOSE_DEFAULT,
            dummy_size: DUMMY_SIZE_DEFAULT,
            divide_components: DIVIDE_COMPONENTS_DEFAULT,
            per_pair_separation: PER_PAIR_SEPARATION_DEFAULT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimum_length_bounds() {
        assert!(parse_minimum_length("0".to_string()).is_err());
        assert_eq!(parse_minimum_length("1".to_string()), Ok(1));
        assert_eq!(
            parse_minimum_length("2147483647".to_string()),
            Ok(i32::MAX as u32)
        );
        assert!(parse_minimum_length("2147483648".to_string()).is_err());
        assert!(parse_minimum_length("abc".to_string()).is_err());
    }

    #[test]
    fn parse_pair_separation_grammar() {
        assert_eq!(parse_pair_separation("n".to_string()), Ok(None));
        assert_eq!(
            parse_pair_separation("10,5".to_string()),
            Ok(Some(PairSeparation {
                vertex_gap: 10.0,
                edge_gap: 5.0
            }))
        );
        assert_eq!(
            parse_pair_separation(" 1.5 , 0 ".to_string()),
            Ok(Some(PairSeparation {
                vertex_gap: 1.5,
                edge_gap: 0.0
            }))
        );
        assert!(parse_pair_separation("y".to_string()).is_err());
        assert!(parse_pair_separation("10".to_string()).is_err());
        assert!(parse_pair_separation("-1,5".to_string()).is_err());
        assert!(parse_pair_separation("NaN,5".to_string()).is_err());
        assert!(parse_pair_separation("inf,5".to_string()).is_err());
    }

    #[test]
    fn validate_accepts_default() {
        Config::default().validate();
    }

    #[test]
    fn validate_accepts_zero_vertex_spacing() {
        Config {
            vertex_spacing: 0.0,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    fn parse_non_negative_float_accepts_zero() {
        assert_eq!(
            parse_non_negative_float("vertex spacing", "0".to_string()),
            Ok(0.0)
        );
        assert!(parse_non_negative_float("vertex spacing", "-1".to_string()).is_err());
    }

    #[test]
    #[should_panic(expected = "minimum_length must be at least 1")]
    fn validate_rejects_zero_minimum_length() {
        Config {
            minimum_length: 0,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "minimum_length must be at most")]
    fn validate_rejects_oversized_minimum_length() {
        Config {
            minimum_length: i32::MAX as u32 + 1,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "non-negative finite")]
    fn validate_rejects_nan_gap() {
        Config {
            per_pair_separation: Some(PairSeparation {
                vertex_gap: f64::NAN,
                edge_gap: 0.0,
            }),
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "non-negative finite")]
    fn validate_rejects_negative_gap() {
        Config {
            per_pair_separation: Some(PairSeparation {
                vertex_gap: 1.0,
                edge_gap: -0.5,
            }),
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "vertex_spacing must be")]
    fn validate_rejects_nan_vertex_spacing() {
        Config {
            vertex_spacing: f64::NAN,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    fn validate_accepts_zero_dummy_size() {
        Config {
            dummy_size: 0.0,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "dummy_size must be")]
    fn validate_rejects_negative_dummy_size() {
        Config {
            dummy_size: -1.0,
            ..Default::default()
        }
        .validate();
    }
}

/// Defines the Ranking type, i.e. how vertices are placed on each layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RankingType {
    /// Places each vertex halfway between the highest and lowest rank it can
    /// occupy (the midpoint of the [`Self::Up`] and [`Self::Down`] rankings)
    Original,
    /// Tries to minimize edge lengths across layers
    MinimizeEdgeLength,
    /// Move vertices as far up as possible
    Up,
    /// Move vertices as far down as possible
    Down,
}

impl TryFrom<String> for RankingType {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "original" => Ok(Self::Original),
            "minimize" => Ok(Self::MinimizeEdgeLength),
            "up" => Ok(Self::Up),
            "down" => Ok(Self::Down),
            s => Err(format!("invalid value for ranking type: {s}")),
        }
    }
}

impl From<RankingType> for &'static str {
    fn from(value: RankingType) -> Self {
        match value {
            RankingType::Up => "up",
            RankingType::Down => "down",
            RankingType::Original => "original",
            RankingType::MinimizeEdgeLength => "minimize",
        }
    }
}

/// Defines the heuristic used for crossing minimization.
/// During crossing minimization, the vertices of one layer are
/// ordered, so they're as close to neighboring vertices as possible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossingMinimization {
    /// Calculates the average of the positions of adjacent neighbors
    Barycenter,
    /// Calculates the weighted median of the positions of adjacent neighbors
    Median,
    /// Disables crossing minimization: vertices keep the initial order
    /// determined by a depth first search. [`Config::transpose`] has no
    /// effect with this setting, since it is part of the minimization sweep.
    None,
}

impl TryFrom<String> for CrossingMinimization {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "barycenter" => Ok(Self::Barycenter),
            "median" => Ok(Self::Median),
            "none" => Ok(Self::None),
            s => Err(format!("invalid value for crossing minimization: {s}")),
        }
    }
}

impl From<CrossingMinimization> for &'static str {
    fn from(value: CrossingMinimization) -> Self {
        match value {
            CrossingMinimization::Median => "median",
            CrossingMinimization::Barycenter => "barycenter",
            CrossingMinimization::None => "none",
        }
    }
}

/// Serializes tests that mutate the process-global environment.
#[cfg(test)]
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Removes all `RUST_GRAPH_*` variables from the environment, so tests
/// holding [`ENV_LOCK`] start from a clean slate regardless of order.
#[cfg(test)]
fn clear_env() {
    for var in [
        ENV_MINIMUM_LENGTH,
        ENV_VERTEX_SPACING,
        ENV_DUMMY_VERTICES,
        ENV_RANKING_TYPE,
        ENV_CROSSING_MINIMIZATION,
        ENV_TRANSPOSE,
        ENV_DUMMY_SIZE,
        ENV_DIVIDE_COMPONENTS,
        ENV_PER_PAIR_SEPARATION,
    ] {
        env::remove_var(var);
    }
}

#[test]
fn from_env_all_valid() {
    use std::env;
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_env();
    env::set_var(ENV_MINIMUM_LENGTH, "5");
    env::set_var(ENV_DUMMY_VERTICES, "y");
    env::set_var(ENV_DUMMY_SIZE, "0.1");
    env::set_var(ENV_RANKING_TYPE, "up");
    env::set_var(ENV_CROSSING_MINIMIZATION, "median");
    env::set_var(ENV_TRANSPOSE, "n");
    env::set_var(ENV_VERTEX_SPACING, "20");
    env::set_var(ENV_DIVIDE_COMPONENTS, "n");
    env::set_var(ENV_PER_PAIR_SEPARATION, "10,5");
    let cfg = Config::new_from_env();
    assert_eq!(cfg.minimum_length, 5);
    assert_eq!(cfg.dummy_vertices, true);
    assert_eq!(cfg.dummy_size, 0.1);
    assert_eq!(cfg.ranking_type, RankingType::Up);
    assert_eq!(cfg.c_minimization, CrossingMinimization::Median);
    assert_eq!(cfg.transpose, false);
    assert_eq!(cfg.vertex_spacing, 20.0);
    assert_eq!(cfg.divide_components, false);
    assert_eq!(
        cfg.per_pair_separation,
        Some(PairSeparation {
            vertex_gap: 10.0,
            edge_gap: 5.0
        })
    );
}

#[test]
fn parse_pair_separation_values() {
    assert_eq!(parse_pair_separation("n".to_string()), Ok(None));
    assert_eq!(
        parse_pair_separation("10,5".to_string()),
        Ok(Some(PairSeparation {
            vertex_gap: 10.0,
            edge_gap: 5.0
        }))
    );
    assert_eq!(
        parse_pair_separation("10.5, 0.25".to_string()),
        Ok(Some(PairSeparation {
            vertex_gap: 10.5,
            edge_gap: 0.25
        }))
    );
    assert!(parse_pair_separation("y".to_string()).is_err());
    assert!(parse_pair_separation("10".to_string()).is_err());
    assert!(parse_pair_separation("a,b".to_string()).is_err());
    assert!(parse_pair_separation(String::new()).is_err());
    assert!(parse_pair_separation("nan,5".to_string()).is_err());
    assert!(parse_pair_separation("5,nan".to_string()).is_err());
    assert!(parse_pair_separation("inf,5".to_string()).is_err());
    assert!(parse_pair_separation("-1,5".to_string()).is_err());
    assert!(parse_pair_separation("5,-1".to_string()).is_err());
}

#[test]
fn parse_dummy_size_values() {
    assert_eq!(
        parse_non_negative_float("dummy size", "10".to_string()),
        Ok(10.0)
    );
    assert_eq!(
        parse_non_negative_float("dummy size", "0.5".to_string()),
        Ok(0.5)
    );
    assert_eq!(
        parse_non_negative_float("dummy size", "0".to_string()),
        Ok(0.0)
    );
    assert!(parse_non_negative_float("dummy size", "-1".to_string()).is_err());
    assert!(parse_non_negative_float("dummy size", "nan".to_string()).is_err());
    assert!(parse_non_negative_float("dummy size", "inf".to_string()).is_err());
    assert!(parse_non_negative_float("dummy size", "abc".to_string()).is_err());
}

#[test]
fn parse_minimum_length_values() {
    assert_eq!(parse_minimum_length("1".to_string()), Ok(1));
    assert_eq!(parse_minimum_length("5".to_string()), Ok(5));
    assert!(parse_minimum_length("0".to_string()).is_err());
    assert!(parse_minimum_length("-1".to_string()).is_err());
    assert!(parse_minimum_length("abc".to_string()).is_err());
}

#[test]
fn from_env_invalid_value() {
    use std::env;

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_env();
    env::set_var(ENV_CROSSING_MINIMIZATION, "flubbeldiflap");
    env::set_var(ENV_VERTEX_SPACING, "1bleh0");
    env::set_var(ENV_DUMMY_SIZE, "-1");
    let cfg = Config::new_from_env();
    let default = Config::default();
    assert_eq!(default.c_minimization, cfg.c_minimization);
    assert_eq!(default.vertex_spacing, cfg.vertex_spacing);
    assert_eq!(default.dummy_size, cfg.dummy_size);
}
