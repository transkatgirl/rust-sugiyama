//! Integration tests that drive the layout pipeline through the crate's
//! public API only, guarding the visibility of the algorithm internals.

use rust_sugiyama_fork::algorithm::{self, Edge, Vertex};
use rust_sugiyama_fork::petgraph::stable_graph::StableDiGraph;
use rust_sugiyama_fork::{
    from_edges, Config, CrossingMinimization, Layouts, PairSeparation, RankingType,
};

/// Drives every phase manually on a graph containing a cycle, a self-loop
/// and an edge spanning several ranks, checking each phase's documented
/// contract along the way.
#[test]
fn manual_pipeline() {
    let mut graph: StableDiGraph<Vertex, Edge> =
        StableDiGraph::from_edges([(0, 1), (1, 2), (2, 0), (1, 1), (0, 3), (2, 3)]);
    for v in graph.node_weights_mut() {
        v.size = (10.0, 10.0);
    }
    let real_vertex_count = graph.node_count();

    // normalization strips the self-loop and stamps ids with node indices
    algorithm::init_graph(&mut graph);
    assert!(graph.edge_indices().all(|e| {
        let (tail, head) = graph.edge_endpoints(e).unwrap();
        tail != head
    }));
    assert!(graph.node_indices().all(|v| graph[v].id == v.index()));

    // phase 0 breaks the cycle 0 -> 1 -> 2 -> 0
    let reversed = algorithm::p0_cycle_removal::remove_cycles(&mut graph);
    assert!(!reversed.is_empty());

    // phase 1 assigns ranks so every edge points downwards with enough slack
    algorithm::p1_layering::rank(&mut graph, 1, RankingType::MinimizeEdgeLength);
    for e in graph.edge_indices() {
        assert!(algorithm::slack(&graph, e, 1) >= 0);
    }

    // phase 2: after dummy insertion every edge spans exactly one rank
    algorithm::p2_reduce_crossings::insert_dummy_vertices(&mut graph, 1.0);
    for e in graph.edge_indices() {
        let (tail, head) = graph.edge_endpoints(e).unwrap();
        assert_eq!(graph[head].rank - graph[tail].rank, 1);
    }
    let layers =
        algorithm::p2_reduce_crossings::ordering(&mut graph, CrossingMinimization::Median, true);
    let vertices_in_layers: usize = layers.iter().map(|l| l.len()).sum();
    assert_eq!(vertices_in_layers, graph.node_count());

    // an Order keeps the tracked positions in sync with the layers
    let order = algorithm::p2_reduce_crossings::Order::new(layers.clone());
    let _crossings = order.crossings(&graph);
    for layer in order.layers() {
        for (pos, v) in layer.iter().enumerate() {
            assert_eq!(order.position(*v), Some(pos));
        }
    }

    // phase 3 produces four candidate layouts which combine into one finite
    // x-coordinate per vertex (including dummies)
    let mut layers = layers;
    let mut layouts =
        algorithm::p3_calculate_coordinates::create_layouts(&mut graph, &mut layers, None);
    assert_eq!(layouts.len(), 4);
    algorithm::p3_calculate_coordinates::align_to_smallest_width_layout(&graph, &mut layouts);
    let x_coordinates = algorithm::p3_calculate_coordinates::calculate_relative_coords(layouts);
    assert_eq!(x_coordinates.len(), graph.node_count());
    assert!(x_coordinates.iter().all(|(_, x)| x.is_finite()));
    let real = x_coordinates
        .iter()
        .filter(|(v, _)| !graph[*v].is_dummy)
        .count();
    assert_eq!(real, real_vertex_count);
}

/// Per-pair separation with explicit gaps keeps the configured edge gap
/// between a routed edge (dummy vertex) and its real neighbor.
#[test]
fn manual_pipeline_per_pair_edge_gap() {
    let mut graph: StableDiGraph<Vertex, Edge> =
        StableDiGraph::from_edges([(0, 1), (1, 2), (0, 2)]);
    // raw sizes: in per-pair mode the caller does not bake spacing into them
    for v in graph.node_weights_mut() {
        v.size = (10.0, 10.0);
    }

    algorithm::init_graph(&mut graph);
    algorithm::p0_cycle_removal::remove_cycles(&mut graph);
    algorithm::p1_layering::rank(&mut graph, 1, RankingType::MinimizeEdgeLength);
    // the edge 0 -> 2 spans two ranks: one dummy lands on rank 1 beside vertex 1
    algorithm::p2_reduce_crossings::insert_dummy_vertices(&mut graph, 2.0);
    let mut layers = algorithm::p2_reduce_crossings::ordering(
        &mut graph,
        CrossingMinimization::Barycenter,
        true,
    );

    let mut layouts = algorithm::p3_calculate_coordinates::create_layouts(
        &mut graph,
        &mut layers,
        Some(PairSeparation {
            vertex_gap: 7.0,
            edge_gap: 3.0,
        }),
    );
    algorithm::p3_calculate_coordinates::align_to_smallest_width_layout(&graph, &mut layouts);
    let x: std::collections::HashMap<_, _> =
        algorithm::p3_calculate_coordinates::calculate_relative_coords(layouts)
            .into_iter()
            .collect();

    let dummy = graph
        .node_indices()
        .find(|v| graph[*v].is_dummy)
        .expect("dummy vertex on rank 1");
    let real = graph
        .node_indices()
        .find(|v| !graph[*v].is_dummy && graph[*v].rank == 1)
        .expect("real vertex on rank 1");
    // minimum center distance: half the pair's widths (10 and 2) plus the edge gap
    assert!((x[&real] - x[&dummy]).abs() >= (10.0 + 2.0) * 0.5 + 3.0 - 1e-9);
}

/// Without dummy vertices, an edge spanning more than one rank can leave a
/// rank empty, which `ordering` with `transpose` enabled rejects — the
/// panic documented on `ordering` and `transpose`.
#[test]
#[should_panic]
fn ordering_panics_on_empty_rank_without_dummies() {
    let mut graph: StableDiGraph<Vertex, Edge> = StableDiGraph::from_edges([(0, 1)]);
    algorithm::init_graph(&mut graph);
    algorithm::p0_cycle_removal::remove_cycles(&mut graph);
    // minimum_length 2 puts the vertices on ranks 0 and 2, leaving rank 1 empty
    algorithm::p1_layering::rank(&mut graph, 2, RankingType::MinimizeEdgeLength);
    algorithm::p2_reduce_crossings::ordering(&mut graph, CrossingMinimization::Barycenter, true);
}

/// `build_layout` accepts an empty graph, yielding an empty layout.
#[test]
fn build_layout_empty_graph() {
    let layout = algorithm::build_layout(StableDiGraph::new(), &Config::default());
    assert_eq!(layout, (Vec::new(), 0.0, 0.0));
}

/// `algorithm::start` on a prebuilt graph is the same computation the
/// crate-level `from_edges` performs.
#[test]
fn start_matches_from_edges() {
    fn sorted(mut layouts: Layouts<usize>) -> Layouts<usize> {
        for (layout, _, _) in &mut layouts {
            layout.sort_by_key(|(id, _)| *id);
        }
        layouts
    }

    let edges = [(0, 1), (1, 2), (1, 3), (2, 4), (3, 4), (0, 4)];
    let config = Config::default();

    let graph: StableDiGraph<Vertex, Edge> = StableDiGraph::from_edges(edges);
    let via_start = algorithm::start(graph, &config);
    let via_from_edges = from_edges(&edges, &config);

    assert_eq!(sorted(via_start), sorted(via_from_edges));
}
