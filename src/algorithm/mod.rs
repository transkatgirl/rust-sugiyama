//! The Sugiyama-style layout pipeline and its individual phases.
//!
//! The implementation roughly follows Sugiyama's algorithm for creating
//! a layered graph layout, split into four phases:
//!
//! 0. [`p0_cycle_removal`] — make the graph acyclic by reversing a feedback
//!    arc set.
//! 1. [`p1_layering`] — assign each vertex to a rank (layer).
//! 2. [`p2_reduce_crossings`] — reorder the vertices within each rank to
//!    reduce edge crossings.
//! 3. [`p3_calculate_coordinates`] — calculate the final coordinates.
//!
//! The whole algorithm roughly follows the 1993 paper "A technique for drawing
//! directed graphs" by Gansner et al. It can be found
//! [here](https://ieeexplore.ieee.org/document/221135).
//! See the submodules for each phase for more details on the implementation
//! and references used.
//!
//! The usual entry point is [`start`] (or the crate-level `from_*` functions
//! wrapping it), which runs all phases on a [`StableDiGraph<Vertex, Edge>`].
//! The phases can also be driven manually; every phase mutates the [`Vertex`]
//! and [`Edge`] weights of the graph in place and relies on the phases before
//! it having run:
//!
//! ```
//! use rust_sugiyama_fork::algorithm::{self, Edge, Vertex};
//! use rust_sugiyama_fork::configure::{CrossingMinimization, RankingType};
//! use rust_sugiyama_fork::petgraph::stable_graph::StableDiGraph;
//!
//! let mut graph: StableDiGraph<Vertex, Edge> =
//!     StableDiGraph::from_edges([(0, 1), (0, 2), (1, 3), (2, 3), (3, 0)]);
//! // give every vertex a size, including the spacing you want around it
//! // (start() derives this from Config::vertex_spacing instead)
//! for v in graph.node_weights_mut() {
//!     v.size = (10.0, 10.0);
//! }
//!
//! // normalization: strips self-loops and seeds the per-vertex state
//! algorithm::init_graph(&mut graph);
//! // phase 0: break the cycle 0 -> 1 -> 3 -> 0
//! let reversed = algorithm::p0_cycle_removal::remove_cycles(&mut graph);
//! assert!(!reversed.is_empty());
//! // phase 1: assign Vertex::rank
//! algorithm::p1_layering::rank(&mut graph, 1, RankingType::MinimizeEdgeLength);
//! // phase 2: insert dummies for long edges, then order the layers
//! algorithm::p2_reduce_crossings::insert_dummy_vertices(&mut graph, 1.0);
//! let mut layers =
//!     algorithm::p2_reduce_crossings::ordering(&mut graph, CrossingMinimization::Barycenter, true);
//! // phase 3: four Brandes-Köpf candidate layouts, combined into x-coordinates
//! let mut layouts =
//!     algorithm::p3_calculate_coordinates::create_layouts(&mut graph, &mut layers, None);
//! algorithm::p3_calculate_coordinates::align_to_smallest_width_layout(&graph, &mut layouts);
//! let x_coordinates = algorithm::p3_calculate_coordinates::calculate_relative_coords(layouts);
//! assert_eq!(x_coordinates.len(), graph.node_count());
//! ```
use std::collections::{BTreeMap, HashMap};

use log::{debug, info};
use petgraph::stable_graph::{EdgeIndex, NodeIndex, StableDiGraph};

use crate::configure::{Config, CrossingMinimization, PairSeparation, RankingType};
use crate::{util::weakly_connected_components, Layout, Layouts};
use p0_cycle_removal as p0;
use p1_layering as p1;
use p2_reduce_crossings as p2;
use p3_calculate_coordinates as p3;

use self::p3_calculate_coordinates::VDir;

pub mod p0_cycle_removal;
pub mod p1_layering;
pub mod p2_reduce_crossings;
pub mod p3_calculate_coordinates;

