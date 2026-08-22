//! Phase 3 of the algorithm: coordinate assignment.
//!
//! The x-coordinates are computed with the algorithm from the 2001 paper
//! "Fast and Simple Horizontal Coordinate Assignment" by Brandes and Köpf
//! ([link](https://doi.org/10.1007/3-540-45848-4_3)): after marking type 1
//! conflicts (edge crossings between a long-edge segment and an ordinary
//! edge), four extremal candidate layouts are produced — one per combination
//! of vertical ([`VDir`]) and horizontal ([`HDir`]) direction — by aligning
//! each vertex vertically with a median neighbor into blocks and compacting
//! the blocks horizontally. The candidates are aligned to the narrowest one
//! ([`align_to_smallest_width_layout`]) and combined by taking, per vertex,
//! the average of the two median candidate coordinates
//! ([`calculate_relative_coords`]).
//!
//! This implementation deviates from the paper in two ways: vertex
//! separation respects individual vertex widths (either per alignment block
//! or, with [`crate::configure::Config::per_pair_separation`], per pair of
//! adjacent vertices plus an explicit vertex/edge gap), and when exactly one
//! of two distinct median neighbors is a dummy vertex, alignment prefers the
//! dummy so long edges stay straight.

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use log::info;
use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::visit::EdgeRef;
use petgraph::Direction::Incoming;

use super::{slack, Edge, Vertex};
use crate::configure::PairSeparation;

/// Computes the four Brandes-Köpf candidate layouts, one map of x-coordinates
/// per combination of vertical and horizontal direction, in the order
/// `[Down/Right, Down/Left, Up/Right, Up/Left]`. Coordinates of the Left
/// runs are already mirrored back, and the graph and layers are restored to
/// their original orientation on return (they are rotated while the
/// function runs).
///
/// `layers` must cover every vertex of the graph exactly once, grouped by
/// rank and ordered by position (as produced by phase 2), and no layer may
/// be empty.
///
/// With `per_pair_separation` set, adjacent vertices are separated by their
/// own widths plus the given explicit gaps instead of by the maximum vertex
/// widths of their alignment blocks; see
/// [`crate::configure::Config::per_pair_separation`].
///
/// # Panics
///
/// Panics (with an index out of bounds) if a layer is empty — note that
/// [`super::p2_reduce_crossings::remove_dummy_vertices`] can leave empty
/// layers behind — or if `layers` does not cover the graph as described
/// above.
pub fn create_layouts(
    graph: &mut StableDiGraph<Vertex, Edge>,
    layers: &mut [Vec<NodeIndex>],
    per_pair_separation: Option<PairSeparation>,
) -> Vec<HashMap<NodeIndex, f64>> {
    info!(target: "coordinate_calculation", "Creating individual layouts for coordinate calculation");
    let mut layouts = Vec::new();
    // marking reads `Vertex::rank` and `Vertex::pos`, which are only valid
    // after an alignment reset
    reset_alignment(graph, layers);
    mark_type_1_conflicts(graph, layers);
    // calculate the coordinates for each direction
    for _v_dir in [VDir::Down, VDir::Up] {
        for h_dir in [HDir::Right, HDir::Left] {
            // reset root, align and sink values
            info!(target: "coordinate_calculation",
                "creating layouts for vertical direction: {:?}, horizontal direction {:?}", 
                _v_dir, 
                h_dir);

            reset_alignment(graph, layers);
            create_vertical_alignments(graph, layers);
            let mut layout = do_horizontal_compaction(graph, layers, per_pair_separation);
            // flip x_coordinates if we went from right to left
            if let HDir::Left = h_dir {
                layout.values_mut().for_each(|x| *x = -*x);
            }
            // print_to_console(v_dir, graph, &orig_layers, layout.clone(), vertex_spacing);
            layouts.push(layout);

            // rotate the graph
            for row in layers.iter_mut() {
                row.reverse();
            }
        }
        // rotate the graph
        graph.reverse();
        layers.reverse();
    }
    // do this one last time, so ranks are in original order
    reset_alignment(graph, layers);
    layouts
}

