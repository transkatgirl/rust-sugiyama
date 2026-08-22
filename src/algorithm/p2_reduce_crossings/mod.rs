//! Phase 2 of the algorithm: crossing reduction.
//!
//! Before the actual reduction, every edge spanning more than one rank is
//! broken into unit segments by [`insert_dummy_vertices`]. An initial order
//! of the vertices within each rank is found via depth first search, and
//! then improved by a bilayer sweep ([`reduce_crossings_bilayer_sweep`]):
//! the sweep runs down and up the layers alternately, reordering each layer
//! by a sort key computed from the neighbor positions in the previously
//! visited layer ([`barycenter`] or [`median`]), optionally followed by a
//! [`transpose`] step that swaps adjacent vertices while doing so reduces
//! crossings. The sweep stops after 4 iterations without improvement, and
//! the best order seen is kept.
//!
//! Crossings between two layers are counted with the accumulator tree
//! technique from the paper "Simple and Efficient Bilayer Cross Counting"
//! by Barth, Mutzel and Jünger
//! ([link](https://doi.org/10.7155/jgaa.00088)).

#[cfg(test)]
mod tests;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::ops::Deref;

use log::{debug, info, trace};
use petgraph::algo::toposort;
use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::Direction::{Incoming, Outgoing};

use crate::configure::CrossingMinimization;
use crate::util::{iterate, radix_sort, IterDir};

use super::{Edge, Vertex};

/// An order of the vertices of a layered graph: the layers from top to
/// bottom, each layer holding its vertices from left to right.
///
/// Alongside the layers, the position of every vertex within its layer is
/// kept, so neighbor positions can be looked up in O(1); the invariant is
/// `position(v) == index of v within its layer`. The mutating methods keep
/// the two in sync, which is why the layers are only handed out immutably
/// (via [`Order::layers`] or the [`Deref`] impl to `Vec<Vec<NodeIndex>>`).
#[derive(Clone)]
pub struct Order {
    layers: Vec<Vec<NodeIndex>>,
    positions: HashMap<NodeIndex, usize>,
}

impl Display for Order {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = String::new();
        for row in &self.layers {
            for c in row {
                s.push_str(&c.index().to_string());
                s.push(',')
            }
            s.push('\n');
        }
        f.write_str(&s)
    }
}

impl Order {
    /// Creates an order from the given layers, deriving the vertex
    /// positions. A vertex must appear in the layers exactly once.
    pub fn new(layers: Vec<Vec<NodeIndex>>) -> Self {
        let mut positions = HashMap::new();
        for l in &layers {
            for (pos, v) in l.iter().enumerate() {
                positions.insert(*v, pos);
            }
        }
        Self { layers, positions }
    }

    /// The layers from top to bottom, each holding its vertices from left to
    /// right.
    pub fn layers(&self) -> &[Vec<NodeIndex>] {
        &self.layers
    }

    /// Consumes the order, returning the layers.
    pub fn into_layers(self) -> Vec<Vec<NodeIndex>> {
        self.layers
    }

    /// The position of the vertex within its layer, or [None] if the vertex
    /// is not part of the order.
    pub fn position(&self, vertex: NodeIndex) -> Option<usize> {
        self.positions.get(&vertex).copied()
    }

    fn max_rank(&self) -> usize {
        self.len()
    }

    /// Swaps the vertices at positions `a` and `b` of the layer with rank
    /// `r`, keeping the tracked positions in sync.
    pub fn exchange(&mut self, a: usize, b: usize, r: usize) {
        // first update positions, then swap
        *self.positions.get_mut(&self.layers[r][a]).unwrap() = b;
        *self.positions.get_mut(&self.layers[r][b]).unwrap() = a;
        self.layers[r].swap(a, b);
    }

