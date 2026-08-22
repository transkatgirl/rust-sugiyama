//! Phase 1 of the algorithm: layering (ranking), which assigns each vertex
//! a rank ([`super::Vertex::rank`]).
//!
//! Four ranking algorithms are implemented, selected via
//! [`RankingType`]:
//!
//! 1. [`RankingType::Original`] - places each vertex at the midpoint of the
//!    highest and lowest rank it can occupy (the Up and Down rankings).
//! 2. [`RankingType::MinimizeEdgeLength`] - minimizes the weighted sum of
//!    edge lengths via the network simplex technique described in section
//!    4.2 of the 1993 paper "A technique for drawing directed graphs" by
//!    Gansner et al. ([link](https://ieeexplore.ieee.org/document/221135)):
//!    starting from a feasible tight spanning tree, tree edges with negative
//!    cut values are iteratively exchanged against the non-tree edge with
//!    minimum slack until no negative cut value remains.
//! 3. [`RankingType::Up`] - moves vertices as far up as possible.
//! 4. [`RankingType::Down`] - moves vertices as far down as possible.

// TODO: Keep non graph edges during rank() procedure in vecdeque to be able
// to cyclically search through them
mod cut_values;
mod low_lim;
pub(super) mod ranking;
#[cfg(test)]
pub(crate) mod tests;

pub use ranking::{init_rank, move_vertices_down, move_vertices_up};

use log::info;
use petgraph::stable_graph::{EdgeIndex, NodeIndex, StableDiGraph};
use petgraph::visit::IntoNodeIdentifiers;

use crate::configure::RankingType;

use self::cut_values::update_cutvalues;
use self::low_lim::update_low_lim;
use self::ranking::{feasible_tree, update_ranks};

use super::{slack, Edge, Vertex};

/// Assigns each vertex a rank ([`super::Vertex::rank`]) according to the
/// given [`RankingType`]; ranks are 0-based after normalization, with edges
/// pointing from lower to higher ranks. The
/// [`RankingType::MinimizeEdgeLength`] strategy respects the edge weights
/// ([`super::Edge::weight`]).
///
/// # Preconditions
///
/// The graph must be acyclic — run [`super::p0_cycle_removal`] first; a
/// cyclic graph panics (see below). `minimum_length` must be at least 1;
/// this is not checked, and violations silently produce broken rankings
/// (with `minimum_length` below 1, all vertices may end up on a single
/// rank).
///
/// Every vertex's rank must still be at its initial value `0` (the
/// [`super::Vertex`] default; note that [`super::init_graph`] does **not**
/// reset it). The existing ranks are read as input — the initial ranking
/// keeps the rank of vertices without incoming edges — so pre-seeded ranks
/// shift the entire ranking and can produce leading empty ranks, which
/// later phases reject.
///
/// The internal network simplex scratch state (spanning tree membership,
/// cut values, low/lim numbers) read by
/// [`RankingType::MinimizeEdgeLength`] must likewise be at its
/// [`super::Vertex`] / [`super::Edge`] defaults; neither
/// [`super::init_graph`] nor this function resets it. Re-running `rank()`
/// on a graph that already went through a `MinimizeEdgeLength` ranking
/// treats the leftover spanning tree as already optimal and silently
/// returns a feasible but un-minimized ranking.
///
/// # Panics
///
/// Panics if the graph is cyclic (all ranking types), or — with
/// [`RankingType::MinimizeEdgeLength`] — if the graph is empty or not
/// weakly connected; the other ranking types treat an empty graph as a
/// no-op.
pub fn rank(
    graph: &mut StableDiGraph<Vertex, Edge>,
    minimum_length: i32,
    ranking_type: RankingType,
) {
    info!(target: "ranking", "Start ranking, ranking type: {ranking_type:?}, minimum_length: {minimum_length}");
    init_rank(graph, minimum_length);
    match ranking_type {
        RankingType::Original => original(graph, minimum_length),
        RankingType::MinimizeEdgeLength => minimize_edge_length(graph, minimum_length),
        RankingType::Up => move_vertices_up(graph, minimum_length),
        RankingType::Down => move_vertices_down(graph, minimum_length),
    }
}

