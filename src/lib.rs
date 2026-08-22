#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

use std::collections::HashMap;

use algorithm::{Edge, Vertex};

use log::info;
use petgraph::{graph::NodeIndex, stable_graph::StableDiGraph};

pub mod algorithm;
pub mod configure;
pub mod util;

pub use petgraph;

pub use configure::{Config, CrossingMinimization, PairSeparation, RankingType};

/// The layout of a single (sub)graph: the laid out vertices as
/// `(id, (x, y))` pairs, followed by the width and the height of the layout.
///
/// The coordinates are the vertex centers, in the geometric units that the
/// vertex sizes and [`Config::vertex_spacing`] are expressed in. `width` and
/// `height` however are vertex and layer *counts*, not geometric extents;
/// see [`from_edges`] for the exact semantics.
pub type Layout = (Vec<(usize, (f64, f64))>, f64, f64);

/// A list of [`Layout`]s, one per weakly connected component of the input
/// (or at most one for the whole graph when
/// [`Config::divide_components`] is disabled — an empty graph yields no
/// layouts), generic over the vertex id
/// type `T` (`usize` in general, [`NodeIndex`] for [`from_graph`]).
pub type Layouts<T> = Vec<(Vec<(T, (f64, f64))>, f64, f64)>;

/// Creates a graph layout from edges, which are given as a `&[(u32, u32)]`.
///
/// The layouts are returned as a list of disjoint subgraphs containing the
/// subgraph layout, the width, and the height. The layout of a subgraph is a
/// list of the vertex number (as specified in the edges) and its x and y
/// position respectively.
///
/// # Width and height semantics
///
/// The returned `width` and `height` are vertex and layer *counts*, not
/// geometric extents: `width` is the maximum number of vertices in any layer
/// (including dummy vertices when [`Config::dummy_vertices`] is enabled), and
/// `height` is the number of layers (including layers that hold only dummy
/// vertices, or that are left empty when dummy vertices are disabled and
/// [`Config::minimum_length`] is greater than 1). The x and y coordinates, in
/// contrast, are in geometric units derived from the vertex sizes and
/// [`Config::vertex_spacing`]. To obtain the geometric bounding box of a
/// subgraph, compute it from the returned coordinates (and, when using sized
/// vertices, the vertex sizes).
///
/// # Self-loops
///
/// Edges whose tail and head are the same vertex cannot be drawn in a layered
/// layout and are ignored: the vertex is laid out as if the edge did not
/// exist.
///
/// # Panics
///
/// Panics if `config` is invalid; see [`algorithm::build_layout`].
pub fn from_edges(edges: &[(u32, u32)], config: &Config) -> Layouts<usize> {
    info!(target: "initializing", "Creating new layout from edges, containing {} edges", edges.len());
    let graph = StableDiGraph::from_edges(edges);
    algorithm::start(graph, config)
}

/// Creates a graph layout from a preexisting [StableDiGraph<V, E>].
///
/// The layouts are returned as a list of disjoint subgraphs containing the
/// subgraph layout, the width, and the height. The layout of a subgraph is a
/// list of the [NodeIndex] and its x and y position respectively.
///
/// The returned `width` and `height` are vertex and layer counts, not
/// geometric extents, and self-loops in `graph` are ignored; see
/// [`from_edges`] for the exact semantics.
///
/// # Panics
///
/// Panics if `config` is invalid; see [`algorithm::build_layout`].
pub fn from_graph<V, E>(
    graph: &StableDiGraph<V, E>,
    vertex_size: &impl Fn(NodeIndex, &V) -> (f64, f64),
    config: &Config,
) -> Layouts<NodeIndex> {
    info!(target: "initializing",
        "Creating new layout from existing graph, containing {} vertices and {} edges.",
        graph.node_count(),
        graph.edge_count());

    let graph = graph.map(
        |id, v| Vertex::new(id.index(), vertex_size(id, v)),
        |_, _| Edge::default(),
    );

    algorithm::start(graph, config)
        .into_iter()
        .map(|(l, w, h)| {
            (
                l.into_iter()
                    .map(|(id, coords)| (NodeIndex::from(id as u32), coords))
                    .collect(),
                w,
                h,
            )
        })
        .collect()
}