    /// The number of crossings among the edges incident to `v` and `w`
    /// themselves if `v` sat immediately left of `w`. Crossings involving
    /// edges of other vertices are **not** counted, so this is not a
    /// per-layer crossing total (use [`Order::bilayer_cross_count`] for
    /// that) — but it is sufficient for [`transpose`] to decide whether
    /// swapping two adjacent vertices pays off, which is what it is used
    /// for. Assumes every edge connects adjacent ranks (guaranteed after
    /// [`insert_dummy_vertices`]).
    ///
    /// # Panics
    ///
    /// Panics if any neighbor of `v` or `w` is not part of the order.
    pub fn cross_count_two_vertices(
        &self,
        v: NodeIndex,
        w: NodeIndex,
        graph: &StableDiGraph<Vertex, Edge>,
    ) -> usize {
        let mut crossings = 0;
        for dir in [Incoming, Outgoing] {
            let mut v_adjacent = graph
                .neighbors_directed(v, dir)
                .map(|n| *self.positions.get(&n).unwrap())
                .collect::<Vec<_>>();
            let mut w_adjacent = graph
                .neighbors_directed(w, dir)
                .map(|n| *self.positions.get(&n).unwrap())
                .collect::<Vec<_>>();
            v_adjacent.sort();
            w_adjacent.sort();
            crossings += Self::calculate_cross_count_two_vertices(&v_adjacent, &w_adjacent);
        }
        crossings
    }

    fn calculate_cross_count_two_vertices(v_adjacent: &[usize], w_adjacent: &[usize]) -> usize {
        let mut all_crossings = 0;
        let mut k = 0;
        for i in v_adjacent {
            let i = *i;
            let mut crossings = k;
            while k < w_adjacent.len() && w_adjacent[k] < i {
                let j = w_adjacent[k];
                if i > j {
                    crossings += 1;
                }
                k += 1;
            }
            all_crossings += crossings;
        }
        all_crossings
    }

    /// The total number of edge crossings in this order, summed over all
    /// pairs of neighboring layers via [`Order::bilayer_cross_count`] — see
    /// there for which edges are counted (edges the order does not fully
    /// cover are silently skipped).
    ///
    /// # Panics
    ///
    /// Panics if the order contains no layers.
    pub fn crossings(&self, graph: &StableDiGraph<Vertex, Edge>) -> usize {
        let mut cross_count = 0;
        for rank in 0..self.max_rank() - 1 {
            cross_count += self.bilayer_cross_count(graph, rank);
        }
        cross_count
    }

    /// The number of crossings between the layers with rank `rank` and
    /// `rank + 1`, counted with the accumulator tree technique by Barth,
    /// Mutzel and Jünger (see the [module docs](self)). Only edges between
    /// vertices on the two ranks are counted, so exact totals for long edges
    /// require the dummy vertices of [`insert_dummy_vertices`].
    ///
    /// Edges are matched via the endpoints' [`Vertex::rank`] fields (only a
    /// rank difference of exactly 1 counts), and an endpoint the order does
    /// not track is silently skipped. The rank fields must therefore agree
    /// with the order's layer grouping (as the layers of [`ordering`] do)
    /// and every vertex must appear in the order — otherwise the count is
    /// silently too low. (Unlike [`Order::cross_count_two_vertices`], which
    /// panics on untracked neighbors.)
    ///
    /// # Panics
    ///
    /// Panics unless both `rank` and `rank + 1` are layers of the order,
    /// i.e. `rank + 1` must be less than the number of layers.
    pub fn bilayer_cross_count(&self, graph: &StableDiGraph<Vertex, Edge>, rank: usize) -> usize {
        // find initial edge order
        let north = &self[rank];
        let south = &self[rank + 1];
        let mut len = south.len();
        let mut key_length = 0;
        while len > 0 {
            len /= 10;
            key_length += 1;
        }
        let edge_endpoint_positions = north
            .iter()
            .flat_map(|v| {
                radix_sort(
                    graph
                        .neighbors_directed(*v, Outgoing)
                        .filter(|n| graph[*v].rank.abs_diff(graph[*n].rank) == 1)
                        .filter_map(|n| self.positions.get(&n))
                        .copied()
                        .collect(),
                    key_length,
                )
            })
            .collect::<Vec<_>>();
        Self::count_crossings(edge_endpoint_positions, south.len())
    }