/// Shifts the four candidate layouts of [`create_layouts`] so they share the
/// horizontal extent of the narrowest one: layouts with an even index ran
/// left-to-right and are aligned on their leftmost vertex extent, odd ones
/// ran right-to-left and are aligned on their rightmost. Extents take the
/// vertex widths ([`Vertex::size`]) into account.
pub fn align_to_smallest_width_layout(
    graph: &StableDiGraph<Vertex, Edge>,
    aligned_layouts: &mut [HashMap<NodeIndex, f64>],
) {
    info!(target: "coordinate_calculation", "Aligning all layouts to the one with the smallest width");
    // determine the left- and rightmost vertex extent of each layout, plus the
    // resulting width; coordinates are vertex centers, so the extents reach
    // half a vertex width beyond them
    let min_max: Vec<(f64, f64, f64)> = aligned_layouts
        .iter()
        .map(|c| {
            let min = c
                .iter()
                .map(|(v, x)| x - graph[*v].size.0 * 0.5)
                .min_by(|a, b| a.total_cmp(b))
                .unwrap();
            let max = c
                .iter()
                .map(|(v, x)| x + graph[*v].size.0 * 0.5)
                .max_by(|a, b| a.total_cmp(b))
                .unwrap();
            (min, max, max - min)
        })
        .collect();

    // determine the layout with the minimum width
    let min_width = min_max
        .iter()
        .enumerate()
        .min_by(|a, b| a.1 .2.total_cmp(&b.1 .2))
        .unwrap()
        .0;

    // align all other layouts to the layout with the minimum width: layouts
    // with an even index ran left-to-right and get aligned on their leftmost
    // extent, odd ones ran right-to-left and get aligned on their rightmost
    for (i, layout) in aligned_layouts.iter_mut().enumerate() {
        let shift = if i % 2 == 0 {
            min_max[min_width].0 - min_max[i].0
        } else {
            min_max[min_width].1 - min_max[i].1
        };
        for v in layout.values_mut() {
            let new = *v + shift;
            *v = new;
        }
    }
}

/// Combines candidate layouts into the final x-coordinate per vertex by
/// averaging the two median candidate values ("the average median is both
/// order and separation preserving", Brandes & Köpf). Expects the aligned
/// layouts of [`align_to_smallest_width_layout`].
///
/// The order of the returned pairs is unspecified and may differ between
/// runs (it derives from hash-map iteration); sort the result if a stable
/// order is needed.
///
/// # Panics
///
/// Panics if fewer than four layouts are given, or if a key of the first
/// layout is missing from any of layouts 1-3. Layouts beyond the fourth,
/// and keys not present in the first layout, are silently ignored.
pub fn calculate_relative_coords(
    aligned_layouts: Vec<HashMap<NodeIndex, f64>>,
) -> Vec<(NodeIndex, f64)> {
    info!(target: "coordinate_calculation", 
        "Calculate relative coordinates, by taking average between two medians of absolute x-coordinates for each layout direction");
    let mut sorted_layouts = HashMap::new();
    for k in aligned_layouts.first().unwrap().keys() {
        let mut vertex_coordinates = [
            *aligned_layouts.first().unwrap().get(k).unwrap(),
            *aligned_layouts.get(1).unwrap().get(k).unwrap(),
            *aligned_layouts.get(2).unwrap().get(k).unwrap(),
            *aligned_layouts.get(3).unwrap().get(k).unwrap(),
        ];
        vertex_coordinates.sort_by(|a, b| a.total_cmp(b));
        sorted_layouts.insert(k, vertex_coordinates);
    }

    // create final layout, by averaging the two median values
    // try to use something like mean
    sorted_layouts
        .into_iter()
        // "the average median is both order and separation preserving" [Brandes & Kopf, 2001]
        .map(|(k, v)| (*k, (v[1] + v[2]) / 2.0))
        .collect::<Vec<_>>()
}

fn is_incident_to_inner_segment(graph: &StableDiGraph<Vertex, Edge>, id: NodeIndex) -> bool {
    graph[id].is_dummy
        && graph
            .neighbors_directed(id, Incoming)
            .any(|n| graph[n].is_dummy)
}

