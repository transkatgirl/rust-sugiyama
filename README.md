## Rust Sugiyama Fork

This is a fork of the [rust-sugiyama](https://crates.io/crates/rust-sugiyama) crate with the following changes:
- Fixes for upstream bugs
- Minor improvements to coordinate assignment
- Added CrossingMinimization::None
- Added Config::divide_components
- Added Config::per_pair_separation
- Access to the algorithm's internals

This fork was made with **heavy** assistance from Claude Fable 5.

The remainder of this README is adapted from `rust-sugiyama`.

## Description

An implementation of Sugiyama's algorithm for displaying a layered graph.

This crate heavily uses the crate [petgraph](https://crates.io/crates/petgraph) under the hood.

Cycle Removal is implemented by using the `greedy_feedback_arc_set` function of petgraph and then reversing the edges from the set.

The rank assignment algorithm is implemented according to the paper `A Technique for Drawing Directed Graphs` by Gansner et al. which can be found [here](https://ieeexplore.ieee.org/document/221135). It first assigns a node a layer and creates an optimal feasible tree for rank assignment.

Crossing Reduction follows the weighted median heuristic which is also described in the above paper, it is also possible to use the barycenter heuristic for crossing reduction via configuration. In order to count crossings, the Bilayer Cross Count algorithm as described in the paper `Simple and Efficient Bilayer Cross Counting` by Wilhelm Barth and Petra Mutzel and Michael Juenger. It can also be found [online](http://ls11-www.cs.tu-dortmund.de/downloads/papers/BJM04.pdf).

Finally, the implementation for coordinate assignment follows the algorithm provided by Brandes and Koepf, which can be found in this [paper](https://www.semanticscholar.org/paper/Fast-and-Simple-Horizontal-Coordinate-Assignment-Brandes-K%C3%B6pf/69cb129a8963b21775d6382d15b0b447b01eb1f8).

## Usage

Currently, there are three options to create a layout:
1. `from_edges`, which takes a `&[(u32, u32)]`
2. `from_vertices_and_edges`, which takes a `&[(u32, (f64, f64))]` (vertex ids with sizes) and a `&[(u32, u32)]`; note that the ids in the returned layouts are positions in the vertex slice, not the ids you supplied
3. `from_graph`, which takes a `petgraph::StableDiGraph<V, E>` and a closure computing the size of each vertex

They will divide the graph into its connected components and calculate the coordinates separately for each component.
This division can be turned off via `Config::divide_components`, in which case the graph is laid out as a whole and a single layout is returned (an empty graph yields no layouts); note that the default ranking type (`MinimizeEdgeLength`) then requires the graph to be connected, while `Up`, `Down` and `Original` also handle disconnected graphs.
Layout parameters like the minimum spacing between vertices are set via the `Config` struct, using struct update syntax.

### from_edges
This takes a `&[(u32, u32)]` slice and calculates the x and y coordinates, the height of the graph, and the width.

```rust
use rust_sugiyama_fork::{configure::Config, from_edges};

let edges = [
    (0, 1),
    //
    (1, 2),
    (1, 3),
    (1, 4),
    (1, 5),
    (1, 6),
    //
    (3, 7),
    (3, 8),
    //
    (4, 7),
    (4, 8),
    //
    (5, 7),
    (5, 8),
    //
    (6, 7),
    (6, 8),
    //
    (7, 9),
    //
    (8, 9),
];

let layouts = from_edges(
    &edges,
    &Config {
        vertex_spacing: 20.0,
        ..Default::default()
    },
);

for (layout, width, height) in layouts {
    println!("Coordinates: {:?}", layout);
    println!("width: {width}, height: {height}");
}
```

### from_graph
Takes as input a `&StableDiGraph<V, E>` plus a closure computing each vertex's size, and calculates the x and y coordinates, the height and width of the graph.
`NodeIndices` are preserved between layouts and map directly to the input graph.

```rust
use std::collections::HashMap;

use rust_sugiyama_fork::petgraph::stable_graph::StableDiGraph;
use rust_sugiyama_fork::{configure::Config, from_graph};

let mut g: StableDiGraph<String, usize> = StableDiGraph::new();

let rick = g.add_node("Rick".to_string());
let morty = g.add_node("Morty".to_string());
let beth = g.add_node("Beth".to_string());
let jerry = g.add_node("Jerry".to_string());
let summer = g.add_node("Summer".to_string());

g.add_edge(rick, beth, 1);
g.add_edge(rick, jerry, 1);
g.add_edge(beth, summer, 1);
g.add_edge(jerry, summer, 1);
g.add_edge(beth, morty, 1);
g.add_edge(jerry, morty, 1);

let layouts = from_graph(
    &g,
    &|_id, _name| (10.0, 10.0),
    &Config {
        vertex_spacing: 100.0,
        ..Default::default()
    },
)
.into_iter()
.map(|(layout, width, height)| {
    let mut new_layout = HashMap::new();
    for (id, coords) in layout {
        new_layout.insert(g[id].clone(), coords);
    }
    (new_layout, width, height)
})
.collect::<Vec<_>>();

for (layout, width, height) in layouts {
    println!("Coordinates: {:?}", layout);
    println!("width: {width}, height: {height}");
}
```

### configuration via envs
It is also possible to configure the algorithm via environment variables, using the method `Config::new_from_env()`.

Environment variables that can be set are:

|ENV|values|default|description|
|---|------|-------|-------|
| RUST_GRAPH_MIN_LEN    | integer, 1..=2147483647     | 1          | minimum number of layers an edge spans (structural only, does not affect vertical spacing) |
| RUST_GRAPH_V_SPACING  | float, >= 0                 | 10         | minimum spacing between vertices on the same layer |
| RUST_GRAPH_DUMMIES    | (y\|n)                       | y          | if dummy vertices are included when calculating the layout (dummies never appear in the final layout; disabling them lets long edges take no horizontal space) |
| RUST_GRAPH_R_TYPE     | (original\|minimize\|up\|down) | minimize   | defines how vertices are places vertically |
| RUST_GRAPH_CROSS_MIN  | (barycenter\|median\|none)   | barycenter | which heuristic to use for crossing reduction, or none to disable it |
| RUST_GRAPH_TRANSPOSE  | (y\|n)                       | y          | if transpose function is used to further try to reduce crossings (may increase runtime significally for large graphs) |
| RUST_GRAPH_DUMMY_SIZE | float, >= 0                 | 1.0        | absolute width of dummy vertices, if dummy vertices are included. small values squish the graph horizontally |
| RUST_GRAPH_DIVIDE_COMPONENTS | (y\|n)               | y          | if the graph is divided into its connected components before layout. if disabled, the default ranking type requires a connected graph |
| RUST_GRAPH_PER_PAIR_SEPARATION | n \| `<vertex_gap>,<edge_gap>` | n | separate adjacent vertices by their own widths plus an explicit border-to-border gap (the edge gap when a dummy vertex is involved) instead of by block-max widths; vertex spacing then only affects vertical spacing. produces tighter layouts |

## Advanced: the pipeline API

The `algorithm` module exposes the four phases of the algorithm individually, so they can be driven (or replaced) one by one. Every phase works on a `StableDiGraph<Vertex, Edge>` and mutates the vertex and edge weights in place:

```rust
use rust_sugiyama_fork::algorithm::{self, Edge, Vertex};
use rust_sugiyama_fork::configure::{CrossingMinimization, RankingType};
use rust_sugiyama_fork::petgraph::stable_graph::StableDiGraph;

let mut graph: StableDiGraph<Vertex, Edge> =
    StableDiGraph::from_edges([(0, 1), (0, 2), (1, 3), (2, 3)]);
for v in graph.node_weights_mut() {
    v.size = (10.0, 10.0); // vertex size including the desired spacing
}

algorithm::init_graph(&mut graph);
algorithm::p0_cycle_removal::remove_cycles(&mut graph);
algorithm::p1_layering::rank(&mut graph, 1, RankingType::MinimizeEdgeLength);
algorithm::p2_reduce_crossings::insert_dummy_vertices(&mut graph, 1.0);
let mut layers =
    algorithm::p2_reduce_crossings::ordering(&mut graph, CrossingMinimization::Barycenter, true);
let mut layouts =
    algorithm::p3_calculate_coordinates::create_layouts(&mut graph, &mut layers, None);
algorithm::p3_calculate_coordinates::align_to_smallest_width_layout(&graph, &mut layouts);
let x_coordinates = algorithm::p3_calculate_coordinates::calculate_relative_coords(layouts);
assert_eq!(x_coordinates.len(), graph.node_count());
```

See the documentation of the `algorithm` module and its phase submodules for the exact contract of each step; `algorithm::start` runs the whole pipeline on a prebuilt graph.