    fn count_crossings(endpoints: Vec<usize>, south_len: usize) -> usize {
        // build the accumulator tree
        let mut c = 0;
        while 1 << c < south_len {
            c += 1
        }
        let tree_size = (1 << (c + 1)) - 1;
        let first_index = (1 << c) - 1;
        let mut tree = vec![0; tree_size];

        let mut cross_count = 0;

        // traverse through the positions and adjust tree nodes
        for pos in endpoints {
            let mut index = pos + first_index;
            tree[index] += 1;
            while index > 0 {
                // traverse up the tree, incrementing the nodes of the tree
                // each time we visit them.
                //
                // When visiting a left node, add the value of the node on the right to
                // the cross count;
                if index % 2 == 1 {
                    cross_count += tree[index + 1]
                }
                index = (index - 1) / 2;
                tree[index] += 1;
            }
        }
        cross_count
    }
}

/// Read-only access to the layers; mutation must go through the [`Order`]
/// methods, which keep the tracked vertex positions in sync.
impl Deref for Order {
    type Target = Vec<Vec<NodeIndex>>;

    fn deref(&self) -> &Self::Target {
        &self.layers
    }
}

/// Replaces every edge spanning more than one rank with a chain of dummy
/// vertices ([`Vertex::is_dummy`], created with size `(dummy_size, 0.0)`)
/// and unit-length edges, one dummy per intermediate rank. Requires ranks
/// from phase 1 ([`super::p1_layering`]).
///
/// For a weakly connected graph this also guarantees that no rank is empty:
/// any would-be-empty rank is crossed by some edge, which now receives a
/// dummy vertex on that rank. (With [`crate::configure::Config::divide_components`]
/// disabled the graph may be disconnected; the same argument covers each
/// component's own rank range, and the supported ranking types leave no gap
/// between those ranges. Empty ranks do arise when the dummies are removed
/// again — see [`remove_dummy_vertices`].)
pub fn insert_dummy_vertices(graph: &mut StableDiGraph<Vertex, Edge>, dummy_size: f64) {
    info!(target: "crossing_reduction", "Inserting dummy vertices for edges spanning more than 1 rank");
    for edge in graph.edge_indices().collect::<Vec<_>>() {
        let (mut tail, head) = graph.edge_endpoints(edge).unwrap();
        if graph[head].rank - graph[tail].rank > 1 {
            trace!(target: "crossing_reduction",
                "Inserting {} dummy vertices between: ({}, {})", 
                graph[head].rank - graph[tail].rank - 1, 
                tail.index(), 
                head.index());

            // we don't need to remember edges that where removed
            graph.remove_edge(edge);
            for rank in (graph[tail].rank + 1)..graph[head].rank {
                let d = Vertex {
                    is_dummy: true,
                    size: (dummy_size, 0.0),
                    ..Default::default()
                };
                let new = graph.add_node(d);
                graph[new].align = new;
                graph[new].root = new;
                graph[new].sink = new;
                graph[new].rank = rank;
                graph.add_edge(tail, new, Edge::default());
                tail = new;
            }
            graph.add_edge(tail, head, Edge::default()); // add last dummy edge connecting to the head
        }
    }
}

/// The inverse of [`insert_dummy_vertices`] structurally: replaces each
/// dummy chain with a single edge between the chain's endpoints, then
/// removes all dummy vertices from the graph and from the given layers.
/// The recreated edges carry default weights ([`Edge::default`], weight 1)
/// — the original edge weights are **not** preserved (they were already
/// discarded by [`insert_dummy_vertices`]), so a subsequent
/// [`RankingType::MinimizeEdgeLength`][crate::configure::RankingType]
/// ranking sees default weights (and requires the scratch-state
/// preconditions of [`super::p1_layering::rank`]).
///
/// This can leave layers empty (a rank whose only occupants were dummy
/// vertices, which happens whenever an edge spanned that rank without a
/// vertex on it). Phase 3
/// ([`super::p3_calculate_coordinates::create_layouts`]) requires every
/// layer to be non-empty, so when driving the phases manually, drop empty
/// layers before continuing; [`super::execute_phase_3`] does so itself.
///
/// # Panics
///
/// Panics if the graph is cyclic, or if a vertex marked
/// [`Vertex::is_dummy`] has no outgoing neighbor — every dummy must lie on
/// a chain as created by [`insert_dummy_vertices`], with exactly one
/// outgoing edge.
pub fn remove_dummy_vertices(
    graph: &mut StableDiGraph<Vertex, Edge>,
    order: &mut [Vec<NodeIndex>],
) {
    // go through all nodes in topological order
    // see if any outgoing neighbors are dummies
    // follow them until the other non dummy node is found
    // insert old edge
    // remove all dummy nodes
    info!(target: "crossing_reduction", "Removing dummy vertices and inserting original edges.");
    let vertices = toposort(&*graph, None).unwrap();
    for v in vertices {
        let mut edges = Vec::new();
        for mut n in graph.neighbors_directed(v, Outgoing) {
            if graph[n].is_dummy {
                while graph[n].is_dummy {
                    let dummy_neighbors = graph.neighbors_directed(n, Outgoing).collect::<Vec<_>>();
                    //assert_eq!(dummy_neighbors.len(), 1);
                    n = dummy_neighbors[0];
                }
                edges.push((v, n));
            }
        }
        for (tail, head) in edges {
            graph.add_edge(tail, head, Edge::default());
        }
    }
    // remove from order
    for l in order {
        l.retain(|v| !graph[*v].is_dummy);
    }
    graph.retain_nodes(|g, v| !g[v].is_dummy);
}