/// The vertex weight threaded through all phases of the algorithm.
///
/// Construct it with [`Vertex::new`] or [`Vertex::default`]; besides the
/// public fields it carries internal scratch state for the network simplex
/// (phase 1) and Brandes-Köpf (phase 3) machinery, so it has no public
/// struct literal. [`init_graph`] must run once on the finished graph before
/// any phase: it strips self-loops, overwrites [`Vertex::id`] with the
/// vertex's node index, and initializes the phase 3 alignment state. It
/// does **not** reset the phase 1 network simplex state — see the
/// preconditions of [`p1_layering::rank`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    /// An identifier for the vertex. [`init_graph`] sets it to the vertex's
    /// node index in the graph, and the ids reported in the final
    /// [`Layout`] are these node indices. [`execute_phase_3`] also re-stamps
    /// the ids of dummy vertices with their node indices; dummies created by
    /// [`p2_reduce_crossings::insert_dummy_vertices`] otherwise keep the
    /// default id `0`, so when driving the phases manually the ids of dummy
    /// vertices are not meaningful. Dummy vertices are never part of the
    /// final [`Layout`].
    pub id: usize,
    /// The width and height of the vertex, in the units the final
    /// coordinates are expressed in.
    ///
    /// [`start`] (via [`build_layout`]) adds [`Config::vertex_spacing`] to
    /// both components before running the phases — only to the height when
    /// [`Config::per_pair_separation`] is set, since horizontal clearance
    /// then comes from the explicit gaps — so each vertex carries its
    /// spacing as padding. When driving the phases manually, include any
    /// desired spacing in the size yourself. Dummy vertices are created with
    /// a size of `(dummy_size, 0.0)`.
    pub size: (f64, f64),
    /// The rank (layer) of the vertex; assigned by phase 1
    /// ([`p1_layering::rank`]), 0-based for the topmost rank after
    /// normalization. Read by all later phases.
    pub rank: i32,
    /// The position of the vertex within its rank, from left to right.
    /// Maintained by phase 3 while it evaluates the layer ordering produced
    /// by phase 2; not kept in sync with an [`p2_reduce_crossings::Order`]
    /// during the phase-2 sweeps themselves.
    pub pos: usize,
    // network simplex state (phase 1): postorder interval [low, lim] and
    // parent of the vertex in the spanning tree, only meaningful while
    // p1_layering's MinimizeEdgeLength ranking runs
    pub(crate) low: u32,
    pub(crate) lim: u32,
    pub(crate) parent: Option<NodeIndex>,
    pub(crate) is_tree_vertex: bool,
    /// Whether this is a dummy vertex: a placeholder inserted by phase 2
    /// ([`p2_reduce_crossings::insert_dummy_vertices`]) to break an edge
    /// spanning more than one rank into unit segments. Dummy vertices are
    /// excluded from the final [`Layout`].
    pub is_dummy: bool,
    // Brandes-Köpf state (phase 3): block root, alignment successor, class
    // sink and shift, only meaningful inside one of the four alignment passes
    // (reset by reset_alignment) plus the separation width used during
    // horizontal compaction
    pub(crate) root: NodeIndex,
    pub(crate) align: NodeIndex,
    pub(crate) shift: f64,
    pub(crate) sink: NodeIndex,
    pub(crate) separation_width: f64,
}

impl Vertex {
    /// Creates a new vertex with the given id and size, all remaining state
    /// as in [`Vertex::default`]. Note that [`init_graph`] — run by [`start`]
    /// and required before driving the phases manually — overwrites the id
    /// with the vertex's node index; see [`Vertex::id`].
    pub fn new(id: usize, size: (f64, f64)) -> Self {
        Self {
            id,
            size,
            ..Default::default()
        }
    }
}

impl Default for Vertex {
    fn default() -> Self {
        Self {
            id: 0,
            size: (0.0, 0.0),
            rank: 0,
            pos: 0,
            low: 0,
            lim: 0,
            parent: None,
            is_tree_vertex: false,
            is_dummy: false,
            root: 0.into(),
            align: 0.into(),
            shift: f64::INFINITY,
            sink: 0.into(),
            separation_width: 0.0,
        }
    }
}