/// Creates a graph layout from `&[(u32, (f64, f64))]` (vertices as vertex id
/// and vertex size) and `&[(u32, u32)]` (edges).
///
/// The layouts are returned as a list of disjoint subgraphs containing the
/// subgraph layout, the width, and the height. The returned `width` and
/// `height` are vertex and layer counts, not geometric extents, and
/// self-loops in `edges` are ignored; see [`from_edges`] for the exact
/// semantics.
///
/// Note that the `usize` in each layout entry is **not** the vertex id given
/// in `vertices`: it is the vertex's position in the `vertices` slice
/// (0-based, in insertion order). To recover the id you supplied, index back
/// into the slice: `vertices[entry].0`. Only when your ids already are
/// `0..vertices.len()` in order do the two coincide.
///
/// # Panics
///
/// Panics if `edges` contain vertices which are not contained in `vertices`,
/// or if `config` is invalid (see [`algorithm::build_layout`]).
pub fn from_vertices_and_edges<'a>(
    vertices: &'a [(u32, (f64, f64))],
    edges: &'a [(u32, u32)],
    config: &Config,
) -> Layouts<usize> {
    info!(target: "initializing",
        "Creating new layout from existing graph, containing {} vertices and {} edges.",
        vertices.len(),
        edges.len());

    let mut graph = StableDiGraph::new();
    let mut id_map = HashMap::new();
    for &(v, size) in vertices {
        let id = graph.add_node(Vertex::new(v as usize, size));
        id_map.insert(v, id);
    }

    for (tail, head) in edges {
        graph.add_edge(
            *id_map.get(tail).unwrap(),
            *id_map.get(head).unwrap(),
            Edge::default(),
        );
    }

    algorithm::start(graph, config)
}

#[test]
fn run_algo_empty_graph() {
    let edges = [];
    let g = from_edges(&edges, &Config::default());
    assert!(g.is_empty());
}

// the documented invalid-config panic holds even when the graph is empty
#[test]
#[should_panic(expected = "minimum_length must be at least 1")]
fn run_algo_empty_graph_invalid_config() {
    let _ = from_edges(
        &[],
        &Config {
            minimum_length: 0,
            ..Default::default()
        },
    );
}

// pins the documented self-loop semantics: the loop edge is ignored, the
// vertex is still laid out
#[test]
fn run_algo_self_loop_only() {
    let edges = [(0, 0)];
    let layouts = from_edges(&edges, &Config::default());
    assert_eq!(layouts.len(), 1);
    assert_eq!(layouts[0].0.len(), 1);
    assert_eq!(layouts[0].0[0].0, 0);
}

#[test]
fn run_algo_cycle_with_self_loop() {
    let edges = [(0, 1), (1, 2), (2, 0), (1, 1)];
    let layouts = from_edges(&edges, &Config::default());
    let mut ids = layouts
        .iter()
        .flat_map(|(l, _, _)| l.iter().map(|(id, _)| *id))
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, vec![0, 1, 2]);
}

// pins the documented id semantics: the returned ids are positions in the
// `vertices` slice, not the caller-supplied vertex ids
#[test]
fn from_vertices_and_edges_returns_slice_positions() {
    let vertices = [(10, (5.0, 5.0)), (20, (5.0, 5.0)), (30, (5.0, 5.0))];
    let edges = [(10, 20), (10, 30)];

    let layouts = from_vertices_and_edges(&vertices, &edges, &Config::default());

    let mut ids = layouts
        .iter()
        .flat_map(|(l, _, _)| l.iter().map(|(id, _)| *id))
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, vec![0, 1, 2]);
}

#[cfg(test)]
mod benchmark {
    use crate::configure::Config;

    use super::from_edges;