// TODO: Maybe write store all upper neighbors on vertex directly
/// The main entry point of the phase: computes the final order of the
/// vertices within each rank, returning the layers top-to-bottom with each
/// layer ordered left-to-right. Requires ranks from phase 1; run
/// [`insert_dummy_vertices`] first for exact crossing counts on long edges
/// (and, when ranks may be empty, to make [`transpose`] safe — see below).
///
/// With [`CrossingMinimization::None`] the initial depth-first-search order
/// of [`init_order`] is returned untouched (and `transpose` has no effect);
/// otherwise the order is improved via
/// [`reduce_crossings_bilayer_sweep`].
///
/// # Panics
///
/// Panics if the graph is empty, or if any rank has no vertices while
/// `transpose` is enabled. An empty rank arises when an edge spans more
/// than one rank and no other vertex sits on a rank in between; running
/// [`insert_dummy_vertices`] first places a dummy vertex on every such
/// rank, which for weakly connected graphs guarantees no rank is empty.
pub fn ordering(
    graph: &mut StableDiGraph<Vertex, Edge>,
    crossing_minimization: CrossingMinimization,
    transpose: bool,
) -> Vec<Vec<NodeIndex>> {
    let order = init_order(graph);
    // move downwards for crossing reduction
    let cm_method = match crossing_minimization {
        CrossingMinimization::Barycenter => self::barycenter,
        CrossingMinimization::Median => self::median,
        CrossingMinimization::None => return order.into_layers(),
    };
    let order = reduce_crossings_bilayer_sweep(graph, order, cm_method, transpose);
    order.into_layers()
}

/// A crossing minimization heuristic, the extension point of
/// [`reduce_crossings_bilayer_sweep`] and [`order_layer`]: given the graph,
/// a vertex, the sweep direction (`true` when sweeping downwards) and the
/// current position of every vertex within its layer, it returns the sort
/// key the vertex's layer is reordered by. [`barycenter`] and [`median`] are
/// the built-in implementations.
pub type CMMethod =
    fn(&StableDiGraph<Vertex, Edge>, NodeIndex, bool, &HashMap<NodeIndex, usize>) -> f64;

/// Builds the initial order: vertices are assigned to the layer matching
/// their rank, in depth-first-search order. Requires ranks from phase 1.
///
/// # Panics
///
/// Panics if the graph is empty or a rank is negative.
pub fn init_order(graph: &StableDiGraph<Vertex, Edge>) -> Order {
    info!(target: "crossing_reduction", 
        "Initializing order of vertices in each rank via dfs.");

    fn dfs(
        v: NodeIndex,
        order: &mut Vec<Vec<NodeIndex>>,
        graph: &StableDiGraph<Vertex, Edge>,
        visited: &mut HashSet<NodeIndex>,
    ) {
        if !visited.contains(&v) {
            visited.insert(v);
            order[graph[v].rank as usize].push(v);
            graph
                .neighbors_directed(v, Outgoing)
                .for_each(|n| dfs(n, order, graph, visited))
        }
    }

    let max_rank = graph
        .node_weights()
        .map(|v| v.rank as usize)
        .max_by(|r1, r2| r1.cmp(r2))
        .expect("Got invalid ranking");
    let mut order = vec![Vec::new(); max_rank + 1];
    let mut visited = HashSet::new();

    // build initial order via dfs
    graph
        .node_indices()
        .for_each(|v| dfs(v, &mut order, graph, &mut visited));

    Order::new(order)
}