/// The edge weight threaded through all phases of the algorithm.
///
/// Construct it with [`Edge::default`]; besides [`Edge::weight`] it carries
/// internal scratch state for the network simplex (phase 1) and Brandes-Köpf
/// (phase 3) machinery, so it has no public struct literal.
#[derive(Clone, Copy, Debug)]
pub struct Edge {
    /// The weight of the edge, used by the network simplex ranking
    /// ([`crate::configure::RankingType::MinimizeEdgeLength`]) which
    /// minimizes the weighted sum of edge lengths. Defaults to 1; higher
    /// weights make an edge more likely to be drawn short and straight.
    pub weight: i32,
    // network simplex state (phase 1): cut value of the edge (Some only for
    // spanning tree edges once cut values are computed) and tree membership
    pub(crate) cut_value: Option<i32>,
    pub(crate) is_tree_edge: bool,
    // Brandes-Köpf state (phase 3): whether the edge crosses an inner
    // segment (set by mark_type_1_conflicts, making it ineligible for
    // vertical alignment)
    pub(crate) has_type_1_conflict: bool,
}

impl Default for Edge {
    fn default() -> Self {
        Self {
            weight: 1,
            cut_value: None,
            is_tree_edge: false,
            has_type_1_conflict: false,
        }
    }
}

/// Runs the whole layout pipeline on a prebuilt graph.
///
/// This is the entry point the crate-level `from_*` functions delegate to.
/// The input may contain cycles (broken in phase 0), self-loops (stripped by
/// [`init_graph`]) and, with [`Config::divide_components`] enabled or a
/// ranking type other than
/// [`RankingType::MinimizeEdgeLength`], disconnected parts. An empty graph
/// yields an empty vec.
///
/// The ids reported in the returned layouts are the graph's node indices
/// ([`init_graph`] overwrites [`Vertex::id`] with them).
///
/// # Panics
///
/// Panics if the graph is disconnected while
/// [`Config::divide_components`] is disabled and
/// [`Config::ranking_type`] is [`RankingType::MinimizeEdgeLength`], or if
/// the configuration is invalid (see [`build_layout`]).
pub fn start(mut graph: StableDiGraph<Vertex, Edge>, config: &Config) -> Layouts<usize> {
    // validate up front so the documented contract also holds when the graph
    // is empty and no component ever reaches build_layout
    config.validate();
    init_graph(&mut graph);
    if config.divide_components {
        weakly_connected_components(graph)
            .into_iter()
            .map(|g| build_layout(g, config))
            .collect()
    } else if graph.node_count() == 0 {
        // the documented contract is no layouts for an empty graph
        // (build_layout would return a single empty layout instead)
        Vec::new()
    } else {
        vec![build_layout(graph, config)]
    }
}

/// Normalizes a graph for the layout pipeline: strips self-loops and seeds
/// the per-vertex state ([`Vertex::id`] and the internal alignment state are
/// set from each vertex's node index).
///
/// Must be called once on the finished graph before running any phase;
/// [`start`] does so automatically.
pub fn init_graph(graph: &mut StableDiGraph<Vertex, Edge>) {
    // Self-loops cannot be broken by the edge reversal in phase 0 and have no
    // influence on the layout (only vertex positions are computed), so drop
    // them here as input normalization; the vertex itself is kept.
    let edge_count_before = graph.edge_count();
    graph.retain_edges(|g, e| {
        let (tail, head) = g.edge_endpoints(e).unwrap();
        tail != head
    });
    let removed = edge_count_before - graph.edge_count();
    if removed > 0 {
        info!("Removed {removed} self-loop(s) from the graph");
    }

    info!("Initializing graphs vertex weights");
    for id in graph.node_indices().collect::<Vec<_>>() {
        graph[id].id = id.index();
        graph[id].root = id;
        graph[id].align = id;
        graph[id].sink = id;
    }
}