    #[test]
    fn r_100() {
        let edges = graph_generator::RandomLayout::new(100)
            .build_edges()
            .into_iter()
            .map(|(r, l)| (r as u32, l as u32))
            .collect::<Vec<(u32, u32)>>();
        let start = std::time::Instant::now();
        let _ = from_edges(&edges, &Config::default());
        println!("Random 100 edges: {}ms", start.elapsed().as_millis());
    }

    #[test]
    fn r_1000() {
        let edges = graph_generator::RandomLayout::new(1000)
            .build_edges()
            .into_iter()
            .map(|(r, l)| (r as u32, l as u32))
            .collect::<Vec<(u32, u32)>>();
        let start = std::time::Instant::now();
        let _ = from_edges(&edges, &Config::default());
        println!("Random 1000 edges: {}ms", start.elapsed().as_millis());
    }

    #[test]
    fn r_2000() {
        let edges = graph_generator::RandomLayout::new(2000).build_edges();
        let start = std::time::Instant::now();
        let _ = from_edges(&edges, &Config::default());
        println!("Random 2000 edges: {}ms", start.elapsed().as_millis());
    }

    #[test]
    fn r_4000() {
        let edges = graph_generator::RandomLayout::new(4000).build_edges();
        let start = std::time::Instant::now();
        let _ = from_edges(&edges, &Config::default());
        println!("Random 4000 edges: {}ms", start.elapsed().as_millis());
    }

    #[test]
    fn l_1000_2() {
        let n = 1000;
        let e = 2;
        let edges = graph_generator::GraphLayout::new_from_num_nodes(n, e).build_edges();
        let start = std::time::Instant::now();
        let _ = from_edges(&edges, &Config::default());
        println!(
            "{n} nodes, {e} edges per node: {}ms",
            start.elapsed().as_millis()
        );
    }

    #[test]
    fn l_2000_2() {
        let n = 2000;
        let e = 2;
        let edges = graph_generator::GraphLayout::new_from_num_nodes(n, e).build_edges();
        let start = std::time::Instant::now();
        let _ = from_edges(&edges, &Config::default());
        println!(
            "{n} nodes, {e} edges per node: {}ms",
            start.elapsed().as_millis()
        );
    }

    #[test]
    fn l_4000_2() {
        let n = 4000;
        let e = 2;
        let edges = graph_generator::GraphLayout::new_from_num_nodes(n, e).build_edges();
        let start = std::time::Instant::now();
        let _ = from_edges(&edges, &Config::default());
        println!(
            "{n} nodes, {e} edges per node: {}ms",
            start.elapsed().as_millis()
        );
    }

    #[test]
    fn l_8000_2() {
        let n = 8000;
        let e = 2;
        let edges = graph_generator::GraphLayout::new_from_num_nodes(n, e).build_edges();
        let start = std::time::Instant::now();
        let _ = from_edges(&edges, &Config::default());
        println!(
            "{n} nodes, {e} edges per node: {}ms",
            start.elapsed().as_millis()
        );
    }
}

#[cfg(test)]
mod check_visuals {

    use crate::{
        configure::{Config, CrossingMinimization, PairSeparation, RankingType},
        from_vertices_and_edges,
    };

    use super::from_edges;

    #[test]
    fn test_crossing_minimization_disabled() {
        let edges = [(0, 1), (1, 2), (2, 3), (2, 4), (3, 5), (4, 5), (0, 5)];
        let layouts = from_edges(
            &edges,
            &Config {
                c_minimization: CrossingMinimization::None,
                ..Default::default()
            },
        );
        assert_eq!(layouts.len(), 1);
        let (layout, _, _) = &layouts[0];
        for v in 0..=5 {
            assert!(layout.iter().any(|(id, _)| *id == v));
        }
    }