/// Improves an order by sweeping down and up the layers alternately,
/// reordering each layer with [`order_layer`] (and, if `transpose` is
/// enabled, swapping adjacent vertices via [`transpose`] after every sweep).
/// Stops after 4 sweeps without improvement and returns the best order seen,
/// judged by [`Order::crossings`].
pub fn reduce_crossings_bilayer_sweep(
    graph: &StableDiGraph<Vertex, Edge>,
    mut order: Order,
    cm_method: CMMethod,
    transpose: bool,
) -> Order {
    info!(target: "crossing_reduction", "Reducing crossings via bilayer sweep");
    let mut best_crossings = order.crossings(graph);
    debug!(target: "crossing_reduction", "Initial number of crossings: {best_crossings}");
    let mut last_best = 0;
    let mut best = order.clone();
    for i in 0.. {
        order = order_layer(graph, i % 2 == 0, &order, cm_method);
        if transpose {
            self::transpose(graph, &mut order, i % 2 == 0);
        }
        let crossings = order.crossings(graph);
        trace!(target: "crossing_reduction", "Current number of crossings: {crossings}");
        if crossings < best_crossings {
            best_crossings = crossings;
            debug!(target: "crossing_reduction", "Lowest number of crossings so far: {best_crossings}");
            best = order.clone();
            last_best = 0;
        } else {
            last_best += 1;
        }
        if last_best == 4 {
            info!(target: "crossing_reduction", "Didn't improve after 4 sweeps, returning");
            return best;
        }
    }
    best
}

/// Greedily swaps adjacent vertices within each layer as long as a swap
/// reduces the crossing count (judged via
/// [`Order::cross_count_two_vertices`]), visiting the layers top-to-bottom
/// when `move_down` is set and bottom-to-top otherwise. Described as the
/// `transpose` procedure in the paper by Gansner et al.
///
/// # Panics
///
/// Panics if any layer of the order is empty (see [`ordering`] on how
/// [`insert_dummy_vertices`] prevents empty layers).
pub fn transpose(graph: &StableDiGraph<Vertex, Edge>, order: &mut Order, move_down: bool) {
    trace!(target: "crossings_reduction",
        "Using transpose, try to swap vertices in each layer manually to reduce cross count");

    let mut improved = true;
    let iter_dir = if move_down {
        IterDir::Forward
    } else {
        IterDir::Backward
    };

    while improved {
        improved = false;
        for r in iterate(iter_dir, order.max_rank()) {
            trace!(target: "reduce_crossings", "Transpose vertices in rank {r}");
            for i in 0..order.layers[r].len() - 1 {
                let v = order.layers[r][i];
                let w = order.layers[r][i + 1];
                let v_w_crossing = order.cross_count_two_vertices(v, w, graph);
                let w_v_crossing = order.cross_count_two_vertices(w, v, graph);
                if v_w_crossing > w_v_crossing {
                    improved = true;
                    order.exchange(i, i + 1, r);
                }
            }
        }
        trace!(target: "reduce_crossings", "Did improve: {improved}");
    }
}

/// Performs one sweep of the crossing minimization: every layer (except the
/// starting one) is reordered by the sort key `cm_method` computes from the
/// neighbor positions in the previously visited layer. Sweeps top-to-bottom
/// when `move_down` is set, bottom-to-top otherwise, and returns the new
/// order.
///
/// # Panics
///
/// Panics if the order contains no layers.
pub fn order_layer(
    graph: &StableDiGraph<Vertex, Edge>,
    move_down: bool,
    cur_order: &Order,
    cm_method: CMMethod,
) -> Order {
    let mut new_order = vec![Vec::new(); cur_order.max_rank()];
    let mut positions = cur_order.positions.clone();
    let dir: Vec<usize> = if move_down {
        new_order[0].clone_from(&cur_order.layers[0]);
        (1..cur_order.max_rank()).collect()
    } else {
        new_order[cur_order.max_rank() - 1].clone_from(&cur_order.layers[cur_order.max_rank() - 1]);
        (0..cur_order.max_rank() - 1).rev().collect()
    };

    for rank in dir {
        trace!(target: "crossing_reduction", "Updating order of vertices in rank {rank}");
        trace!(target: "crossing_reduction", "Original order: {:?}",
            cur_order[rank]
                .iter()
                .map(|v| v.index())
                .collect::<Vec<_>>()
                .as_slice()
        );

        new_order[rank].clone_from(&cur_order[rank]);
        let ordering = new_order[rank]
            .iter()
            .map(|n| (*n, cm_method(graph, *n, move_down, &positions)))
            .collect::<HashMap<NodeIndex, f64>>();

        new_order[rank]
            .sort_by(|a, b| ordering.get(a).unwrap().total_cmp(ordering.get(b).unwrap()));

        new_order[rank].iter().enumerate().for_each(|(pos, v)| {
            positions.insert(*v, pos);
        });
        trace!(target: "crossing_reduction", "Updated order : {:?}",
            new_order[rank]
                .iter()
                .map(|v| v.index())
                .collect::<Vec<_>>()
                .as_slice()
        );
    }

    Order::new(new_order)
}