fn minimize_edge_length(graph: &mut StableDiGraph<Vertex, Edge>, minimum_length: i32) {
    feasible_tree(graph, minimum_length);
    while let Some(removed_edge) = leave_edge(graph) {
        // swap edges and calculate cut value
        let swap_edge = enter_edge(graph, removed_edge, minimum_length);
        exchange(graph, removed_edge, swap_edge, minimum_length);
    }

    // don't balance ranks since we want maximum width to
    // give indication about number of parallel processes running
    normalize(graph);
}

fn original(graph: &mut StableDiGraph<Vertex, Edge>, minimum_length: i32) {
    // place each vertex at the midpoint of the highest rank it can occupy
    // (up ranking) and the lowest (down ranking). The floor-average of two
    // feasible rankings is feasible: for an edge (u, v),
    // up(v) + down(v) >= up(u) + down(u) + 2 * minimum_length.
    move_vertices_up(graph, minimum_length);
    let up_ranks: Vec<(NodeIndex, i32)> = graph
        .node_identifiers()
        .map(|v| (v, graph[v].rank))
        .collect();
    move_vertices_down(graph, minimum_length);
    for (v, up) in up_ranks {
        graph[v].rank = (up + graph[v].rank) / 2;
    }
}

fn leave_edge(graph: &StableDiGraph<Vertex, Edge>) -> Option<EdgeIndex> {
    for edge in graph.edge_indices() {
        if let Some(cut_value) = graph[edge].cut_value {
            if cut_value < 0 {
                return Some(edge);
            }
        }
    }
    None
}

fn enter_edge(
    graph: &mut StableDiGraph<Vertex, Edge>,
    edge: EdgeIndex,
    minimum_length: i32,
) -> EdgeIndex {
    // find a non-tree edge to replace e.
    // remove e from tree
    // consider all edges going from head to tail component.
    // choose edge with minimum slack.
    let (mut u, mut v) = graph
        .edge_endpoints(edge)
        .map(|(t, h)| (graph[t], graph[h]))
        .unwrap();
    let is_root_in_head = u.lim < v.lim;
    if !is_root_in_head {
        std::mem::swap(&mut u, &mut v);
    }

    graph
        .edge_indices()
        .filter(|e| !graph[*e].is_tree_edge && is_head_to_tail(graph, *e, u, is_root_in_head))
        .min_by(|e1, e2| slack(graph, *e1, minimum_length).cmp(&slack(graph, *e2, minimum_length)))
        .unwrap()
}

fn exchange(
    graph: &mut StableDiGraph<Vertex, Edge>,
    removed_edge: EdgeIndex,
    swap_edge: EdgeIndex,
    minimum_length: i32,
) {
    // swap edges
    graph[removed_edge].is_tree_edge = false;
    graph[swap_edge].is_tree_edge = true;

    // update the graph
    let least_common_ancestor = update_cutvalues(graph, removed_edge, swap_edge);
    update_low_lim(graph, least_common_ancestor);
    update_ranks(graph, minimum_length);
}

fn normalize(graph: &mut StableDiGraph<Vertex, Edge>) {
    let min_rank = graph
        .node_identifiers()
        .map(|v| graph[v].rank)
        .min()
        .unwrap();
    for v in graph.node_weights_mut() {
        v.rank -= min_rank;
    }
}

fn is_head_to_tail(
    graph: &StableDiGraph<Vertex, Edge>,
    edge: EdgeIndex,
    u: Vertex,
    is_root_in_head: bool,
) -> bool {
    // edge needs to go from head to tail. e.g. tail neads to be in head component, and head in tail component
    let (tail, head) = graph
        .edge_endpoints(edge)
        .map(|(t, h)| (graph[t], graph[h]))
        .unwrap();
    // check if head is in tail component
    is_root_in_head == (u.low <= head.lim && head.lim <= u.lim) &&
    // check if tail is in head component
    is_root_in_head != (u.low <= tail.lim && tail.lim <= u.lim)
}