    #[test]
    fn test_per_pair_separation() {
        // a 5-rank chain 0-1-2-3-4 forming one tall block that contains the
        // wide vertex 2, plus vertex 5 sharing a rank with the narrow chain
        // member 1
        let vertices: [(u32, (f64, f64)); 6] = [
            (0, (10.0, 10.0)),
            (1, (10.0, 10.0)),
            (2, (200.0, 10.0)),
            (3, (10.0, 10.0)),
            (4, (10.0, 10.0)),
            (5, (10.0, 10.0)),
        ];
        let edges = [(0, 1), (1, 2), (2, 3), (3, 4), (0, 5)];

        // lays out the graph, checks that vertices sharing a rank don't
        // overlap horizontally (and keep the explicit gap, when one is
        // configured), and returns the layout's geometric width
        let run = |per_pair_separation: Option<PairSeparation>| {
            let layouts = from_vertices_and_edges(
                &vertices,
                &edges,
                &Config {
                    per_pair_separation,
                    ..Default::default()
                },
            );
            assert_eq!(layouts.len(), 1);
            let (layout, _, _) = layouts.into_iter().next().unwrap();
            assert_eq!(layout.len(), 6);

            // width of each vertex as phase 3 sees it: in block-max mode the
            // input width padded with the default vertex spacing, in per-pair
            // mode the input width as-is
            let width = |id: usize| match per_pair_separation {
                None => vertices[id].1 .0 + 10.0,
                Some(_) => vertices[id].1 .0,
            };
            // there are no dummies in this graph (every edge spans one
            // rank), so only the vertex gap applies
            let gap = per_pair_separation.map_or(0.0, |gaps| gaps.vertex_gap);

            let mut entries = layout;
            entries
                .sort_by(|(_, (ax, ay)), (_, (bx, by))| (ay, ax).partial_cmp(&(by, bx)).unwrap());
            for pair in entries.windows(2) {
                let (a, (ax, ay)) = pair[0];
                let (b, (bx, by)) = pair[1];
                if ay == by {
                    assert!(bx - ax >= (width(a) + width(b)) * 0.5 + gap - 1e-9);
                }
            }

            let left = entries
                .iter()
                .map(|(v, (x, _))| x - width(*v) * 0.5)
                .fold(f64::INFINITY, f64::min);
            let right = entries
                .iter()
                .map(|(v, (x, _))| x + width(*v) * 0.5)
                .fold(f64::NEG_INFINITY, f64::max);
            right - left
        };

        let block_max_width = run(None);
        let per_pair_width = run(Some(PairSeparation {
            vertex_gap: 10.0,
            edge_gap: 5.0,
        }));
        assert!(
            per_pair_width < block_max_width,
            "per-pair layout ({per_pair_width}) should be narrower than block-max ({block_max_width})"
        );
    }

    #[test]
    fn test_divide_components_disabled_disconnected() {
        // two components; the default ranking type requires a connected
        // graph, so use one that supports disconnected input
        let edges = [(0, 1), (2, 3)];
        let layouts = from_edges(
            &edges,
            &Config {
                divide_components: false,
                ranking_type: RankingType::Up,
                ..Default::default()
            },
        );
        assert_eq!(layouts.len(), 1);
        let (layout, _, _) = &layouts[0];
        assert_eq!(layout.len(), 4);
        for v in 0..=3 {
            assert!(layout.iter().any(|(id, _)| *id == v));
        }
        for (_, (x, y)) in layout {
            assert!(x.is_finite() && y.is_finite());
        }
        // vertices of different components share one coordinate space but
        // must not overlap
        for (i, (_, a)) in layout.iter().enumerate() {
            for (_, b) in &layout[i + 1..] {
                assert!(a != b);
            }
        }
    }

    #[test]
    fn test_divide_components_disabled_connected() {
        let edges = [(0, 1), (1, 2), (0, 3), (3, 2)];
        let layouts = from_edges(
            &edges,
            &Config {
                divide_components: false,
                ..Default::default()
            },
        );
        assert_eq!(layouts.len(), 1);
        let (layout, _, _) = &layouts[0];
        for v in 0..=3 {
            assert!(layout.iter().any(|(id, _)| *id == v));
        }
    }