/// Runs all four phases on one graph, producing a single [`Layout`] in one
/// coordinate space. An empty graph yields an empty layout.
///
/// The graph must have been prepared with [`init_graph`] first — unlike
/// [`start`], this does **not** run it automatically. [`init_graph`] strips
/// self-loops and stamps [`Vertex::id`] with the node indices reported in
/// the returned layout; without it every real vertex reports the default id
/// `0`, so the layout cannot be attributed to vertices, and a self-loop in
/// the input makes phase 0 fail.
///
/// Unlike [`start`] this also does not divide the graph into its weakly
/// connected components first; with [`Config::ranking_type`] set to
/// [`RankingType::MinimizeEdgeLength`] the graph must be weakly connected.
///
/// Note that this adds [`Config::vertex_spacing`] to every vertex size (both
/// components, or only the height with [`Config::per_pair_separation`]; see
/// [`Vertex::size`]), so it is not idempotent — call it once per graph.
///
/// # Panics
///
/// Panics if the configuration is invalid: [`Config::minimum_length`] must
/// be between 1 and [`i32::MAX`], [`Config::dummy_size`] must be a positive
/// finite number, and [`Config::vertex_spacing`] and the [`PairSeparation`]
/// gaps must be non-negative finite numbers.
pub fn build_layout(mut graph: StableDiGraph<Vertex, Edge>, config: &Config) -> Layout {
    config.validate();
    if graph.node_count() == 0 {
        return (Vec::new(), 0.0, 0.0);
    }
    info!(target: "layouting", "Start building layout");
    info!(target: "layouting", "Configuration is: {:?}", config);

    // Without per-pair separation, the vertex spacing is horizontal padding baked into each node:
    // each node takes 50% of the "responsibility" of the vertex spacing (dummy vertices, created
    // later without this padding, get 50% of the spacing to their neighbors). With per-pair
    // separation the horizontal clearance comes entirely from the explicit gaps, so only the
    // height is padded (the vertical spacing between ranks).
    let pad_width = config.per_pair_separation.is_none();
    for vertex in graph.node_weights_mut() {
        if pad_width {
            vertex.size.0 += config.vertex_spacing;
        }
        vertex.size.1 += config.vertex_spacing;
    }

    // we don't remember the edges that where reversed for now, since they are
    // currently not needed
    let _ = execute_phase_0(&mut graph);

    execute_phase_1(
        &mut graph,
        config.minimum_length as i32,
        config.ranking_type,
    );

    let layers = execute_phase_2(
        &mut graph,
        config.dummy_vertices.then_some(config.dummy_size),
        config.c_minimization,
        config.transpose,
    );

    let layout = execute_phase_3(&mut graph, layers, config.per_pair_separation);
    debug!(target: "layouting", "Coordinates: {:?}\nwidth: {}, height:{}",
        layout.0,
        layout.1,
        layout.2
    );
    layout
}

/// Executes phase 0 ([`p0_cycle_removal`]): makes the graph acyclic by
/// reversing the edges of a feedback arc set. Returns the indices of the
/// newly added reversed edges. Requires [`init_graph`] to have run (no
/// self-loops).
pub fn execute_phase_0(graph: &mut StableDiGraph<Vertex, Edge>) -> Vec<EdgeIndex> {
    info!(target: "layouting", "Executing phase 0: Cycle Removal");
    p0::remove_cycles(graph)
}

/// Executes phase 1 ([`p1_layering`]): assigns each vertex a rank
/// ([`Vertex::rank`]). Requires an acyclic graph (run phase 0 first).
pub fn execute_phase_1(
    graph: &mut StableDiGraph<Vertex, Edge>,
    minimum_length: i32,
    ranking_type: RankingType,
) {
    info!(target: "layouting", "Executing phase 1: Ranking");
    p1::rank(graph, minimum_length, ranking_type);
}

/// Executes phase 2 ([`p2_reduce_crossings`]): reorders vertices in ranks to
/// reduce crossings, returning the layers top-to-bottom with each layer
/// ordered left-to-right. If `dummy_size` is [Some], dummies will be passed
/// along to the next phase (and are part of the returned layers). Requires
/// ranks from phase 1.
///
/// In both modes, edges spanning more than one rank are destructively
/// replaced ([`p2_reduce_crossings::insert_dummy_vertices`]): with
/// `dummy_size` [`Some`] by chains of dummy vertices and unit edges, with
/// [`None`] — after the ordering — recreated as direct edges with default
/// weights ([`p2_reduce_crossings::remove_dummy_vertices`]). Custom
/// [`Edge::weight`]s of such edges are not preserved and their edge indices
/// are invalidated either way.
///
/// # Panics
///
/// Panics if the graph is empty, or — with `dummy_size` [`None`] — if the
/// graph is cyclic (via [`p2_reduce_crossings::remove_dummy_vertices`]).
pub fn execute_phase_2(
    graph: &mut StableDiGraph<Vertex, Edge>,
    dummy_size: Option<f64>,
    crossing_minimization: CrossingMinimization,
    transpose: bool,
) -> Vec<Vec<NodeIndex>> {
    info!(target: "layouting", "Executing phase 2: Crossing Reduction");
    info!(target: "layouting",
        "dummy vertex size: {:?}, heuristic for crossing minimization: {:?}, using transpose: {}",
        dummy_size,
        crossing_minimization,
        transpose
    );

    p2::insert_dummy_vertices(graph, dummy_size.unwrap_or(0.0));
    let mut order = p2::ordering(graph, crossing_minimization, transpose);
    if dummy_size.is_none() {
        p2::remove_dummy_vertices(graph, &mut order);
    }
    order
}