/// The barycenter heuristic ([`CrossingMinimization::Barycenter`]), a
/// [`CMMethod`]: the average of the positions of the vertex's neighbors on
/// the adjacent rank in the sweep direction (incoming neighbors when
/// sweeping down, outgoing when sweeping up). A vertex without neighbors on
/// that rank keeps its current position.
pub fn barycenter(
    graph: &StableDiGraph<Vertex, Edge>,
    vertex: NodeIndex,
    move_down: bool,
    positions: &HashMap<NodeIndex, usize>,
) -> f64 {
    let neighbors = if move_down {
        graph.neighbors_directed(vertex, Incoming)
    } else {
        graph.neighbors_directed(vertex, Outgoing)
    };

    // Only look at direct neighbors
    let adjacent = neighbors
        .filter(|n| graph[vertex].rank.abs_diff(graph[*n].rank) == 1)
        .map(|n| *positions.get(&n).unwrap())
        .collect::<Vec<usize>>();

    if adjacent.is_empty() {
        // no neighbors on the adjacent rank in the sweep direction: keep the
        // current position
        return *positions.get(&vertex).unwrap() as f64;
    }

    adjacent.iter().sum::<usize>() as f64 / adjacent.len() as f64
}

/// The weighted median heuristic ([`CrossingMinimization::Median`]) from the
/// paper by Gansner et al., a [`CMMethod`]: the median position of the
/// vertex's neighbors on the adjacent rank in the sweep direction. With an
/// even number of neighbors the two medians are interpolated, weighted by
/// how tightly the neighbor positions cluster on each side; when the
/// interpolation weights are both zero (e.g. duplicate positions from
/// parallel edges), the plain average of the two medians is used. A vertex
/// without neighbors in the sweep direction keeps its current position.
pub fn median(
    graph: &StableDiGraph<Vertex, Edge>,
    vertex: NodeIndex,
    move_down: bool,
    positions: &HashMap<NodeIndex, usize>,
) -> f64 {
    let neighbors = if move_down {
        graph.neighbors_directed(vertex, Incoming)
    } else {
        graph.neighbors_directed(vertex, Outgoing)
    };
    // Only look at direct neighbors
    let mut adjacent = neighbors
        .filter(|n| graph[vertex].rank.abs_diff(graph[*n].rank) == 1)
        .map(|n| *positions.get(&n).unwrap())
        .collect::<Vec<_>>();

    adjacent.sort();

    let length_p = adjacent.len();
    let m = length_p / 2;
    if length_p == 0 {
        // no neighbors in the sweep direction: keep the current position
        *positions.get(&vertex).unwrap() as f64
    } else if length_p % 2 == 1 {
        adjacent[m] as f64
    } else {
        let left = adjacent[m - 1] - adjacent[0];
        let right = adjacent[length_p - 1] - adjacent[m];
        if left + right == 0 {
            // the middle values equal the extremes on both sides (two
            // neighbors, or duplicate positions from e.g. parallel edges),
            // so the weighted interpolation would divide by zero; fall
            // back to the plain average
            (adjacent[m - 1] + adjacent[m]) as f64 / 2.
        } else {
            (adjacent[m - 1] * right + adjacent[m] * left) as f64 / (left + right) as f64
        }
    }
}