    #[test]
    fn test_divide_components_disabled_empty_graph() {
        let edges = [];
        let layouts = from_edges(
            &edges,
            &Config {
                divide_components: false,
                ..Default::default()
            },
        );
        assert!(layouts.is_empty());
    }

    #[test]
    fn test_no_dummies() {
        let vertices = [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
        ];
        let edges = [
            (1, 2),
            (1, 3),
            (2, 5),
            (2, 16),
            (4, 5),
            (4, 6),
            (4, 7),
            (6, 17),
            (6, 3),
            (6, 18),
            (8, 3),
            (8, 9),
            (8, 10),
            (9, 16),
            (9, 7),
            (9, 19),
            (11, 7),
            (11, 12),
            (11, 13),
            (12, 18),
            (12, 10),
            (12, 20),
            (14, 10),
            (14, 15),
            (15, 19),
            (15, 13),
        ];
        let _ = from_vertices_and_edges(
            &vertices
                .into_iter()
                .map(|v| (v, (0.0, 0.0)))
                .collect::<Vec<_>>(),
            &edges,
            &Config {
                dummy_vertices: true,
                ..Default::default()
            },
        );
    }
    #[test]
    fn verify_looks_good() {
        // NOTE: This test might fail eventually, since the order of lements in a row canot be guaranteed;
        let edges = [
            (0, 1),
            (1, 2),
            (2, 3),
            (2, 4),
            (3, 5),
            (3, 6),
            (3, 7),
            (3, 8),
            (4, 5),
            (4, 6),
            (4, 7),
            (4, 8),
            (5, 9),
            (6, 9),
            (7, 9),
            (8, 9),
        ];
        let (layout, width, height) = &mut from_edges(&edges, &Config::default())[0];
        layout.sort_by(|a, b| a.0.cmp(&b.0));

        assert_eq!(*width, 4.0);
        assert_eq!(*height, 6.0);
        println!("{:?}", layout);
    }

    #[test]
    fn root_vertices_on_top_disabled() {
        let edges = [(1, 0), (2, 1), (3, 0), (4, 0)];
        let layout = from_edges(&edges, &Config::default());
        for (id, (_, y)) in layout[0].0.clone() {
            if id == 2 {
                assert_eq!(y, 0.0);
            } else if id == 3 || id == 4 || id == 1 {
                assert_eq!(y, 10.0);
            } else {
                assert_eq!(y, 20.0)
            }
        }
    }

    // regression test: the shift-to-zero minimum used to include dummy
    // vertices, leaving the leftmost real vertex at x > 0 when an edge
    // routing occupied the left margin
    #[test]
    fn leftmost_real_vertex_at_zero() {
        let edges = [(0, 1), (0, 2), (1, 3), (2, 3), (0, 3)];
        let layouts = from_edges(&edges, &Config::default());
        assert_eq!(layouts.len(), 1);
        let min_x = layouts[0]
            .0
            .iter()
            .map(|(_, (x, _))| *x)
            .fold(f64::INFINITY, f64::min);
        assert!(
            min_x.abs() < 1e-9,
            "leftmost real vertex should sit at x = 0, got {min_x}"
        );
    }

    #[test]
    fn check_coords_2() {
        let edges = [
            (0, 1),
            (0, 2),
            (0, 3),
            (1, 4),
            (4, 5),
            (5, 6),
            (2, 6),
            (3, 6),
            (3, 7),
            (3, 8),
            (3, 9),
        ];
        let layout = from_edges(&edges, &Config::default());
        println!("{:?}", layout);
    }