/// Executes phase 3 ([`p3_calculate_coordinates`]): calculates the final
/// coordinates for each vertex, after the graph was layered and crossings
/// were minimized, and assembles the [`Layout`] (real vertices only, minimum
/// x-coordinate shifted to 0, y-coordinates from stacking the per-rank
/// maximum heights).
///
/// `layers` must be the layering produced by phase 2 and cover every vertex
/// of the graph; individual empty layers are tolerated (they are dropped
/// after the reported layer count is recorded), but the graph itself must
/// contain at least one real (non-dummy) vertex.
///
/// `per_pair_separation` selects the separation mode of the phase; see
/// [`Config::per_pair_separation`].
///
/// # Panics
///
/// Panics if the graph contains no real (non-dummy) vertices.
pub fn execute_phase_3(
    graph: &mut StableDiGraph<Vertex, Edge>,
    mut layers: Vec<Vec<NodeIndex>>,
    per_pair_separation: Option<PairSeparation>,
) -> Layout {
    info!(target: "layouting", "Executing phase 3: Coordinate Calculation");
    for n in graph.node_indices().collect::<Vec<_>>() {
        if graph[n].is_dummy {
            graph[n].id = n.index();
        }
    }
    let width = layers.iter().map(|l| l.len()).max().unwrap_or(0) as f64;
    let height = layers.len() as f64;
    // removing dummy vertices can leave empty ranks behind (when dummies are
    // disabled and minimum_length > 1); drop them after the layer count is
    // recorded, so the rest of the phase can rely on every layer being
    // occupied
    layers.retain(|l| !l.is_empty());
    let mut layouts = p3::create_layouts(graph, &mut layers, per_pair_separation);

    p3::align_to_smallest_width_layout(graph, &mut layouts);
    // dummy vertices are never part of the returned layout and nothing below
    // reads their positions, so drop them before the shift: the minimum is
    // then taken over exactly the vertices the layout reports
    let mut x_coordinates: Vec<_> = p3::calculate_relative_coords(layouts)
        .into_iter()
        .filter(|(v, _)| !graph[*v].is_dummy)
        .collect();
    // determine the smallest x-coordinate
    let min = x_coordinates
        .iter()
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap()
        .1;

    // shift all coordinates so the minimum coordinate is 0
    for (_, c) in &mut x_coordinates {
        *c -= min;
    }

    // Find max y size in each rank. Use a BTreeMap so iteration through the map
    // is ordered.
    let mut rank_to_max_height = BTreeMap::<i32, f64>::new();
    for vertex in graph.node_weights() {
        let max = rank_to_max_height.entry(vertex.rank).or_default();
        *max = max.max(vertex.size.1);
    }

    // Stack up each rank to assign it an offset. The gap between each rank and the next is half the
    // height of the current rank, plus half the height of the next rank.
    let mut rank_to_y_offset = HashMap::new();
    let mut current_rank_top_offset = *rank_to_max_height.iter().next().unwrap().1 * -0.5;
    for (rank, max_height) in rank_to_max_height {
        // The center of the rank is the middle of the max height plus the top of the rank.
        rank_to_y_offset.insert(rank, current_rank_top_offset + max_height * 0.5);
        // Shift by the height of the rank. The height of a rank already includes the vertex
        // spacing.
        current_rank_top_offset += max_height;
    }

    (
        x_coordinates
            .into_iter()
            // calculate y coordinate
            .map(|(v, x)| {
                (
                    graph[v].id,
                    (x, *rank_to_y_offset.get(&graph[v].rank).unwrap()),
                )
            })
            .collect::<Vec<_>>(),
        width,
        height,
    )
}