/// Assumes id is incident to inner segment
fn get_inner_segment_upper_neighbor(
    graph: &StableDiGraph<Vertex, Edge>,
    id: NodeIndex,
) -> Option<NodeIndex> {
    if is_incident_to_inner_segment(graph, id) {
        graph.neighbors_directed(id, Incoming).next()
    } else {
        None
    }
}

/// Marks every edge that crosses an inner segment (an edge between two
/// dummy vertices) with `Edge::has_type_1_conflict`, making it ineligible
/// for vertical alignment; long edges win over ordinary edges.
fn mark_type_1_conflicts(graph: &mut StableDiGraph<Vertex, Edge>, layers: &[Vec<NodeIndex>]) {
    info!(target: "coordinate_calculation", 
        "Marking type one conflicts (edge crossings between dummy vertices and non dummy vertices)");

    for (level, next_level) in layers[..layers.len() - 1].iter().zip(layers[1..].iter()) {
        let mut left_dummy_index = 0;
        let mut l = 0;
        for (l_1, dummy_candidate) in next_level.iter().enumerate() {
            let right_dummy_index = match get_inner_segment_upper_neighbor(graph, *dummy_candidate)
            {
                Some(id) => graph[id].pos,
                None => {
                    if l_1 == next_level.len() - 1 {
                        level.len()
                    } else {
                        continue;
                    }
                }
            };
            while l <= l_1 {
                let vertex = next_level[l];
                let mut upper_neighbors = graph
                    .neighbors_directed(vertex, Incoming)
                    .collect::<Vec<_>>();
                upper_neighbors.sort_by(|a, b| graph[*a].pos.cmp(&graph[*b].pos));
                for upper_neighbor in upper_neighbors {
                    let vertex_index = graph[upper_neighbor].pos;
                    if vertex_index < left_dummy_index || vertex_index > right_dummy_index {
                        // mark every parallel edge between the pair, so an
                        // unmarked duplicate can't be used for alignment
                        let edges = graph
                            .edges_connecting(upper_neighbor, vertex)
                            .map(|e| e.id())
                            .collect::<Vec<_>>();
                        for edge in edges {
                            graph[edge].has_type_1_conflict = true;
                        }
                    }
                }
                l += 1;
            }
            left_dummy_index = right_dummy_index;
        }
    }
}

/// Resets the per-vertex alignment state (root, align, sink, shift) and
/// re-derives `Vertex::rank` and `Vertex::pos` from the given layers, in
/// preparation for one alignment pass.
pub(super) fn reset_alignment(graph: &mut StableDiGraph<Vertex, Edge>, layers: &[Vec<NodeIndex>]) {
    for (rank, row) in layers.iter().enumerate() {
        for (pos, v) in row.iter().enumerate() {
            let weight: &mut Vertex = &mut graph[*v];
            weight.rank = rank as i32;
            weight.pos = pos;
            weight.shift = f64::INFINITY;
            weight.align = *v;
            weight.root = *v;
            weight.sink = *v;
        }
    }
}