    #[test]
    fn hlrs_ping() {
        let _nodes = [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
        ];
        let edges = [
            (1, 2),
            (1, 4),
            (1, 5),
            (1, 3),
            (2, 4),
            (2, 5),
            (3, 9),
            (3, 10),
            (3, 8),
            (4, 6),
            (4, 9),
            (4, 8),
            (5, 6),
            (5, 10),
            (5, 8),
            (6, 7),
            (7, 9),
            (7, 10),
            (8, 14),
            (8, 15),
            (8, 13),
            (9, 11),
            (9, 14),
            (9, 13),
            (10, 11),
            (10, 15),
            (10, 13),
            (11, 12),
            (12, 14),
            (12, 15),
            (13, 18),
            (13, 19),
            (13, 20),
            (14, 16),
            (14, 18),
            (14, 20),
            (15, 16),
            (15, 19),
            (15, 20),
            (16, 17),
            (17, 18),
            (17, 19),
            (18, 21),
            (19, 21),
        ]
        .into_iter()
        .map(|(t, h)| (t - 1, h - 1))
        .collect::<Vec<_>>();

        let layout = from_edges(
            &edges,
            &Config {
                ranking_type: RankingType::Up,
                ..Default::default()
            },
        );
        println!("{layout:?}");
    }

    #[test]
    fn run_algo_empty_graph() {
        use super::from_edges;
        let edges = [];
        let g = from_edges(&edges, &Config::default());
        assert!(g.is_empty());
    }

    #[test]
    fn run_algo_with_duplicate_edges() {
        let edges = [
            (1, 2),
            (2, 5),
            (2, 6),
            (2, 3),
            (3, 4),
            (4, 3),
            (4, 8),
            (8, 4),
            (8, 7),
            (3, 7),
            (6, 7),
            (7, 6),
            (5, 6),
            (5, 1),
        ];

        let layout = from_edges(&edges, &Config::default());
        println!("{layout:?}");
    }

    // regression test: this acyclic graph with duplicate edges used to hang
    // the network simplex forever, because cut values were resolved via
    // endpoint pairs, which are ambiguous for parallel edges
    #[test]
    fn duplicate_edges_terminate() {
        let edges = [
            (0, 1),
            (0, 2),
            (2, 3),
            (3, 4),
            (0, 5),
            (3, 6),
            (3, 7),
            (4, 8),
            (2, 9),
            (8, 10),
            (0, 4),
            (1, 7),
            (3, 6),
            (0, 4),
            (5, 8),
            (5, 8),
            (1, 9),
            (4, 6),
            (0, 6),
            (3, 6),
            (0, 1),
            (7, 8),
            (5, 9),
            (6, 9),
            (0, 2),
            (2, 3),
            (1, 6),
            (1, 9),
        ];

        let layouts = from_edges(&edges, &Config::default());
        let mut seen = std::collections::HashSet::new();
        for (layout, _, _) in &layouts {
            for (_, (x, y)) in layout {
                assert!(x.is_finite() && y.is_finite());
                assert!(
                    seen.insert(((x * 100.0).round() as i64, (y * 100.0).round() as i64)),
                    "two vertices placed at the same coordinates"
                );
            }
        }
    }

    // regression test: tight edges spanning two ranks used to leave empty
    // ranks behind, which crashed crossing reduction and coordinate assignment
    #[test]
    fn minimum_length_2_no_panic() {
        for dummy_vertices in [true, false] {
            let config = Config {
                minimum_length: 2,
                dummy_vertices,
                ..Default::default()
            };
            for edges in [vec![(0, 1)], vec![(0, 1), (1, 2), (0, 2)]] {
                let layouts = from_edges(&edges, &config);
                let mut y = std::collections::HashMap::new();
                for (layout, _, _) in &layouts {
                    for (id, (_, vy)) in layout {
                        y.insert(*id, *vy);
                    }
                }
                for (tail, head) in &edges {
                    assert!(
                        y[&(*head as usize)] > y[&(*tail as usize)],
                        "edge ({tail}, {head}) does not point downwards"
                    );
                }
            }
        }
    }

    // regression test: duplicate parallel edges used to produce a NaN in the
    // weighted median, which crashed the ordering sort
    #[test]
    fn median_duplicate_edges_no_panic() {
        let edges = [(0, 2), (0, 2), (1, 2), (1, 2), (0, 3)];
        let config = Config {
            c_minimization: crate::configure::CrossingMinimization::Median,
            ..Default::default()
        };
        let layout = from_edges(&edges, &config);
        println!("{layout:?}");
    }
}