/// The slack of an edge: `rank(head) - rank(tail) - minimum_length`. An edge
/// is *tight* when its slack is 0. Only meaningful once ranks are assigned
/// (phase 1).
///
/// # Panics
///
/// Panics if `edge` is not an edge of `graph`.
pub fn slack(graph: &StableDiGraph<Vertex, Edge>, edge: EdgeIndex, minimum_length: i32) -> i32 {
    let (tail, head) = graph.edge_endpoints(edge).unwrap();
    graph[head].rank - graph[tail].rank - minimum_length
}

#[allow(dead_code)]
fn print_to_console(
    dir: VDir,
    graph: &StableDiGraph<Vertex, Edge>,
    layers: &[Vec<NodeIndex>],
    mut coordinates: HashMap<NodeIndex, isize>,
    vertex_spacing: usize,
) {
    let min = *coordinates.values().min().unwrap();
    let str_width = 4;
    coordinates
        .values_mut()
        .for_each(|v| *v = str_width * (*v - min) / vertex_spacing as isize);
    let width = *coordinates.values().max().unwrap() as usize;

    for line in layers {
        let mut v_line = vec!['-'; width + str_width as usize];
        let mut a_line = vec![' '; width + str_width as usize];
        for v in line {
            let pos = *coordinates.get(v).unwrap() as usize;
            if graph[*v].root != *v {
                a_line[pos] = if dir == VDir::Up { 'v' } else { '^' };
            }
            for (i, c) in v.index().to_string().chars().enumerate() {
                v_line[pos + i] = c;
            }
        }
        match dir {
            VDir::Up => {
                println!("{}", v_line.into_iter().collect::<String>());
                println!("{}", a_line.into_iter().collect::<String>());
            }
            VDir::Down => {
                println!("{}", a_line.into_iter().collect::<String>());
                println!("{}", v_line.into_iter().collect::<String>());
            }
        }
    }
    println!();
}

#[test]
fn self_loops_do_not_panic() {
    // graphs containing self-loops used to panic in cycle removal
    let edges = [(0, 1), (1, 1)];

    let graph = StableDiGraph::from_edges(edges);

    let layouts = start(graph, &Config::default());

    assert_eq!(layouts.len(), 1);
    let mut ids = layouts[0].0.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, vec![0, 1]);
}

#[test]
fn is_valid_layout() {
    fn has_duplicates<T: Eq + std::hash::Hash>(vec: &[T]) -> bool {
        let mut seen = std::collections::HashSet::new();
        for item in vec {
            let is_new = seen.insert(item);
            if !is_new {
                return true; // Found a duplicate
            }
        }
        false // No duplicates found
    }

    fn layout_is_valid(layout: &[(usize, (f64, f64))]) -> bool {
        let rank_scale = 2_i64.pow(31) as f64; // make space to pack x & y into an i64
        let xs = layout
            .iter()
            .map(|(_s, (x, y))| (y * rank_scale + x * 100.0).round() as i64)
            .collect::<Vec<_>>();

        !has_duplicates(&xs)
    }

    // this graph failed to create a valid layout
    // in versions <= 0.3
    let edges = [
        (2, 1),
        (3, 1),
        (7, 4),
        (8, 7),
        (9, 2),
        (10, 1),
        (4, 2),
        (6, 1),
        (11, 4),
        (5, 4),
        (12, 1),
    ];

    let graph = StableDiGraph::from_edges(edges);

    let layouts = start(graph, &Config::default());

    for (positions, _, _) in layouts {
        assert!(layout_is_valid(&positions));
    }
}

#[test]
fn init_graph_strips_self_loops() {
    let mut graph = StableDiGraph::<Vertex, Edge>::from_edges([(0, 1), (1, 1), (1, 2)]);

    init_graph(&mut graph);

    assert_eq!(graph.node_count(), 3);
    assert_eq!(graph.edge_count(), 2);
    assert!(graph.edge_indices().all(|e| {
        let (tail, head) = graph.edge_endpoints(e).unwrap();
        tail != head
    }));
}