// TODO: Change this so the graph gets rotated outside of the function
/// Aligns the graph in so called blocks, which are used in the next step
/// to determine the x-coordinate of a vertex.
fn create_vertical_alignments(
    graph: &mut StableDiGraph<Vertex, Edge>,
    layers: &mut [Vec<NodeIndex>],
) {
    info!(target: "coordinate_calculation", "Creating vertical alignments");
    for layer in layers {
        let mut r = -1;

        for v in layer.iter().copied() {
            let mut edges = graph
                .edges_directed(v, Incoming)
                .filter(|e| slack(graph, e.id(), 1) == 0)
                .map(|e| (e.id(), e.source()))
                .collect::<Vec<_>>();

            if edges.is_empty() {
                continue;
            }

            edges.sort_by(|e1, e2| graph[e1.1].pos.cmp(&graph[e2.1].pos));

            let d = (edges.len() as f64 + 1.) / 2. - 1.; // need to subtract one because indices are zero based
            let mut lower_upper_median = [d.floor() as usize, d.ceil() as usize];

            // when exactly one of two distinct median neighbors is a dummy,
            // prefer aligning with the dummy so long edges stay straight
            let [lo, hi] = lower_upper_median;
            if lo != hi && graph[edges[hi].1].is_dummy && !graph[edges[lo].1].is_dummy {
                lower_upper_median = [hi, lo];
            }

            for m in lower_upper_median {
                if graph[v].align == v {
                    let edge_id = edges[m].0;
                    let median_neighbor = edges[m].1;

                    if !graph[edge_id].has_type_1_conflict
                        && r < graph[median_neighbor].pos as isize
                    {
                        graph[median_neighbor].align = v;
                        graph[v].root = graph[median_neighbor].root;
                        graph[v].align = graph[v].root;
                        r = graph[median_neighbor].pos as isize;
                    }
                }
            }
        }
    }
}

/// Computes the x-coordinates of one alignment pass: places the blocks
/// formed by `create_vertical_alignments`, then shifts the block classes as
/// close together as possible.
fn do_horizontal_compaction(
    graph: &mut StableDiGraph<Vertex, Edge>,
    layers: &[Vec<NodeIndex>],
    per_pair_separation: Option<PairSeparation>,
) -> HashMap<NodeIndex, f64> {
    info!(target: "coordinate_calculation", "calculating coordinates for layout.");
    compute_separation_widths(graph, per_pair_separation);

    let mut x_coordinates = place_blocks(graph, layers, per_pair_separation);
    // calculate class shifts
    info!(target: "coordinate_calculation", "move blocks as close together as possible");
    for i in 0..layers.len() {
        let mut v = layers[i][0];
        if graph[v].sink == v {
            if graph[graph[v].sink].shift == f64::INFINITY {
                let v_sink = graph[v].sink;
                graph[v_sink].shift = 0.0;
            }
            let mut j = i; // level index
            let mut k = 0; // vertex in level index
            loop {
                v = layers[j][k];

                // traverse one block
                while graph[v].align != graph[v].root {
                    v = graph[v].align;
                    j += 1;

                    if graph[v].pos > 0 {
                        let u = pred(graph[v], layers);
                        let gap = separation(graph, v, u, per_pair_separation);
                        let distance_v_u = *x_coordinates.get(&v).unwrap()
                            - (*x_coordinates.get(&u).unwrap() + gap);
                        let u_sink = graph[u].sink;
                        graph[u_sink].shift = graph[u_sink]
                            .shift
                            .min(graph[graph[v].sink].shift + distance_v_u);
                    }
                }
                k = graph[v].pos + 1;

                if k == layers[j].len() || graph[v].sink != graph[layers[j][k]].sink {
                    break;
                }
            }
        }
    }

    // calculate absolute x-coordinates
    for v in graph.node_indices() {
        x_coordinates.insert(
            v,
            *x_coordinates.get(&v).unwrap() + graph[graph[v].sink].shift,
        );
    }
    x_coordinates
}

/// Assigns [Vertex::separation_width], the width used when separating a
/// vertex from its neighbors on the same layer: the maximum width of the
/// vertices in the vertex's block, or the vertex's own width if
/// `per_pair_separation` is enabled (the explicit gaps are added on top by
/// [`separation`]).
fn compute_separation_widths(
    graph: &mut StableDiGraph<Vertex, Edge>,
    per_pair_separation: Option<PairSeparation>,
) {
    if per_pair_separation.is_some() {
        for v in graph.node_indices().collect::<Vec<_>>() {
            graph[v].separation_width = graph[v].size.0;
        }
        return;
    }
    for root in graph
        .node_indices()
        .filter(|v| graph[*v].root == *v)
        // Collect so we can mutate nodes while iterating.
        .collect::<Vec<_>>()
    {
        let root_vertex = &mut graph[root];

        let mut max_vertex_width = root_vertex.size.0;
        let mut current = root_vertex.align;
        while current != root {
            let current_vertex = &graph[current];
            max_vertex_width = max_vertex_width.max(current_vertex.size.0);
            current = current_vertex.align;
        }

        let root_vertex = &mut graph[root];
        root_vertex.separation_width = max_vertex_width;

        current = root_vertex.align;
        while current != root {
            let current_vertex = &mut graph[current];
            current_vertex.separation_width = max_vertex_width;
            current = current_vertex.align;
        }
    }
}

/// The minimum center-to-center distance between the horizontally adjacent
/// vertices `a` and `b`: half the sum of their separation widths, plus, in
/// per-pair mode, the configured gap (the edge gap when either vertex is a
/// dummy). Both consumers of the separation (block placement and the sink
/// shifts) must use this same value, or block classes could overlap.
fn separation(
    graph: &StableDiGraph<Vertex, Edge>,
    a: NodeIndex,
    b: NodeIndex,
    per_pair_separation: Option<PairSeparation>,
) -> f64 {
    let base = (graph[a].separation_width + graph[b].separation_width) * 0.5;
    match per_pair_separation {
        None => base,
        Some(gaps) if graph[a].is_dummy || graph[b].is_dummy => base + gaps.edge_gap,
        Some(gaps) => base + gaps.vertex_gap,
    }
}

fn place_blocks(
    graph: &mut StableDiGraph<Vertex, Edge>,
    layers: &[Vec<NodeIndex>],
    per_pair_separation: Option<PairSeparation>,
) -> HashMap<NodeIndex, f64> {
    info!(target: "coordinate_calculation", "Placing vertices in blocks.");
    let mut x_coordinates = HashMap::new();
    // place blocks
    for root in graph
        .node_indices()
        .filter(|v| graph[*v].root == *v)
        .collect::<Vec<_>>()
    {
        place_block(graph, layers, root, &mut x_coordinates, per_pair_separation);
    }
    x_coordinates
}
fn place_block(
    graph: &mut StableDiGraph<Vertex, Edge>,
    layers: &[Vec<NodeIndex>],
    root: NodeIndex,
    x_coordinates: &mut HashMap<NodeIndex, f64>,
    per_pair_separation: Option<PairSeparation>,
) {
    if x_coordinates.get(&root).is_some() {
        return;
    }
    x_coordinates.insert(root, 0.0);
    let mut w = root;
    loop {
        if graph[w].pos > 0 {
            let pred_w = pred(graph[w], layers);
            let u = graph[pred_w].root;
            place_block(graph, layers, u, x_coordinates, per_pair_separation);
            // initialize sink of current node to have the same sink as the root
            if graph[root].sink == root {
                graph[root].sink = graph[u].sink;
            }
            if graph[root].sink == graph[u].sink {
                // the constraint is between the same-layer pair (w, pred_w);
                // applying it against x[u] is valid because every member of
                // u's block ends up on the x-coordinate of its root u
                let gap = separation(graph, w, pred_w, per_pair_separation);
                x_coordinates.insert(
                    root,
                    x_coordinates
                        .get(&root)
                        .unwrap()
                        .max(x_coordinates.get(&u).unwrap() + gap),
                );
            }
        }
        w = graph[w].align;
        if w == root {
            break;
        }
    }
    // align all other vertices in this block to the x-coordinate of the root
    while graph[w].align != root {
        w = graph[w].align;
        x_coordinates.insert(w, *x_coordinates.get(&root).unwrap());
        graph[w].sink = graph[root].sink;
    }
}

// the vertex immediately left of the given vertex within its layer
fn pred(vertex: Vertex, layers: &[Vec<NodeIndex>]) -> NodeIndex {
    layers[vertex.rank as usize][vertex.pos - 1]
}

/// The horizontal direction in which a Brandes-Köpf alignment pass runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HDir {
    /// The pass runs right-to-left.
    Left,
    /// The pass runs left-to-right.
    Right,
}

/// The vertical direction in which a Brandes-Köpf alignment pass runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VDir {
    /// The pass runs bottom-to-top.
    Up,
    /// The pass runs top-to-bottom.
    Down,
}
